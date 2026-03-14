//! Streaming message support via the IRC edit-message hack.
//!
//! Enables real-time token-by-token message streaming (e.g., from an LLM)
//! by sending an initial message and then repeatedly editing it as content
//! arrives. Clients that support `+draft/edit` see the message update in
//! place; others see each edit as a new message.
//!
//! # Usage
//!
//! ```no_run
//! use freeq_sdk::streaming::StreamingMessage;
//!
//! # async fn example(handle: freeq_sdk::client::ClientHandle) {
//! let mut stream = StreamingMessage::start(&handle, "#channel").await.unwrap();
//! stream.append("Hello ").await.unwrap();
//! stream.append("world!").await.unwrap();
//! stream.finish().await.unwrap();
//! // Final message text: "Hello world!"
//! # }
//! ```

use anyhow::Result;
use std::time::{Duration, Instant};

use crate::client::ClientHandle;

/// A message being streamed via repeated edits.
///
/// Created by [`StreamingMessage::start`], which sends an initial placeholder
/// message and captures its server-assigned `msgid` via `echo-message`.
///
/// Call [`append`](StreamingMessage::append) to add text, and
/// [`finish`](StreamingMessage::finish) when the stream is complete.
pub struct StreamingMessage {
    handle: ClientHandle,
    target: String,
    msgid: String,
    content: String,
    /// Cursor character appended during streaming (e.g., "▍").
    cursor: String,
    /// Minimum interval between edits to avoid flooding.
    throttle: Duration,
    last_edit: Instant,
    /// Whether we've accumulated changes since the last edit.
    dirty: bool,
}

impl StreamingMessage {
    /// Send an initial placeholder message and capture the server-assigned msgid.
    ///
    /// Uses `send_and_await_echo` which requires the `echo-message` IRCv3 cap.
    /// Does **not** require access to the events receiver.
    pub async fn start(handle: &ClientHandle, target: &str) -> Result<Self> {
        Self::start_with_options(handle, target, "▍", Duration::from_millis(300)).await
    }

    /// Like [`start`](Self::start) but with custom cursor and throttle.
    pub async fn start_with_options(
        handle: &ClientHandle,
        target: &str,
        cursor: &str,
        throttle: Duration,
    ) -> Result<Self> {
        // Send initial message with streaming tag, get back the msgid
        let mut tags = std::collections::HashMap::new();
        tags.insert("+freeq.at/streaming".to_string(), "1".to_string());
        let initial_text = cursor;
        let msgid = handle.send_and_await_echo(target, initial_text, tags).await?;

        Ok(Self {
            handle: handle.clone(),
            target: target.to_string(),
            msgid,
            content: String::new(),
            cursor: cursor.to_string(),
            throttle,
            last_edit: Instant::now(),
            dirty: false,
        })
    }

    /// Append text to the streaming message.
    ///
    /// Sends an edit if enough time has passed since the last one (throttled
    /// to avoid IRC flood). The cursor character is appended to show the
    /// message is still streaming.
    pub async fn append(&mut self, text: &str) -> Result<()> {
        self.content.push_str(text);
        self.dirty = true;

        // Throttle edits
        if self.last_edit.elapsed() >= self.throttle {
            self.flush().await?;
        }
        Ok(())
    }

    /// Force-send an edit with the current content (ignoring throttle).
    pub async fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let display = format!("{}{}", self.content, self.cursor);
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/edit".to_string(), self.msgid.clone());
        tags.insert("+freeq.at/streaming".to_string(), "1".to_string());
        self.handle.send_tagged(&self.target, &display, tags).await?;
        self.last_edit = Instant::now();
        self.dirty = false;
        Ok(())
    }

    /// Set the full content (replacing everything).
    pub async fn set(&mut self, text: &str) -> Result<()> {
        self.content = text.to_string();
        self.dirty = true;
        if self.last_edit.elapsed() >= self.throttle {
            self.flush().await?;
        }
        Ok(())
    }

    /// Finish the stream: send a final edit without the cursor or streaming tag.
    pub async fn finish(self) -> Result<String> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/edit".to_string(), self.msgid.clone());
        // No +freeq.at/streaming tag — signals completion
        self.handle.send_tagged(&self.target, &self.content, tags).await?;
        Ok(self.msgid)
    }

    /// Finish with custom final text.
    pub async fn finish_with(mut self, final_text: &str) -> Result<String> {
        self.content = final_text.to_string();
        self.finish().await
    }

    /// Cancel the stream and delete the message.
    pub async fn cancel(self) -> Result<()> {
        self.handle.delete_message(&self.target, &self.msgid).await
    }

    /// Get the current accumulated content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the message ID being edited.
    pub fn msgid(&self) -> &str {
        &self.msgid
    }
}
