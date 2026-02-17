//! Tool definitions and execution for AI agents.
//!
//! Real tools that interact with the filesystem, shell, GitHub, and miren.
//! Tools return structured results that get fed back to the LLM.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::llm::ToolDef;

/// Workspace for a project — isolated directory for generated code.
pub struct Workspace {
    pub root: PathBuf,
    pub project_name: String,
}

impl Workspace {
    /// Create a new workspace directory.
    pub async fn create(base: &Path, project_name: &str) -> Result<Self> {
        let safe_name: String = project_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
            .collect();
        let root = base.join(&safe_name);
        tokio::fs::create_dir_all(&root).await?;
        Ok(Self {
            root,
            project_name: safe_name,
        })
    }

    /// Write a file relative to workspace root.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<String> {
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&full, content).await?;
        Ok(format!("Wrote {} ({} bytes)", path, content.len()))
    }

    /// Read a file relative to workspace root.
    pub async fn read_file(&self, path: &str) -> Result<String> {
        let full = self.root.join(path);
        let content = tokio::fs::read_to_string(&full)
            .await
            .with_context(|| format!("Failed to read {path}"))?;
        Ok(content)
    }

    /// List files in workspace.
    pub async fn list_files(&self) -> Result<Vec<String>> {
        let root = self.root.clone();
        let files = tokio::task::spawn_blocking(move || list_files_sync(&root)).await?;
        Ok(files)
    }
}

/// List files recursively (sync, called from async context via spawn_blocking).
pub fn list_files_sync_pub(root: &Path) -> Vec<String> {
    list_files_sync(root)
}

fn list_files_sync(root: &Path) -> Vec<String> {
    let mut result = Vec::new();
    fn walk(dir: &Path, root: &Path, result: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__" {
                        continue;
                    }
                    walk(&path, root, result);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    result.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    walk(root, root, &mut result);
    result
}

/// Execute a shell command in a workspace.
pub async fn shell(workspace: &Workspace, cmd: &str, timeout_secs: u64) -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&workspace.root)
            .output(),
    )
    .await
    .context("Command timed out")?
    .context("Failed to execute command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = output.status;

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr] ");
        result.push_str(&stderr);
    }
    if !status.success() {
        result.push_str(&format!("\n[exit code: {}]", status.code().unwrap_or(-1)));
    }

    // Truncate very long output
    if result.len() > 8000 {
        result.truncate(8000);
        result.push_str("\n... (truncated)");
    }

    Ok(result)
}

/// Deploy a workspace to miren.
pub async fn miren_deploy(workspace: &Workspace) -> Result<String> {
    // Check if miren is initialized
    let miren_toml = workspace.root.join(".miren/app.toml");
    if !miren_toml.exists() {
        // Initialize
        let init_output = shell(workspace, &format!("miren init -n {}", workspace.project_name), 30).await?;
        tracing::info!("miren init: {init_output}");
    }

    // Deploy
    let output = shell(workspace, "miren deploy 2>&1", 120).await?;
    Ok(output)
}

/// Execute a tool call from the LLM and return the result.
pub async fn execute_tool(
    workspace: &Workspace,
    tool_name: &str,
    input: &Value,
) -> Result<String> {
    match tool_name {
        "write_file" => {
            let path = input["path"].as_str().unwrap_or("unnamed.txt");
            let content = input["content"].as_str().unwrap_or("");
            workspace.write_file(path, content).await
        }

        "read_file" => {
            let path = input["path"].as_str().unwrap_or("");
            workspace.read_file(path).await
        }

        "list_files" => {
            let root = workspace.root.clone();
            let files = tokio::task::spawn_blocking(move || list_files_sync(&root)).await?;
            Ok(files.join("\n"))
        }

        "shell" => {
            let cmd = input["command"].as_str().unwrap_or("echo 'no command'");
            let timeout = input["timeout"].as_u64().unwrap_or(30);
            shell(workspace, cmd, timeout).await
        }

        "deploy" => miren_deploy(workspace).await,

        _ => anyhow::bail!("Unknown tool: {tool_name}"),
    }
}

/// Tool definitions for the code-generation agent.
pub fn code_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "write_file".to_string(),
            description: "Write a file to the project workspace. Creates directories as needed."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative file path (e.g. 'src/main.py', 'Dockerfile')"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content"
                    }
                }
            }),
        },
        ToolDef {
            name: "read_file".to_string(),
            description: "Read a file from the project workspace.".to_string(),
            input_schema: json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative file path"
                    }
                }
            }),
        },
        ToolDef {
            name: "list_files".to_string(),
            description: "List all files in the project workspace.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDef {
            name: "shell".to_string(),
            description: "Run a shell command in the project workspace. Use for: installing dependencies, running tests, git operations, etc.".to_string(),
            input_schema: json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)"
                    }
                }
            }),
        },
        ToolDef {
            name: "deploy".to_string(),
            description: "Deploy the project to miren (PaaS). The project needs a Procfile. Returns the deployed URL.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}
