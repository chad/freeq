//! Factory orchestrator — coordinates agent roles through a build pipeline.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use crate::llm::{ContentBlock, LlmClient, Message, MessageContent, ToolResultBlock};
use crate::memory::Memory;
use crate::output::{self, AgentId};
use crate::tools::{self, Workspace};
use freeq_sdk::client::ClientHandle;

/// Factory configuration.
#[derive(Debug, Clone)]
pub struct FactoryConfig {
    /// Channel the factory operates in.
    pub channel: String,
    /// Base directory for project workspaces.
    pub workspace_base: PathBuf,
}

/// Factory state.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Specifying,
    Designing,
    Building,
    Reviewing,
    Testing,
    Deploying,
    Complete,
    Paused,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Idle => write!(f, "idle"),
            Phase::Specifying => write!(f, "specifying"),
            Phase::Designing => write!(f, "designing"),
            Phase::Building => write!(f, "building"),
            Phase::Reviewing => write!(f, "reviewing"),
            Phase::Testing => write!(f, "testing"),
            Phase::Deploying => write!(f, "deploying"),
            Phase::Complete => write!(f, "complete"),
            Phase::Paused => write!(f, "paused"),
        }
    }
}

/// Agent identities.
fn product() -> AgentId {
    AgentId {
        role: "product".to_string(),
        color: None,
    }
}
fn architect() -> AgentId {
    AgentId {
        role: "architect".to_string(),
        color: None,
    }
}
fn builder() -> AgentId {
    AgentId {
        role: "builder".to_string(),
        color: None,
    }
}
fn reviewer() -> AgentId {
    AgentId {
        role: "reviewer".to_string(),
        color: None,
    }
}
fn qa() -> AgentId {
    AgentId {
        role: "qa".to_string(),
        color: None,
    }
}
fn deployer() -> AgentId {
    AgentId {
        role: "deploy".to_string(),
        color: None,
    }
}

/// The software factory.
pub struct Factory {
    pub config: FactoryConfig,
    pub phase: Arc<Mutex<Phase>>,
    workspace: Arc<Mutex<Option<Workspace>>>,
    project_name: Arc<Mutex<Option<String>>>,
}

impl Factory {
    pub fn new(config: FactoryConfig) -> Self {
        Self {
            config,
            phase: Arc::new(Mutex::new(Phase::Idle)),
            workspace: Arc::new(Mutex::new(None)),
            project_name: Arc::new(Mutex::new(None)),
        }
    }

    /// Handle a user command directed at the factory.
    pub async fn handle_command(
        &self,
        handle: &ClientHandle,
        channel: &str,
        _sender: &str,
        command: &str,
        args: &str,
        llm: &LlmClient,
        memory: &Memory,
    ) -> Result<()> {
        match command {
            "build" | "create" | "make" => {
                self.start_build(handle, channel, args, llm, memory).await?;
            }
            "status" => {
                let phase = self.phase.lock().await;
                let project = self.project_name.lock().await;
                let name = project.as_deref().unwrap_or("none");
                output::status(
                    handle,
                    channel,
                    &product(),
                    "📊",
                    &format!("Phase: {phase} | Project: {name}"),
                )
                .await?;
            }
            "pause" => {
                *self.phase.lock().await = Phase::Paused;
                output::status(handle, channel, &product(), "⏸️", "Factory paused").await?;
            }
            "resume" => {
                output::status(handle, channel, &product(), "▶️", "Factory resumed").await?;
            }
            "spec" => {
                if let Some(ref name) = *self.project_name.lock().await {
                    if let Some(spec) = memory.get(name, "spec", "current")? {
                        output::say(handle, channel, &product(), &spec).await?;
                    } else {
                        output::say(handle, channel, &product(), "No spec yet.").await?;
                    }
                }
            }
            "files" => {
                if let Some(ref ws) = *self.workspace.lock().await {
                    let root = ws.root.clone();
                    let files = tokio::task::spawn_blocking(move || {
                        crate::tools::list_files_sync_pub(&root)
                    })
                    .await?;
                    output::file_tree(handle, channel, &builder(), &files).await?;
                }
            }
            _ => {
                output::say(
                    handle,
                    channel,
                    &product(),
                    "Unknown command. Try: build <spec>, status, pause, resume, spec, files",
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Run the full factory pipeline.
    async fn start_build(
        &self,
        handle: &ClientHandle,
        channel: &str,
        spec: &str,
        llm: &LlmClient,
        memory: &Memory,
    ) -> Result<()> {
        if spec.trim().is_empty() {
            output::say(
                handle,
                channel,
                &product(),
                "I need a spec! Tell me what to build.",
            )
            .await?;
            return Ok(());
        }

        // Phase 1: Product — clarify and write spec
        *self.phase.lock().await = Phase::Specifying;
        output::status(
            handle,
            channel,
            &product(),
            "📋",
            "Analyzing requirements...",
        )
        .await?;

        let refined_spec = llm.complete(
            "You are a product lead. Take the user's rough idea and produce a clear, concise product spec. Include: purpose, core features (bulleted), tech constraints (if any), and success criteria. Be specific but brief. Output ONLY the spec, no preamble.",
            spec,
        ).await?;

        let project_name = crate::prototype::generate_project_name_pub(llm, spec).await?;
        *self.project_name.lock().await = Some(project_name.clone());

        memory.set(&project_name, "spec", "current", &refined_spec)?;
        output::say(
            handle,
            channel,
            &product(),
            &format!("Project: {project_name}"),
        )
        .await?;
        output::say(handle, channel, &product(), &refined_spec).await?;

        // Phase 2: Architect — propose design
        *self.phase.lock().await = Phase::Designing;
        output::status(
            handle,
            channel,
            &architect(),
            "🏗️",
            "Designing architecture...",
        )
        .await?;

        let design = llm.complete(
            "You are a software architect. Given a product spec, propose a minimal, deployable architecture. Include: stack choice (prefer Python/Flask for speed), file structure, key abstractions. Be terse. Output ONLY the design, no preamble.",
            &refined_spec,
        ).await?;

        memory.set(&project_name, "decision", "architecture", &design)?;
        output::say(handle, channel, &architect(), &design).await?;

        // Phase 3: Builder — write code
        *self.phase.lock().await = Phase::Building;
        let workspace = Workspace::create(&self.config.workspace_base, &project_name).await?;

        let build_prompt = format!(
            "Build this project. Write ALL the code files, then deploy.\n\n## Spec\n{refined_spec}\n\n## Architecture\n{design}"
        );

        let tools = tools::code_tools();
        let mut messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(build_prompt),
        }];

        let mut deployed_url: Option<String> = None;

        // Agentic build loop
        for _iteration in 0..25 {
            let phase = { self.phase.lock().await.clone() };
            if phase == Phase::Paused {
                output::status(
                    handle,
                    channel,
                    &builder(),
                    "⏸️",
                    "Paused — waiting for /factory resume",
                )
                .await?;
                // In a real impl, we'd wait on a signal. For now, break.
                break;
            }

            let resp = llm.chat(BUILDER_SYSTEM, &messages, &tools, 4096).await?;

            let mut text_parts = Vec::new();
            let mut tool_uses = Vec::new();

            for block in &resp.content {
                match block {
                    ContentBlock::Text { text } => text_parts.push(text.clone()),
                    ContentBlock::ToolUse(tu) => tool_uses.push(tu.clone()),
                    _ => {}
                }
            }

            // Post commentary
            let commentary = text_parts.join("").trim().to_string();
            if !commentary.is_empty() && commentary.len() < 500 {
                output::say(handle, channel, &builder(), &commentary).await?;
            }

            if tool_uses.is_empty() {
                break;
            }

            // Add assistant message
            let mut response_blocks: Vec<ContentBlock> = Vec::new();
            for text in &text_parts {
                if !text.trim().is_empty() {
                    response_blocks.push(ContentBlock::Text { text: text.clone() });
                }
            }
            for tu in &tool_uses {
                response_blocks.push(ContentBlock::ToolUse(tu.clone()));
            }
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(response_blocks),
            });

            // Execute tools
            let mut result_blocks = Vec::new();
            for tu in &tool_uses {
                // Decide which agent is "talking"
                let agent = match tu.name.as_str() {
                    "deploy" => {
                        *self.phase.lock().await = Phase::Deploying;
                        deployer()
                    }
                    "shell" if tu.input["command"].as_str().unwrap_or("").contains("test") => {
                        *self.phase.lock().await = Phase::Testing;
                        qa()
                    }
                    _ => builder(),
                };

                match tu.name.as_str() {
                    "write_file" => {
                        let path = tu.input["path"].as_str().unwrap_or("?");
                        output::status(handle, channel, &agent, "✏️", &format!("Writing {path}"))
                            .await?;
                    }
                    "shell" => {
                        let cmd = tu.input["command"].as_str().unwrap_or("?");
                        let short = if cmd.len() > 60 { &cmd[..57] } else { cmd };
                        output::status(handle, channel, &agent, "⚙️", &format!("$ {short}"))
                            .await?;
                    }
                    "deploy" => {
                        output::status(handle, channel, &agent, "🚀", "Deploying...").await?;
                    }
                    _ => {}
                }

                let result = match tools::execute_tool(&workspace, &tu.name, &tu.input).await {
                    Ok(out) => {
                        if tu.name == "deploy"
                            && let Some(url) = extract_url(&out)
                        {
                            deployed_url = Some(url.clone());
                            output::deploy_result(handle, channel, &deployer(), &url).await?;
                            memory.set(&project_name, "deploy", "url", &url)?;
                        }
                        if tu.name == "write_file"
                            && let (Some(path), Some(content)) =
                                (tu.input["path"].as_str(), tu.input["content"].as_str())
                        {
                            memory.set(&project_name, "file", path, content)?;
                        }
                        out
                    }
                    Err(e) => {
                        output::error(handle, channel, &agent, &format!("{}: {e}", tu.name))
                            .await?;
                        format!("Error: {e}")
                    }
                };

                result_blocks.push(ContentBlock::ToolResult(ToolResultBlock {
                    tool_use_id: tu.id.clone(),
                    content: result,
                    is_error: None,
                }));
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });
        }

        // Phase 4: Review (quick pass)
        *self.phase.lock().await = Phase::Reviewing;
        let ctx = memory.project_context(&project_name)?;
        if !ctx.is_empty() {
            let review = llm.complete(
                "You are a code reviewer. Given a project's files and spec, give a brief review: what's good, what could be improved. Be constructive and concise. 3-5 bullet points max.",
                &ctx,
            ).await?;
            output::say(handle, channel, &reviewer(), &review).await?;
        }

        // Done
        *self.phase.lock().await = Phase::Complete;
        if let Some(ref url) = deployed_url {
            output::status(
                handle,
                channel,
                &product(),
                "✅",
                &format!("Factory complete! Live at: {url}"),
            )
            .await?;
        } else {
            output::status(handle, channel, &product(), "✅", "Factory complete!").await?;
        }

        // Store workspace
        *self.workspace.lock().await = Some(workspace);

        Ok(())
    }
}

const BUILDER_SYSTEM: &str = r#"You are the builder agent in a software factory. Write production-quality code.

Rules:
- Use Python (Flask) for web apps unless told otherwise. It deploys fastest.
- Always include: Procfile (with gunicorn), requirements.txt, and full app code.
- The Procfile format: web: python -m gunicorn --bind 0.0.0.0:${PORT:-8000} app:app
- Include gunicorn and flask in requirements.txt.
- Write complete, working code — not stubs or placeholders.
- Use clean structure: separate concerns, add comments.
- After writing files, deploy.

Tools available: write_file, read_file, list_files, shell, deploy."#;

fn extract_url(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("https://") {
            return Some(trimmed.to_string());
        }
    }
    None
}
