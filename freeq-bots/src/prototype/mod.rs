//! Spec-to-Prototype bot.
//!
//! Takes a product spec (dropped as a message) and produces:
//! 1. Architecture decision
//! 2. Generated code files
//! 3. Tests
//! 4. Deployment to miren with live URL
//!
//! All work is visible in the channel in real-time.

use anyhow::Result;
use std::path::Path;

use crate::llm::{ContentBlock, LlmClient, Message, MessageContent, ToolResultBlock};
use crate::memory::Memory;
use crate::output::{self, AgentId};
use crate::tools::{self, Workspace};
use freeq_sdk::client::ClientHandle;

const SYSTEM_PROMPT: &str = r#"You are a rapid prototype builder. Given a product spec, you build a working, deployable application.

Rules:
- Use Python (Flask) for web apps unless the spec requires something else. It's the fastest to deploy.
- Keep it minimal but functional. Real code, not stubs.
- Always include: Procfile, requirements.txt, and the actual application code.
- The Procfile must use: web: python -m gunicorn --bind 0.0.0.0:${PORT:-8000} app:app
- Include gunicorn and flask in requirements.txt.
- Write clean, readable code with comments.
- Build the COMPLETE app — all features from the spec, not just a skeleton.
- After writing all files, run any needed commands (pip install, tests).
- Always deploy at the end.

You have these tools:
- write_file: Create project files
- read_file: Read existing files
- list_files: See what's in the project
- shell: Run commands (install deps, run tests, etc.)
- deploy: Deploy to miren PaaS (returns URL)

Work step by step:
1. Analyze the spec
2. Write all code files
3. Test if possible
4. Deploy
5. Report the live URL"#;

/// Agents shown in channel.
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
fn deployer() -> AgentId {
    AgentId {
        role: "deploy".to_string(),
        color: None,
    }
}

/// Run the prototype pipeline for a spec.
pub async fn build(
    handle: &ClientHandle,
    channel: &str,
    spec: &str,
    llm: &LlmClient,
    memory: &Memory,
    workspace_base: &Path,
) -> Result<Option<String>> {
    // Generate a project name from the spec
    let project_name = generate_project_name(llm, spec).await?;

    output::status(
        handle,
        channel,
        &architect(),
        "🔍",
        &format!("Analyzing spec for: {project_name}"),
    )
    .await?;

    // Create workspace
    let workspace = Workspace::create(workspace_base, &project_name).await?;

    // Store the spec
    memory.set(&project_name, "spec", "current", spec)?;
    memory.log(&project_name, "event", "Build started")?;

    // Run the agentic loop — LLM with tools
    let tools = tools::code_tools();
    let mut messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(format!(
            "Build a working prototype for this spec and deploy it:\n\n{spec}"
        )),
    }];

    let mut deployed_url: Option<String> = None;
    let mut iteration = 0;
    const MAX_ITERATIONS: usize = 20;

    loop {
        iteration += 1;
        if iteration > MAX_ITERATIONS {
            output::error(
                handle,
                channel,
                &builder(),
                "Max iterations reached, stopping",
            )
            .await?;
            break;
        }

        let resp = llm.chat(SYSTEM_PROMPT, &messages, &tools, 4096).await?;

        // Collect text and tool uses from response
        let mut text_parts = Vec::new();
        let mut tool_uses = Vec::new();

        for block in &resp.content {
            match block {
                ContentBlock::Text { text } => {
                    text_parts.push(text.clone());
                }
                ContentBlock::ToolUse(tu) => {
                    tool_uses.push(tu.clone());
                }
                _ => {}
            }
        }

        // Post any commentary to channel
        let commentary = text_parts.join("").trim().to_string();
        if !commentary.is_empty() {
            // Keep channel messages concise — just first ~200 chars of commentary
            let short = if commentary.len() > 300 {
                format!("{}...", &commentary[..297])
            } else {
                commentary.clone()
            };
            output::say(handle, channel, &builder(), &short).await?;
        }

        // If no tool uses, we're done
        if tool_uses.is_empty() {
            break;
        }

        // Add assistant message to conversation
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

        // Execute each tool and collect results
        let mut result_blocks = Vec::new();

        for tu in &tool_uses {
            // Post tool activity to channel
            match tu.name.as_str() {
                "write_file" => {
                    let path = tu.input["path"].as_str().unwrap_or("?");
                    output::status(
                        handle,
                        channel,
                        &builder(),
                        "✏️",
                        &format!("Writing {path}"),
                    )
                    .await?;
                }
                "shell" => {
                    let cmd = tu.input["command"].as_str().unwrap_or("?");
                    let short_cmd = if cmd.len() > 80 {
                        format!("{}...", &cmd[..77])
                    } else {
                        cmd.to_string()
                    };
                    output::status(
                        handle,
                        channel,
                        &builder(),
                        "⚙️",
                        &format!("Running: {short_cmd}"),
                    )
                    .await?;
                }
                "deploy" => {
                    output::status(handle, channel, &deployer(), "🚀", "Deploying to miren...")
                        .await?;
                }
                "list_files" => {
                    output::status(handle, channel, &builder(), "📁", "Listing files").await?;
                }
                _ => {}
            }

            let result = match tools::execute_tool(&workspace, &tu.name, &tu.input).await {
                Ok(output) => {
                    // Check for deploy URL in output
                    if tu.name == "deploy"
                        && let Some(url) = extract_deploy_url(&output)
                    {
                        deployed_url = Some(url.clone());
                        output::deploy_result(handle, channel, &deployer(), &url).await?;
                        memory.set(&project_name, "deploy", "url", &url)?;
                    }

                    // Store files in memory
                    if tu.name == "write_file"
                        && let (Some(path), Some(content)) =
                            (tu.input["path"].as_str(), tu.input["content"].as_str())
                    {
                        memory.set(&project_name, "file", path, content)?;
                    }

                    output
                }
                Err(e) => {
                    let err = format!("Error: {e}");
                    output::error(
                        handle,
                        channel,
                        &builder(),
                        &format!("Tool {} failed: {e}", tu.name),
                    )
                    .await?;
                    err
                }
            };

            result_blocks.push(ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: tu.id.clone(),
                content: result,
                is_error: None,
            }));
        }

        // Add tool results to conversation
        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(result_blocks),
        });
    }

    // Final summary
    if let Some(ref url) = deployed_url {
        output::status(
            handle,
            channel,
            &deployer(),
            "✅",
            &format!("Prototype ready: {url}"),
        )
        .await?;
    } else {
        output::status(
            handle,
            channel,
            &builder(),
            "⚠️",
            "Build complete but no deploy URL found",
        )
        .await?;
    }

    memory.log(&project_name, "event", "Build complete")?;
    Ok(deployed_url)
}

/// Public wrapper for project name generation.
pub async fn generate_project_name_pub(llm: &LlmClient, spec: &str) -> Result<String> {
    generate_project_name(llm, spec).await
}

/// Generate a short project name from a spec.
async fn generate_project_name(llm: &LlmClient, spec: &str) -> Result<String> {
    let name = llm
        .complete(
            "You generate short, lowercase, hyphenated project names. Respond with ONLY the name, nothing else. Max 20 chars.",
            &format!("Generate a project name for:\n{spec}"),
        )
        .await?;
    let clean: String = name
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();
    let clean = if clean.is_empty() {
        format!("proto-{}", chrono::Utc::now().timestamp() % 10000)
    } else if clean.len() > 20 {
        clean[..20].to_string()
    } else {
        clean
    };
    Ok(clean)
}

/// Extract a deployed URL from miren deploy output.
fn extract_deploy_url(output: &str) -> Option<String> {
    // miren outputs: "Your app is available at:\n  https://..."
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("https://") {
            return Some(trimmed.to_string());
        }
    }
    None
}
