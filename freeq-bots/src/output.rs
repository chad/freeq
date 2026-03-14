//! Structured output formatting for IRC channels.
//!
//! Agents produce structured artifacts (code diffs, diagrams, status updates).
//! This module formats them for readable IRC output.

use freeq_sdk::client::ClientHandle;
use freeq_sdk::streaming::StreamingMessage;
use tokio::sync::mpsc;
use crate::llm::StreamDelta;

/// An agent identity for channel messages.
#[derive(Debug, Clone)]
pub struct AgentId {
    /// Display name shown in messages, e.g. "architect", "builder"
    pub role: String,
    /// IRC color code (optional).
    pub color: Option<u8>,
}

/// Post a message to a channel with agent role prefix.
pub async fn say(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    text: &str,
) -> anyhow::Result<()> {
    // Split long messages across multiple IRC lines (max ~400 chars for safety)
    for line in wrap_lines(text, 400) {
        let msg = format!("[{}] {}", agent.role, line);
        handle.privmsg(channel, &msg).await?;
        // Small delay between multi-line messages to avoid flood
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Post a status update (brief, one-line).
pub async fn status(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    emoji: &str,
    text: &str,
) -> anyhow::Result<()> {
    let msg = format!("[{}] {} {}", agent.role, emoji, text);
    handle.privmsg(channel, &msg).await
}

/// Post a code block (multi-line, formatted for readability).
pub async fn code(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    filename: &str,
    content: &str,
    max_lines: usize,
) -> anyhow::Result<()> {
    let lines: Vec<&str> = content.lines().collect();
    let truncated = lines.len() > max_lines;
    let show_lines = if truncated { max_lines } else { lines.len() };

    status(
        handle,
        channel,
        agent,
        "📄",
        &format!("{filename} ({} lines)", lines.len()),
    )
    .await?;

    for line in &lines[..show_lines] {
        handle.privmsg(channel, &format!("  {line}")).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    if truncated {
        handle
            .privmsg(
                channel,
                &format!("  ... ({} more lines)", lines.len() - max_lines),
            )
            .await?;
    }

    Ok(())
}

/// Post a file listing.
pub async fn file_tree(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    files: &[String],
) -> anyhow::Result<()> {
    status(
        handle,
        channel,
        agent,
        "📁",
        &format!("Project files ({})", files.len()),
    )
    .await?;
    for f in files.iter().take(20) {
        handle.privmsg(channel, &format!("  {f}")).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if files.len() > 20 {
        handle
            .privmsg(channel, &format!("  ... and {} more", files.len() - 20))
            .await?;
    }
    Ok(())
}

/// Post a deploy result with the URL highlighted.
pub async fn deploy_result(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    url: &str,
) -> anyhow::Result<()> {
    status(handle, channel, agent, "🚀", &format!("Deployed → {url}")).await
}

/// Post an error.
pub async fn error(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    text: &str,
) -> anyhow::Result<()> {
    status(handle, channel, agent, "❌", text).await
}

/// Stream an LLM response to a channel, updating a single message in real-time.
///
/// Uses the IRC edit-message hack: sends an initial message, then repeatedly
/// edits it as tokens arrive from the LLM stream. Clients that support
/// `+draft/edit` see the message update in place.
///
/// Returns the final message text and msgid.
pub async fn stream_response(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    mut deltas: mpsc::Receiver<StreamDelta>,
) -> anyhow::Result<(String, String)> {
    let prefix = format!("[{}] ", agent.role);

    // Start a streaming message with a thinking cursor
    let mut stream = StreamingMessage::start(handle, channel).await?;

    let mut full_text = String::new();
    while let Some(delta) = deltas.recv().await {
        match delta {
            StreamDelta::Text(chunk) => {
                full_text.push_str(&chunk);
                // Set the full content with prefix each time
                stream.set(&format!("{prefix}{full_text}")).await?;
            }
            StreamDelta::Done => break,
            StreamDelta::Error(e) => {
                let error_text = format!("{prefix}❌ Stream error: {e}");
                stream.finish_with(&error_text).await?;
                anyhow::bail!("LLM stream error: {e}");
            }
        }
    }

    // Flush any remaining content and finish
    let final_text = format!("{prefix}{full_text}");
    let msgid = stream.finish_with(&final_text).await?;
    Ok((full_text, msgid))
}

/// Wrap text into lines of max_len, breaking on word boundaries.
fn wrap_lines(text: &str, max_len: usize) -> Vec<String> {
    let mut result = Vec::new();
    for line in text.lines() {
        if line.len() <= max_len {
            result.push(line.to_string());
        } else {
            let mut current = String::new();
            for word in line.split_whitespace() {
                if current.len() + word.len() + 1 > max_len {
                    if !current.is_empty() {
                        result.push(current);
                    }
                    current = word.to_string();
                } else {
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    current.push_str(word);
                }
            }
            if !current.is_empty() {
                result.push(current);
            }
        }
    }
    result
}
