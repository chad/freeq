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

/// Post a message to a channel with agent role prefix. The SDK
/// auto-routes `\n`-bearing text through a `draft/multiline` BATCH
/// when the cap is acked, so a long agent response arrives as ONE
/// logical message (msgid coherence for edits/reactions) instead of
/// the N per-line PRIVMSGs the old wrap-and-sleep workaround produced.
pub async fn say(
    handle: &ClientHandle,
    channel: &str,
    agent: &AgentId,
    text: &str,
) -> anyhow::Result<()> {
    let msg = format!("[{}] {}", agent.role, text);
    handle.privmsg(channel, &msg).await
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

/// Post a code block (multi-line, formatted for readability). Sends
/// a status header PRIVMSG, then ONE multi-line PRIVMSG carrying the
/// indented body — the SDK BATCH-routes the body so it stays one
/// logical message rather than N flood-rate-limited per-line PRIVMSGs.
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

    let mut body = lines[..show_lines]
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    if truncated {
        body.push_str(&format!("\n  ... ({} more lines)", lines.len() - max_lines));
    }
    handle.privmsg(channel, &body).await
}

/// Post a file listing — status header + one multi-line body PRIVMSG.
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

    let mut body = files
        .iter()
        .take(20)
        .map(|f| format!("  {f}"))
        .collect::<Vec<_>>()
        .join("\n");
    if files.len() > 20 {
        body.push_str(&format!("\n  ... and {} more", files.len() - 20));
    }
    handle.privmsg(channel, &body).await
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

