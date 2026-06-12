//! IRC client with ATPROTO-CHALLENGE SASL support.
//!
//! This is the main entry point for SDK consumers. It manages the TCP
//! connection, IRC registration, CAP/SASL negotiation, and emits events.
//! Supports both plaintext and TLS connections.
//!
//! ## SASL Authentication
//!
//! Two SASL methods are supported:
//!
//! - **`web-token`**: A short-lived token minted by the auth broker after OAuth.
//!   Set `config.sasl_token` and `config.sasl_method = "web-token"`. The token is
//!   sent as the SASL payload and verified against the server's in-memory token map.
//!   Tokens expire after 5 minutes. Best for web and mobile clients that go through
//!   the OAuth broker flow.
//!
//! - **`crypto`**: Direct cryptographic challenge-response using the user's AT Protocol
//!   signing key. Set `config.sasl_method = "crypto"` and provide a DID + signing key.
//!   The server sends a challenge; the client signs it; the server verifies against the
//!   DID document. Best for bots and CLI tools with direct key access.
//!
//! ## Reconnection
//!
//! The SDK does not implement automatic reconnection. Consumers should implement
//! their own reconnect logic with exponential backoff (e.g., 2→4→8→16→30s cap)
//! to avoid overwhelming the server. Listen for [`Event::Disconnected`] and retry.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use base64::Engine;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls;

use crate::auth::{self, ChallengeSigner};
use crate::event::Event;
use crate::irc::Message;

/// Registry for pending echo-message callbacks.
/// When a client sends a PRIVMSG with a `+freeq.at/echo-nonce` tag, the nonce
/// is registered here. When the echo comes back, the msgid is sent via the oneshot.
type EchoRegistry = std::sync::Arc<parking_lot::Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>;

/// Configuration for connecting to an IRC server.
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    /// Server address (host:port).
    pub server_addr: String,
    /// Desired nickname.
    pub nick: String,
    /// Username (ident).
    pub user: String,
    /// Real name.
    pub realname: String,
    /// Use TLS.
    pub tls: bool,
    /// Skip TLS certificate verification (for self-signed certs).
    pub tls_insecure: bool,
    /// One-time web-token for SASL WEB-TOKEN authentication (from OAuth flow).
    pub web_token: Option<String>,
    /// WebSocket URL — `wss://host/path` or `ws://host/path`. When set, the
    /// SDK connects via WebSocket instead of raw TCP. Mirrors the JS
    /// client's transport (`freeq-sdk-js/src/transport.ts`) so iOS can
    /// reach the server on networks that block port 6667.
    pub websocket_url: Option<String>,
}

impl Default for ConnectConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:6667".to_string(),
            nick: "user".to_string(),
            user: "user".to_string(),
            realname: "IRC AT SDK User".to_string(),
            tls: false,
            tls_insecure: false,
            web_token: None,
            websocket_url: None,
        }
    }
}

impl ConnectConfig {
    /// Validate configuration fields. Returns an error describing the first invalid field.
    pub fn validate(&self) -> Result<(), String> {
        if self.server_addr.is_empty() {
            return Err("server_addr must not be empty".into());
        }
        if self.nick.is_empty() || self.nick.len() > 64 {
            return Err("nick must be 1-64 characters".into());
        }
        if self.nick.contains(|c: char| c.is_control() || c == ' ' || c == ',' || c == '*' || c == '?' || c == '!' || c == '@' || c == '#') {
            return Err("nick contains invalid characters".into());
        }
        if self.user.is_empty() {
            return Err("user must not be empty".into());
        }
        if self.tls_insecure && !self.tls {
            tracing::warn!("tls_insecure has no effect when tls is false");
        }
        Ok(())
    }
}

/// Commands the consumer can send to the client.
#[derive(Debug)]
pub enum Command {
    Join(String),
    Privmsg { target: String, text: String },
    /// Send a `draft/multiline` BATCH. Used when the assembled body
    /// either contains `\n` (one chunk per logical line, concat=false)
    /// or exceeds a single line and needs ciphertext-style chunking
    /// (concat=true on every chunk after the first). Opener tags ride
    /// on the BATCH opener; the SDK reuses the consumer-supplied chunks
    /// verbatim — no internal line-splitting — so the wire shape is
    /// fully controlled by the caller.
    SendMultiline {
        target: String,
        chunks: Vec<MultilineChunk>,
        opener_tags: std::collections::HashMap<String, String>,
    },
    Raw(String),
    Quit(Option<String>),
}

/// One chunk of a `draft/multiline` send. `concat=true` translates to
/// `+draft/multiline-concat` on the wire — receivers join with no
/// separator. The first chunk's `concat` is conventionally `false`.
#[derive(Debug, Clone)]
pub struct MultilineChunk {
    pub body: String,
    pub concat: bool,
}

/// State for one in-flight inbound `draft/multiline` batch — the
/// opener-derived metadata plus the accumulated chunks. Cleared when
/// the BATCH closer fires.
#[derive(Debug)]
struct InboundMultilineBatch {
    target: String,
    from: String,
    opener_tags: std::collections::HashMap<String, String>,
    lines: Vec<MultilineChunk>,
    parent_batch_id: Option<String>,
}

/// Caps the server has ACKed, shared between the read loop (which
/// populates it on CAP ACK) and the `ClientHandle` (which reads it
/// to decide e.g. whether a `\n`-bearing `privmsg` should auto-route
/// to a `draft/multiline` BATCH).
pub(crate) type CapsAcked = Arc<parking_lot::Mutex<HashSet<String>>>;

/// A handle to a running IRC client connection.
#[derive(Clone)]
pub struct ClientHandle {
    cmd_tx: mpsc::Sender<Command>,
    echo_registry: EchoRegistry,
    caps_acked: CapsAcked,
}

impl ClientHandle {
    pub async fn join(&self, channel: &str) -> Result<()> {
        self.cmd_tx.send(Command::Join(channel.to_string())).await?;
        Ok(())
    }

    /// Send a PRIVMSG. If `text` contains `\n` and the server acked
    /// `draft/multiline` + `batch`, the SDK auto-routes the send to a
    /// `draft/multiline` BATCH (one chunk per source line) so the
    /// wire stays valid. Single-line text goes out as one PRIVMSG
    /// unchanged.
    ///
    /// `\n`-bearing text against a server that didn't ack the cap
    /// still goes out as a single (malformed) PRIVMSG — callers
    /// targeting old servers should pre-encode or call
    /// `send_multiline_chunks` with explicit chunks.
    pub async fn privmsg(&self, target: &str, text: &str) -> Result<()> {
        let multiline_ready = text.contains('\n') && {
            let caps = self.caps_acked.lock();
            caps.contains("draft/multiline") && caps.contains("batch")
        };
        if multiline_ready {
            let chunks: Vec<MultilineChunk> = text
                .split('\n')
                .map(|line| MultilineChunk {
                    body: line.to_string(),
                    concat: false,
                })
                .collect();
            self.cmd_tx
                .send(Command::SendMultiline {
                    target: target.to_string(),
                    chunks,
                    opener_tags: std::collections::HashMap::new(),
                })
                .await?;
        } else {
            self.cmd_tx
                .send(Command::Privmsg {
                    target: target.to_string(),
                    text: text.to_string(),
                })
                .await?;
        }
        Ok(())
    }

    /// Send a multi-line message via `draft/multiline` BATCH. Splits
    /// `text` on `\n` boundaries by default — each logical line becomes
    /// one wire chunk with `concat=false` so receivers reassemble with
    /// `\n` separators. Pass `opener_tags` for client-only tags that
    /// should ride on the BATCH opener (e.g. commit-reveal payloads).
    ///
    /// For ciphertext-style chunking (one assembled blob split into
    /// fixed-size pieces with concat=true), construct the `Vec<MultilineChunk>`
    /// directly and use `send_multiline_chunks`.
    pub async fn send_multiline(
        &self,
        target: &str,
        text: &str,
        opener_tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let chunks: Vec<MultilineChunk> = text
            .split('\n')
            .map(|line| MultilineChunk {
                body: line.to_string(),
                concat: false,
            })
            .collect();
        self.send_multiline_chunks(target, chunks, opener_tags).await
    }

    /// Lower-level multiline send: caller supplies the wire chunks
    /// directly. Use this when you need control over `concat` flags
    /// (e.g. splitting a single-line ciphertext into byte-sized pieces).
    pub async fn send_multiline_chunks(
        &self,
        target: &str,
        chunks: Vec<MultilineChunk>,
        opener_tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.cmd_tx
            .send(Command::SendMultiline {
                target: target.to_string(),
                chunks,
                opener_tags,
            })
            .await?;
        Ok(())
    }

    pub async fn quit(&self, message: Option<&str>) -> Result<()> {
        self.cmd_tx
            .send(Command::Quit(message.map(|s| s.to_string())))
            .await?;
        Ok(())
    }

    pub async fn raw(&self, line: &str) -> Result<()> {
        self.cmd_tx.send(Command::Raw(line.to_string())).await?;
        Ok(())
    }

    /// Send a tagged message and await the server-assigned msgid via echo-message.
    ///
    /// This inserts a unique nonce tag (`+freeq.at/echo-nonce`) that the client
    /// loop matches against incoming echo-messages. Requires `echo-message` cap.
    ///
    /// Returns the server-assigned `msgid`.
    pub async fn send_and_await_echo(
        &self,
        target: &str,
        text: &str,
        mut tags: std::collections::HashMap<String, String>,
    ) -> Result<String> {
        let nonce = format!("echo-{:016x}", rand::random::<u64>());
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.echo_registry.lock().insert(nonce.clone(), tx);
        tags.insert("+freeq.at/echo-nonce".to_string(), nonce.clone());
        self.send_tagged(target, text, tags).await?;
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(msgid)) => Ok(msgid),
            Ok(Err(_)) => anyhow::bail!("Echo channel dropped"),
            Err(_) => {
                self.echo_registry.lock().remove(&nonce);
                anyhow::bail!("Timed out waiting for echo-message msgid")
            }
        }
    }

    /// Send a message with IRCv3 tags (for rich media). If `text`
    /// contains `\n` and the server acked `draft/multiline` + `batch`,
    /// the SDK auto-routes the send to a `draft/multiline` BATCH with
    /// the given tags on the opener — same auto-routing behavior as
    /// `privmsg`. Otherwise emits a single tagged PRIVMSG.
    pub async fn send_tagged(
        &self,
        target: &str,
        text: &str,
        tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let multiline_ready = text.contains('\n') && {
            let caps = self.caps_acked.lock();
            caps.contains("draft/multiline") && caps.contains("batch")
        };
        if multiline_ready {
            let chunks: Vec<MultilineChunk> = text
                .split('\n')
                .map(|line| MultilineChunk {
                    body: line.to_string(),
                    concat: false,
                })
                .collect();
            self.cmd_tx
                .send(Command::SendMultiline {
                    target: target.to_string(),
                    chunks,
                    opener_tags: tags,
                })
                .await?;
        } else {
            let msg = crate::irc::Message {
                tags,
                prefix: None,
                command: "PRIVMSG".to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            self.cmd_tx.send(Command::Raw(msg.to_string())).await?;
        }
        Ok(())
    }

    /// Send a media attachment to a target (channel or user).
    pub async fn send_media(
        &self,
        target: &str,
        media: &crate::media::MediaAttachment,
    ) -> Result<()> {
        self.send_tagged(target, &media.fallback_text(), media.to_tags())
            .await
    }

    /// Send a TAGMSG (tags-only, no body) to a target.
    pub async fn send_tagmsg(
        &self,
        target: &str,
        tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let msg = crate::irc::Message {
            tags,
            prefix: None,
            command: "TAGMSG".to_string(),
            params: vec![target.to_string()],
        };
        self.cmd_tx.send(Command::Raw(msg.to_string())).await?;
        Ok(())
    }

    /// Start a new AV (voice/video) call in `channel`. The server
    /// replies with an `av-state` TAGMSG carrying the session id —
    /// watch for it with [`crate::av::parse_av_state`] applied to
    /// incoming [`Event::TagMsg`](crate::event::Event::TagMsg) tags.
    /// `instance` is a per-call id from [`crate::av::new_av_instance`].
    pub async fn av_start(
        &self,
        channel: &str,
        instance: &str,
        title: Option<&str>,
    ) -> Result<()> {
        self.send_tagmsg(channel, crate::av::av_start_tags(instance, title))
            .await
    }

    /// Join the active AV call in `channel`. `session_id` is the id from
    /// the channel's `av-state` broadcast.
    pub async fn av_join(
        &self,
        channel: &str,
        session_id: &str,
        instance: &str,
    ) -> Result<()> {
        self.send_tagmsg(channel, crate::av::av_join_tags(session_id, instance))
            .await
    }

    /// Leave an AV call.
    pub async fn av_leave(
        &self,
        channel: &str,
        session_id: &str,
        instance: &str,
    ) -> Result<()> {
        self.send_tagmsg(channel, crate::av::av_leave_tags(session_id, instance))
            .await
    }

    /// Send a reaction to a target (channel or user).
    /// Falls back to PRIVMSG for plain clients.
    pub async fn send_reaction(
        &self,
        target: &str,
        reaction: &crate::media::Reaction,
    ) -> Result<()> {
        self.send_tagmsg(target, reaction.to_tags()).await
    }

    /// Send a link preview as a tagged message.
    pub async fn send_link_preview(
        &self,
        target: &str,
        preview: &crate::media::LinkPreview,
    ) -> Result<()> {
        let fallback = match (&preview.title, &preview.description) {
            (Some(t), Some(d)) => format!("🔗 {} — {} ({})", t, d, preview.url),
            (Some(t), None) => format!("🔗 {} ({})", t, preview.url),
            _ => format!("🔗 {}", preview.url),
        };
        self.send_tagged(target, &fallback, preview.to_tags()).await
    }

    // ── Convenience helpers ──

    /// Send a reply to a specific message (adds +reply tag).
    pub async fn reply(&self, target: &str, msgid: &str, text: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+reply".to_string(), msgid.to_string());
        self.send_tagged(target, text, tags).await
    }

    /// Send a reply in a thread (same as reply — thread parent is the msgid).
    pub async fn reply_in_thread(
        &self,
        target: &str,
        parent_msgid: &str,
        text: &str,
    ) -> Result<()> {
        self.reply(target, parent_msgid, text).await
    }

    /// Send a typing indicator start.
    pub async fn typing_start(&self, target: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+typing".to_string(), "active".to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Send a typing indicator stop.
    pub async fn typing_stop(&self, target: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+typing".to_string(), "done".to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Join multiple channels at once.
    pub async fn join_many(&self, channels: &[&str]) -> Result<()> {
        if channels.is_empty() {
            return Ok(());
        }
        // IRC allows comma-separated JOIN
        let joined = channels.join(",");
        self.raw(&format!("JOIN {joined}")).await
    }

    /// Set a channel mode. Examples: `mode("#chan", "+o", Some("nick"))`.
    pub async fn mode(&self, channel: &str, flags: &str, arg: Option<&str>) -> Result<()> {
        match arg {
            Some(a) => self.raw(&format!("MODE {channel} {flags} {a}")).await,
            None => self.raw(&format!("MODE {channel} {flags}")).await,
        }
    }

    /// Request latest N messages of history (CHATHISTORY LATEST).
    pub async fn history_latest(&self, target: &str, count: usize) -> Result<()> {
        self.raw(&format!("CHATHISTORY LATEST {target} * {count}"))
            .await
    }

    /// Request N messages before a given msgid (CHATHISTORY BEFORE).
    pub async fn history_before(&self, target: &str, msgid: &str, count: usize) -> Result<()> {
        self.raw(&format!(
            "CHATHISTORY BEFORE {target} msgid={msgid} {count}"
        ))
        .await
    }

    /// Request N messages after a given msgid (CHATHISTORY AFTER).
    pub async fn history_after(&self, target: &str, msgid: &str, count: usize) -> Result<()> {
        self.raw(&format!("CHATHISTORY AFTER {target} msgid={msgid} {count}"))
            .await
    }

    /// Request DM conversation list (CHATHISTORY TARGETS).
    pub async fn chathistory_targets(&self, limit: usize) -> Result<()> {
        self.raw(&format!("CHATHISTORY TARGETS * * {limit}")).await
    }

    /// Send a reaction emoji to a specific message.
    pub async fn react(&self, target: &str, emoji: &str, msgid: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+react".to_string(), emoji.to_string());
        tags.insert("+reply".to_string(), msgid.to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Edit a previously sent message.
    pub async fn edit_message(
        &self,
        target: &str,
        original_msgid: &str,
        new_text: &str,
    ) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/edit".to_string(), original_msgid.to_string());
        self.send_tagged(target, new_text, tags).await
    }

    /// Delete a previously sent message (via TAGMSG).
    pub async fn delete_message(&self, target: &str, msgid: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/delete".to_string(), msgid.to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Pin a message in a channel.
    pub async fn pin(&self, channel: &str, msgid: &str) -> Result<()> {
        self.raw(&format!("PIN {channel} {msgid}")).await
    }

    /// Unpin a message in a channel.
    pub async fn unpin(&self, channel: &str, msgid: &str) -> Result<()> {
        self.raw(&format!("UNPIN {channel} {msgid}")).await
    }

    /// Set the channel topic.
    pub async fn topic(&self, channel: &str, topic: &str) -> Result<()> {
        self.raw(&format!("TOPIC {channel} :{topic}")).await
    }

    // ── Agent-native methods ─────────────────────────────────────────

    /// Register this connection as an agent (or external_agent).
    pub async fn register_agent(&self, class: &str) -> Result<()> {
        self.raw(&format!("AGENT REGISTER :class={class}")).await
    }

    /// Submit a provenance declaration (JSON value, will be base64url-encoded).
    pub async fn submit_provenance(&self, provenance: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_vec(provenance)?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&json);
        self.raw(&format!("PROVENANCE :{encoded}")).await
    }

    /// Update structured agent presence.
    pub async fn set_presence(
        &self,
        state: &str,
        status: Option<&str>,
        task: Option<&str>,
    ) -> Result<()> {
        let mut parts = vec![format!("state={state}")];
        if let Some(s) = status {
            parts.push(format!("status={s}"));
        }
        if let Some(t) = task {
            parts.push(format!("task={t}"));
        }
        self.raw(&format!("PRESENCE :{}", parts.join(";"))).await
    }

    /// Send a heartbeat with the given state and TTL (seconds).
    pub async fn send_heartbeat(&self, state: &str, ttl: u64) -> Result<()> {
        self.raw(&format!("HEARTBEAT :state={state};ttl={ttl}"))
            .await
    }

    /// Request approval for a capability in a channel.
    pub async fn request_approval(
        &self,
        channel: &str,
        capability: &str,
        resource: Option<&str>,
    ) -> Result<()> {
        let resource_part = resource.map(|r| format!(";resource={r}")).unwrap_or_default();
        self.raw(&format!("APPROVAL_REQUEST {channel} :{capability}{resource_part}"))
            .await
    }

    /// Pause an agent (must be op in shared channel).
    pub async fn pause_agent(&self, nick: &str, reason: Option<&str>) -> Result<()> {
        match reason {
            Some(r) => self.raw(&format!("AGENT PAUSE {nick} :{r}")).await,
            None => self.raw(&format!("AGENT PAUSE {nick}")).await,
        }
    }

    /// Resume an agent (must be op in shared channel).
    pub async fn resume_agent(&self, nick: &str) -> Result<()> {
        self.raw(&format!("AGENT RESUME {nick}")).await
    }

    /// Revoke an agent (must be op in shared channel).
    pub async fn revoke_agent(&self, nick: &str, reason: Option<&str>) -> Result<()> {
        match reason {
            Some(r) => self.raw(&format!("AGENT REVOKE {nick} :{r}")).await,
            None => self.raw(&format!("AGENT REVOKE {nick}")).await,
        }
    }

    /// Approve an agent's pending capability request.
    pub async fn approve_agent(&self, nick: &str, capability: &str) -> Result<()> {
        self.raw(&format!("AGENT APPROVE {nick} {capability}")).await
    }

    /// Deny an agent's pending capability request.
    pub async fn deny_agent(&self, nick: &str, capability: &str, reason: Option<&str>) -> Result<()> {
        match reason {
            Some(r) => self.raw(&format!("AGENT DENY {nick} {capability} :{r}")).await,
            None => self.raw(&format!("AGENT DENY {nick} {capability}")).await,
        }
    }

    // ── Phase 3: Coordination events ────────────────────────────────

    /// Emit a typed coordination event to a channel.
    /// Sends both a TAGMSG (structured, for rich clients) and PRIVMSG (human-readable).
    pub async fn emit_event(
        &self,
        channel: &str,
        event_type: &str,
        payload_json: &str,
        ref_id: Option<&str>,
        human_text: &str,
    ) -> Result<String> {
        let event_id = format!("{:016x}{:016x}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            rand::random::<u64>(),
        );
        let mut tags = format!(
            "+freeq.at/event={event_type};msgid={event_id};+freeq.at/payload={}",
            payload_json.replace(';', "%3B").replace(' ', "%20")
        );
        if let Some(rid) = ref_id {
            tags.push_str(&format!(";+freeq.at/ref={rid}"));
        }
        // Send structured TAGMSG
        self.raw(&format!("@{tags} TAGMSG {channel}")).await?;
        // Send human-readable PRIVMSG with the same event tags (for rich client rendering)
        self.raw(&format!("@{tags} PRIVMSG {channel} :{human_text}")).await?;
        Ok(event_id)
    }

    /// Create a new task and return its ID.
    pub async fn create_task(&self, channel: &str, description: &str) -> Result<String> {
        let payload = serde_json::json!({"description": description}).to_string();
        self.emit_event(
            channel,
            "task_request",
            &payload,
            None,
            &format!("📋 New task: {description}"),
        ).await
    }

    /// Update a task's status.
    pub async fn update_task(
        &self,
        channel: &str,
        task_id: &str,
        phase: &str,
        summary: &str,
    ) -> Result<()> {
        let payload = serde_json::json!({"phase": phase, "summary": summary}).to_string();
        self.emit_event(
            channel,
            "task_update",
            &payload,
            Some(task_id),
            &format!("🔄 [{phase}] {summary}"),
        ).await?;
        Ok(())
    }

    /// Complete a task.
    pub async fn complete_task(
        &self,
        channel: &str,
        task_id: &str,
        summary: &str,
        url: Option<&str>,
    ) -> Result<()> {
        let mut payload = serde_json::json!({"summary": summary});
        if let Some(u) = url {
            payload["url"] = serde_json::json!(u);
        }
        let url_str = url.map(|u| format!(" — {u}")).unwrap_or_default();
        self.emit_event(
            channel,
            "task_complete",
            &payload.to_string(),
            Some(task_id),
            &format!("🎉 Task complete: {summary}{url_str}"),
        ).await?;
        Ok(())
    }

    /// Fail a task.
    pub async fn fail_task(
        &self,
        channel: &str,
        task_id: &str,
        error: &str,
    ) -> Result<()> {
        let payload = serde_json::json!({"error": error}).to_string();
        self.emit_event(
            channel,
            "task_failed",
            &payload,
            Some(task_id),
            &format!("❌ Task failed: {error}"),
        ).await?;
        Ok(())
    }

    /// Attach evidence to a task.
    pub async fn attach_evidence(
        &self,
        channel: &str,
        task_id: &str,
        evidence_type: &str,
        summary: &str,
        url: Option<&str>,
    ) -> Result<()> {
        let mut payload = serde_json::json!({
            "type": evidence_type,
            "summary": summary,
        });
        if let Some(u) = url {
            payload["url"] = serde_json::json!(u);
        }
        let url_str = url.map(|u| format!(" — {u}")).unwrap_or_default();
        self.emit_event(
            channel,
            "evidence_attach",
            &payload.to_string(),
            Some(task_id),
            &format!("📎 Evidence ({evidence_type}): {summary}{url_str}"),
        ).await?;
        Ok(())
    }

    // ── Phase 4: Manifests and Spawning ────────────────────────────

    /// Submit an agent manifest (base64-encoded TOML).
    pub async fn submit_manifest(&self, toml_content: &str) -> Result<()> {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(toml_content.as_bytes());
        self.raw(&format!("AGENT MANIFEST {encoded}")).await
    }

    /// Spawn a child agent in a channel.
    pub async fn spawn_agent(
        &self,
        channel: &str,
        nick: &str,
        capabilities: &[&str],
        ttl_seconds: Option<u64>,
        task_ref: Option<&str>,
    ) -> Result<()> {
        let mut params = format!("nick={nick}");
        if !capabilities.is_empty() {
            params.push_str(&format!(";capabilities={}", capabilities.join(",")));
        }
        if let Some(ttl) = ttl_seconds {
            params.push_str(&format!(";ttl={ttl}"));
        }
        if let Some(task) = task_ref {
            params.push_str(&format!(";task={task}"));
        }
        self.raw(&format!("AGENT SPAWN {channel} :{params}")).await
    }

    /// Despawn a child agent.
    pub async fn despawn_agent(&self, nick: &str) -> Result<()> {
        self.raw(&format!("AGENT DESPAWN {nick}")).await
    }

    /// Send a message as a spawned child agent.
    pub async fn send_as_child(&self, child_nick: &str, channel: &str, text: &str) -> Result<()> {
        self.raw(&format!("AGENT MSG {child_nick} {channel} :{text}")).await
    }

    // ── Phase 5: Economic Controls ─────────────────────────────────

    /// Report spend for the current action.
    pub async fn report_spend(
        &self,
        channel: &str,
        amount: f64,
        unit: &str,
        description: &str,
        task_ref: Option<&str>,
    ) -> Result<()> {
        let mut params = format!("amount={amount:.6};unit={unit};desc={description}");
        if let Some(task) = task_ref {
            params.push_str(&format!(";task={task}"));
        }
        self.raw(&format!("SPEND {channel} :{params}")).await
    }

    /// Set a channel budget (must be channel op).
    pub async fn set_budget(
        &self,
        channel: &str,
        max_amount: f64,
        unit: &str,
        period: &str,
        sponsor_did: &str,
    ) -> Result<()> {
        self.raw(&format!(
            "BUDGET {channel} :max={max_amount};unit={unit};period={period};sponsor={sponsor_did}"
        )).await
    }

    /// Query channel budget status.
    pub async fn query_budget(&self, channel: &str) -> Result<()> {
        self.raw(&format!("BUDGET {channel}")).await
    }

    /// Start automatic heartbeat in a background task.
    /// Returns a handle that stops the heartbeat when dropped.
    pub fn start_heartbeat(
        &self,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let handle = self.clone();
        let ttl = interval.as_secs() * 2;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if handle.send_heartbeat("active", ttl).await.is_err() {
                    break; // Connection closed
                }
            }
        })
    }
}

/// Establish TCP (and optionally TLS) connection to the server.
///
/// This is done **before** the TUI starts so that connection errors
/// are visible on stderr. Returns the established connection for
/// `connect_with_stream` to use.
/// Cap any single transport-layer connect attempt at this duration. The
/// underlying OS TCP timeout on iOS/macOS is ~75s — that's the cliff that
/// produces the user-visible "Connecting…" hang on networks that drop the
/// SYN to port 6667 silently. 10s is long enough for any real network and
/// short enough to fall through to a WebSocket fallback in time.
pub const TRANSPORT_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub async fn establish_connection(config: &ConnectConfig) -> Result<EstablishedConnection> {
    config
        .validate()
        .map_err(|e| anyhow::anyhow!("invalid ConnectConfig: {e}"))?;

    // If a WebSocket URL is configured, prefer that transport. iOS sets this
    // so it can reach the server on networks that block port 6667.
    #[cfg(feature = "websocket")]
    if let Some(ref ws_url) = config.websocket_url {
        return establish_ws_connection(ws_url).await;
    }

    // Auto-detect TLS from port if not explicitly set
    let use_tls = config.tls || config.server_addr.ends_with(":6697");
    let mode = if use_tls { "TLS" } else { "plain" };

    tracing::debug!("Resolving {}...", config.server_addr);
    let tcp = match tokio::time::timeout(
        TRANSPORT_CONNECT_TIMEOUT,
        TcpStream::connect(&config.server_addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err(anyhow::anyhow!(
                "TCP connect to {} failed: {e}",
                config.server_addr
            ))
        }
        Err(_) => {
            return Err(anyhow::anyhow!(
                "TCP connect to {} timed out after {}s",
                config.server_addr,
                TRANSPORT_CONNECT_TIMEOUT.as_secs()
            ))
        }
    };
    tracing::debug!("TCP connected to {} ({mode})", config.server_addr);

    if use_tls {
        let tls_config = if config.tls_insecure {
            tracing::debug!("TLS: insecure mode (skipping cert verification)");
            rustls_insecure_config()
        } else {
            tracing::debug!("TLS: verifying server certificate...");
            rustls_default_config()
        };
        let connector = TlsConnector::from(Arc::new(tls_config));
        let server_name = config.server_addr.split(':').next().unwrap_or("localhost");
        let dns_name = rustls::pki_types::ServerName::try_from(server_name.to_string())?;
        let tls_stream = connector.connect(dns_name, tcp).await.map_err(|e| {
            let hint = if format!("{e}").contains("UnknownIssuer") {
                " (the server's certificate chain may be incomplete — try --tls-insecure to skip verification, or ensure the server sends its full certificate chain including intermediates)"
            } else {
                ""
            };
            anyhow::anyhow!("TLS handshake with {} failed: {e}{hint}", config.server_addr)
        })?;
        tracing::debug!("TLS handshake complete");
        Ok(EstablishedConnection::Tls(tls_stream))
    } else {
        Ok(EstablishedConnection::Plain(tcp))
    }
}

/// A connection that has completed TCP (and optionally TLS) but hasn't
/// started IRC registration yet.
pub enum EstablishedConnection {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
    /// Iroh QUIC connection (already encrypted, NAT-traversing).
    #[cfg(feature = "iroh-transport")]
    Iroh(tokio::io::DuplexStream),
    /// WebSocket connection (encrypted via TLS for `wss://`). The client
    /// speaks raw IRC line bytes; the bridge tasks frame them as
    /// WebSocket text messages and unframe inbound messages identically.
    /// Reuses the same `DuplexStream` plumbing as Iroh so `run_irc()`
    /// doesn't need to know which transport it's running over.
    #[cfg(feature = "websocket")]
    WebSocket(tokio::io::DuplexStream),
}

/// ALPN for IRC-over-iroh (must match server).
#[cfg(feature = "iroh-transport")]
pub const IROH_ALPN: &[u8] = b"freeq/iroh/1";

#[cfg(feature = "iroh-transport")]
/// Establish a connection to an IRC server via iroh.
///
/// `addr` is the iroh endpoint address string (EndpointAddr format).
pub async fn establish_iroh_connection(addr: &str) -> Result<EstablishedConnection> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tracing::debug!("Creating iroh endpoint...");
    let endpoint = iroh::Endpoint::bind(iroh::endpoint::presets::N0).await?;

    tracing::debug!("Connecting to iroh peer {addr}...");
    // Parse the endpoint ID (public key) and create an address.
    // Iroh's relay/discovery system handles finding the actual network path.
    let endpoint_id: iroh::EndpointId = addr
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid iroh endpoint ID '{addr}': {e}"))?;
    let endpoint_addr = iroh::EndpointAddr::new(endpoint_id);
    let conn = endpoint.connect(endpoint_addr, IROH_ALPN).await?;
    tracing::debug!("Iroh QUIC connection established (encrypted)");

    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open bidirectional stream: {e}"))?;
    tracing::debug!("Bidirectional stream open, ready for IRC");

    // Bridge QUIC send/recv to a DuplexStream that the IRC handler can use.
    // irc_side goes to the IRC protocol handler.
    // bridge_side is shuttled to/from QUIC by two background tasks.
    let (irc_side, bridge_side) = tokio::io::duplex(16384);
    let (mut bridge_read, mut bridge_write) = tokio::io::split(bridge_side);

    // QUIC recv → bridge_write → IRC handler reads from irc_side
    tokio::spawn(async move {
        let mut recv = recv;
        let mut buf = vec![0u8; 4096];
        while let Ok(Some(n)) = recv.read(&mut buf).await {
            if bridge_write.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
        let _ = bridge_write.shutdown().await;
    });

    // IRC handler writes to irc_side → bridge_read → QUIC send
    tokio::spawn(async move {
        let mut send = send;
        let mut buf = vec![0u8; 4096];
        loop {
            match bridge_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if send.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = send.finish();
    });

    // Keep endpoint + connection alive for the lifetime of the session
    tokio::spawn(async move {
        let _endpoint = endpoint;
        let _conn = conn;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    Ok(EstablishedConnection::Iroh(irc_side))
}

/// Probe an IRC server for iroh endpoint ID via CAP LS.
///
/// Connects via TCP (or TLS for port 6697), sends CAP LS, reads the response,
/// extracts `iroh=<endpoint-id>` if present, and disconnects cleanly.
/// Returns `None` if the server doesn't advertise iroh.
///
/// This enables automatic iroh transport upgrade: connect cheap (TCP),
/// discover capabilities, reconnect optimal (iroh QUIC).
#[cfg(feature = "iroh-transport")]
pub async fn discover_iroh_id(server_addr: &str, tls: bool, tls_insecure: bool) -> Option<String> {
    use std::time::Duration;
    use tokio::time::timeout;

    let use_tls = tls || server_addr.ends_with(":6697");

    // Give the probe 5 seconds max
    let result = timeout(Duration::from_secs(5), async {
        let tcp = TcpStream::connect(server_addr).await.ok()?;

        if use_tls {
            let tls_config = if tls_insecure {
                rustls_insecure_config()
            } else {
                rustls_default_config()
            };
            let connector = TlsConnector::from(Arc::new(tls_config));
            let host = server_addr.split(':').next().unwrap_or("localhost");
            let dns_name = rustls::pki_types::ServerName::try_from(host.to_string()).ok()?;
            let tls_stream = connector.connect(dns_name, tcp).await.ok()?;
            probe_cap_ls(tls_stream).await
        } else {
            probe_cap_ls(tcp).await
        }
    })
    .await;

    result.ok().flatten()
}

/// Send CAP LS and parse iroh endpoint ID from response.
async fn probe_cap_ls<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: S,
) -> Option<String> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Send CAP LS and a throwaway NICK/USER so the server doesn't time us out
    writer
        .write_all(b"CAP LS 302\r\nNICK _probe\r\nUSER _probe 0 * :probe\r\n")
        .await
        .ok()?;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.ok()?;
        if n == 0 {
            return None;
        }

        // Look for CAP * LS :...
        if line.contains("CAP") && line.contains("LS") {
            // Find iroh=<id> in the caps string
            for token in line.split_whitespace() {
                if let Some(id) = token.strip_prefix("iroh=") {
                    // Clean up: send QUIT
                    let _ = writer.write_all(b"QUIT\r\n").await;
                    let _ = writer.shutdown().await;
                    return Some(id.trim().to_string());
                }
            }
            // Server responded to CAP LS but no iroh — done
            let _ = writer.write_all(b"QUIT\r\n").await;
            let _ = writer.shutdown().await;
            return None;
        }
    }
}

/// Connect using an already-established connection.
///
/// Returns a handle for sending commands and a receiver for events.
/// The IRC protocol runs in a spawned task.
pub fn connect_with_stream(
    conn: EstablishedConnection,
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
) -> (ClientHandle, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(4096);
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let echo_registry: EchoRegistry = std::sync::Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let caps_acked: CapsAcked = Arc::new(parking_lot::Mutex::new(HashSet::new()));

    let handle = ClientHandle {
        cmd_tx: cmd_tx.clone(),
        echo_registry: echo_registry.clone(),
        caps_acked: caps_acked.clone(),
    };

    let echo_reg = echo_registry.clone();
    let caps_for_loop = caps_acked.clone();
    tokio::spawn(async move {
        let _ = event_tx.send(Event::Connected).await;
        let result = match conn {
            EstablishedConnection::Plain(tcp) => {
                let (reader, writer) = tokio::io::split(tcp);
                run_irc(
                    BufReader::new(reader),
                    writer,
                    &config,
                    signer,
                    event_tx.clone(),
                    cmd_rx,
                    echo_reg,
                    caps_for_loop,
                )
                .await
            }
            EstablishedConnection::Tls(tls) => {
                let (reader, writer) = tokio::io::split(tls);
                run_irc(
                    BufReader::new(reader),
                    writer,
                    &config,
                    signer,
                    event_tx.clone(),
                    cmd_rx,
                    echo_reg,
                    caps_for_loop,
                )
                .await
            }
            #[cfg(feature = "iroh-transport")]
            EstablishedConnection::Iroh(duplex) => {
                let (reader, writer) = tokio::io::split(duplex);
                run_irc(
                    BufReader::new(reader),
                    writer,
                    &config,
                    signer,
                    event_tx.clone(),
                    cmd_rx,
                    echo_reg,
                    caps_for_loop,
                )
                .await
            }
            #[cfg(feature = "websocket")]
            EstablishedConnection::WebSocket(duplex) => {
                let (reader, writer) = tokio::io::split(duplex);
                run_irc(
                    BufReader::new(reader),
                    writer,
                    &config,
                    signer,
                    event_tx.clone(),
                    cmd_rx,
                    echo_reg,
                    caps_for_loop,
                )
                .await
            }
        };
        if let Err(e) = result {
            let _ = event_tx
                .send(Event::Disconnected {
                    reason: e.to_string(),
                })
                .await;
        }
    });

    (handle, event_rx)
}

/// Connect to an IRC server and run the client.
///
/// Returns a handle for sending commands and a receiver for events.
/// The connection runs in a spawned task.
///
/// Note: prefer `establish_connection` + `connect_with_stream` for better
/// error reporting (connection errors happen before the TUI starts).
pub fn connect(
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
) -> (ClientHandle, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(4096);
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let echo_registry: EchoRegistry = std::sync::Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let caps_acked: CapsAcked = Arc::new(parking_lot::Mutex::new(HashSet::new()));

    let handle = ClientHandle {
        cmd_tx: cmd_tx.clone(),
        echo_registry: echo_registry.clone(),
        caps_acked: caps_acked.clone(),
    };

    let echo_reg = echo_registry.clone();
    let caps_for_loop = caps_acked.clone();
    tokio::spawn(async move {
        if let Err(e) =
            run_client(config, signer, event_tx.clone(), cmd_rx, echo_reg, caps_for_loop).await
        {
            let _ = event_tx
                .send(Event::Disconnected {
                    reason: e.to_string(),
                })
                .await;
        }
    });

    (handle, event_rx)
}

async fn run_client(
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
    event_tx: mpsc::Sender<Event>,
    cmd_rx: mpsc::Receiver<Command>,
    echo_registry: EchoRegistry,
    caps_acked: CapsAcked,
) -> Result<()> {
    let conn = establish_connection(&config).await?;
    let _ = event_tx.send(Event::Connected).await;
    match conn {
        EstablishedConnection::Plain(tcp) => {
            let (reader, writer) = tokio::io::split(tcp);
            run_irc(
                BufReader::new(reader),
                writer,
                &config,
                signer,
                event_tx,
                cmd_rx,
                echo_registry,
                caps_acked,
            )
            .await
        }
        EstablishedConnection::Tls(tls) => {
            let (reader, writer) = tokio::io::split(tls);
            run_irc(
                BufReader::new(reader),
                writer,
                &config,
                signer,
                event_tx,
                cmd_rx,
                echo_registry,
                caps_acked,
            )
            .await
        }
        #[cfg(feature = "iroh-transport")]
        EstablishedConnection::Iroh(duplex) => {
            let (reader, writer) = tokio::io::split(duplex);
            run_irc(
                BufReader::new(reader),
                writer,
                &config,
                signer,
                event_tx,
                cmd_rx,
                echo_registry,
                caps_acked,
            )
            .await
        }
        #[cfg(feature = "websocket")]
        EstablishedConnection::WebSocket(duplex) => {
            let (reader, writer) = tokio::io::split(duplex);
            run_irc(
                BufReader::new(reader),
                writer,
                &config,
                signer,
                event_tx,
                cmd_rx,
                echo_registry,
                caps_acked,
            )
            .await
        }
    }
}

/// Connect via WebSocket and bridge the framed transport to a `DuplexStream`
/// that `run_irc()` reads/writes as plain IRC bytes.
///
/// The server-side counterpart is `freeq-server/src/web.rs::bridge_ws()`,
/// which terminates the WebSocket and feeds bytes into the same IRC handler
/// it uses for raw TCP. Here we do the mirror: outbound bytes from
/// `run_irc` are wrapped in `WsMessage::Text`, and inbound text/binary
/// frames are written back into the duplex.
#[cfg(feature = "websocket")]
async fn establish_ws_connection(url: &str) -> Result<EstablishedConnection> {
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    // The rustls default crypto provider must be installed before any TLS
    // handshake — including the one tokio-tungstenite does internally for
    // wss://. The TCP-with-TLS path covers this in `rustls_default_config()`,
    // but the WebSocket path bypasses that helper, so the install was being
    // skipped and the wss handshake silently hung.
    install_crypto_provider();

    tracing::debug!("Connecting WebSocket {url}...");
    let connect_result = tokio::time::timeout(
        TRANSPORT_CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(url),
    )
    .await;
    let (ws, _resp) = match connect_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            return Err(anyhow::anyhow!("WebSocket connect to {url} failed: {e}"))
        }
        Err(_) => {
            return Err(anyhow::anyhow!(
                "WebSocket connect to {url} timed out after {}s",
                TRANSPORT_CONNECT_TIMEOUT.as_secs()
            ))
        }
    };
    tracing::debug!("WebSocket connected: {url}");

    // 64 KiB matches the JS transport's bufferedAmount threshold and gives
    // both directions room without unbounded memory.
    let (client_side, bridge_side) = tokio::io::duplex(65_536);
    let (mut bridge_reader, mut bridge_writer) = tokio::io::split(bridge_side);
    let (mut ws_writer, mut ws_reader) = ws.split();

    // Wire framing: mirror what `freeq-server/src/web.rs::bridge_ws()` does
    // on the server side. One WebSocket text frame == one IRC line without
    // its trailing CRLF. The server appends `\r\n` on receive and strips it
    // before forwarding outbound frames; we do the symmetric thing here.

    // Outbound: read CRLF-terminated lines from `run_irc`'s write half and
    // emit each as its own WS text frame (without the CRLF).
    tokio::spawn(async move {
        tracing::info!("WS: outbound bridge task started");
        let mut buf = vec![0u8; 4096];
        let mut line_buf: Vec<u8> = Vec::new();
        let mut frames_out: u64 = 0;
        loop {
            match bridge_reader.read(&mut buf).await {
                Ok(0) => {
                    tracing::warn!(frames_out, "WS: outbound EOF on bridge_read");
                    break;
                }
                Ok(n) => {
                    line_buf.extend_from_slice(&buf[..n]);
                    while let Some(pos) = line_buf.windows(2).position(|w| w == b"\r\n") {
                        let line = String::from_utf8_lossy(&line_buf[..pos]).into_owned();
                        line_buf.drain(..pos + 2);
                        frames_out += 1;
                        let preview: String = line.chars().take(80).collect();
                        tracing::debug!(n = frames_out, preview = %preview, "WS: → text frame");
                        if let Err(e) = ws_writer.send(WsMessage::Text(line.into())).await {
                            tracing::warn!("WS: outbound send error: {e}");
                            return;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("WS: bridge_read error: {e}");
                    break;
                }
            }
        }
        tracing::warn!(frames_out, "WS: outbound bridge task ending");
        let _ = ws_writer.close().await;
    });

    // Inbound: each WS text frame is one IRC line without CRLF — append
    // CRLF before writing into the duplex so `run_irc`'s line reader sees
    // properly terminated lines and doesn't hang waiting for `\n`.
    tokio::spawn(async move {
        tracing::info!("WS: inbound bridge task started");
        let mut frames_in: u64 = 0;
        while let Some(msg) = ws_reader.next().await {
            let mut bytes = match msg {
                Ok(WsMessage::Text(t)) => {
                    frames_in += 1;
                    let preview: String = t.chars().take(80).collect();
                    tracing::debug!(n = frames_in, len = t.len(), preview = %preview, "WS: ← text frame");
                    t.as_bytes().to_vec()
                }
                Ok(WsMessage::Binary(b)) => {
                    frames_in += 1;
                    tracing::debug!(n = frames_in, len = b.len(), "WS: ← binary frame");
                    b.to_vec()
                }
                Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) | Ok(WsMessage::Frame(_)) => continue,
                Ok(WsMessage::Close(c)) => {
                    tracing::warn!(close = ?c, "WS: ← close frame");
                    break;
                }
                Err(e) => {
                    tracing::warn!("WS: read error: {e}");
                    break;
                }
            };
            if !bytes.ends_with(b"\r\n") {
                bytes.extend_from_slice(b"\r\n");
            }
            if let Err(e) = bridge_writer.write_all(&bytes).await {
                tracing::warn!("WS: bridge_write error: {e}");
                break;
            }
        }
        tracing::warn!(frames_in, "WS: inbound bridge task ending");
        let _ = bridge_writer.shutdown().await;
    });

    Ok(EstablishedConnection::WebSocket(client_side))
}

fn install_crypto_provider() {
    // Install a crypto provider for rustls.
    // ring is preferred (works on iOS); aws-lc-rs is the default on desktop.
    #[cfg(feature = "ring")]
    {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    #[cfg(all(feature = "aws-lc-rs", not(feature = "ring")))]
    {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

fn rustls_default_config() -> rustls::ClientConfig {
    install_crypto_provider();

    let mut root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // Also load the system's native certificate store (covers CAs not in
    // Mozilla's bundle, e.g. corporate/Sectigo intermediates).
    let native = rustls_native_certs::load_native_certs();
    if !native.errors.is_empty() {
        tracing::warn!("Errors loading native certificates: {:?}", native.errors);
    }
    let before = root_store.len();
    for cert in native.certs {
        let _ = root_store.add(cert);
    }
    let added = root_store.len() - before;
    if added > 0 {
        tracing::debug!("Loaded {added} native root certificates");
    }

    rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

fn rustls_insecure_config() -> rustls::ClientConfig {
    install_crypto_provider();
    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
        .with_no_client_auth()
}

#[derive(Debug)]
struct InsecureVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms.supported_schemes())
            .unwrap_or_default()
    }
}

async fn run_irc<R, W>(
    mut reader: R,
    mut writer: W,
    config: &ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
    event_tx: mpsc::Sender<Event>,
    mut cmd_rx: mpsc::Receiver<Command>,
    echo_registry: EchoRegistry,
    caps_acked: CapsAcked,
) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // Always negotiate capabilities (message-tags, and optionally sasl)
    writer.write_all(b"CAP LS 302\r\n").await?;

    writer
        .write_all(format!("NICK {}\r\n", config.nick).as_bytes())
        .await?;
    writer
        .write_all(format!("USER {} 0 * :{}\r\n", config.user, config.realname).as_bytes())
        .await?;

    let mut sasl_in_progress = false;
    let mut registered = false;
    let mut nick_tries: u32 = 0;
    let mut web_token = config.web_token.clone();
    let mut authenticated_did: Option<String> = None;
    let mut pending_commands: Vec<Command> = Vec::new();
    // Session message-signing keypair (generated after SASL success)
    let mut msg_signing_key: Option<ed25519_dalek::SigningKey> = None;
    let mut msg_signing_did: Option<String> = None;
    // Open `draft/multiline` batches keyed by batch id. Chunks
    // accumulate here while the batch is open; the BATCH closer drains
    // and emits a single Event::Message with the assembled body.
    let mut multiline_batches: std::collections::HashMap<String, InboundMultilineBatch> =
        std::collections::HashMap::new();
    let mut line_buf = String::new();
    let mut last_activity = tokio::time::Instant::now();
    let ping_interval = tokio::time::Duration::from_secs(60);
    let ping_timeout = tokio::time::Duration::from_secs(120);

    loop {
        tokio::select! {
            result = reader.read_line(&mut line_buf) => {
                let n = result?;
                if n == 0 {
                    let _ = event_tx.send(Event::Disconnected { reason: "EOF".to_string() }).await;
                    break;
                }

                last_activity = tokio::time::Instant::now();
                let raw = line_buf.trim_end().to_string();
                let _ = event_tx.send(Event::RawLine(raw.clone())).await;

                if let Some(msg) = Message::parse(&line_buf) {
                    match msg.command.as_str() {
                        // ERR_NICKNAMEINUSE
                        "433" => {
                            // Nickname is already in use; try a variant before registration completes.
                            // Use base nick from config and append a short suffix.
                            nick_tries = nick_tries.saturating_add(1);
                            if nick_tries <= 5 {
                                let base = &config.nick;
                                let alt = if nick_tries == 1 {
                                    format!("{}1", base)
                                } else {
                                    format!("{}{}", base, nick_tries)
                                };
                                // Best-effort: attempt new nick immediately.
                                writer.write_all(format!("NICK {}\r\n", alt).as_bytes()).await?;
                            } else {
                                // Give up; let reconnect logic handle it.
                                let _ = event_tx.send(Event::Disconnected { reason: "Nick in use".to_string() }).await;
                                break;
                            }
                        }
                        "CAP" => {
                            handle_cap_response(&msg, &signer, &web_token, &mut writer, &mut sasl_in_progress, &caps_acked).await?;
                        }
                        "AUTHENTICATE" => {
                            if let Some(ref token) = web_token {
                                // Web-token SASL: server sends challenge, we respond with JSON
                                // containing method:"web-token" and the token as signature.
                                // The DID is extracted by the server from the token lookup.
                                let payload = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                                if payload == "+" || !payload.is_empty() {
                                    // For web-token, we need the DID from the token lookup.
                                    // Send a JSON response matching ChallengeResponse format.
                                    // The DID comes from the server's token store, but we need
                                    // to send *something* — use a placeholder that matches.
                                    // Actually, we need the DID. Extract from config or just
                                    // send with empty DID — server validates via token lookup.
                                    let response = serde_json::json!({
                                        "did": "", // Server fills from token lookup
                                        "method": "web-token",
                                        "signature": token,
                                    });
                                    use base64::Engine;
                                    let json_bytes = response.to_string();
                                    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json_bytes.as_bytes());
                                    writer.write_all(format!("AUTHENTICATE {encoded}\r\n").as_bytes()).await?;
                                }
                            } else if let Some(ref signer) = signer {
                                handle_authenticate_challenge(&msg, signer.as_ref(), &mut writer).await?;
                            }
                        }
                        // Handle DPOP_NONCE notice during SASL — update signer nonce
                        "NOTICE" if sasl_in_progress => {
                            if let Some(text) = msg.params.last()
                                && let Some(nonce) = text.strip_prefix("DPOP_NONCE ")
                                    && let Some(ref s) = signer {
                                        s.set_dpop_nonce(nonce.trim());
                                    }
                                    // Server will re-issue AUTHENTICATE challenge next
                        }
                        // 900 RPL_LOGGEDIN — server tells us our authenticated DID
                        "900" => {
                            // :server 900 nick :You are now logged in as did:plc:...
                            if let Some(text) = msg.params.last()
                                && let Some(did) = text.split("as ").last() {
                                    let did = did.trim().to_string();
                                    if did.starts_with("did:") {
                                        authenticated_did = Some(did);
                                    }
                                }
                        }
                        "903" => {
                            sasl_in_progress = false;
                            let did = authenticated_did.take()
                                .or_else(|| signer.as_ref().map(|s| s.did().to_string()))
                                .unwrap_or_default();
                            if !did.is_empty() {
                                let _ = event_tx.send(Event::Authenticated { did: did.clone() }).await;
                                // Generate session message-signing keypair
                                let key = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
                                let pubkey_bytes = key.verifying_key().as_bytes().to_vec();
                                use base64::Engine;
                                let pubkey_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&pubkey_bytes);
                                msg_signing_key = Some(key);
                                msg_signing_did = Some(did);
                                writer.write_all(format!("MSGSIG {pubkey_b64}\r\n").as_bytes()).await?;
                            }
                            web_token = None;
                            writer.write_all(b"CAP END\r\n").await?;
                        }
                        "904" => {
                            sasl_in_progress = false;
                            let reason = msg.params.get(1).cloned().unwrap_or_else(|| "Unknown".to_string());
                            // eprintln!("  SASL authentication FAILED: {reason}");
                            let _ = event_tx.send(Event::AuthFailed { reason }).await;
                            writer.write_all(b"CAP END\r\n").await?;
                        }
                        "BATCH" => {
                            if let Some(ref_id) = msg.params.first() {
                                if let Some(id) = ref_id.strip_prefix('+') {
                                    let batch_type = msg.params.get(1).cloned().unwrap_or_default();
                                    let target = msg.params.get(2).cloned().unwrap_or_default();
                                    if batch_type == "draft/multiline" {
                                        // Capture opener metadata so the
                                        // assembled message inherits the
                                        // right identity (msgid, time,
                                        // sender, client-only tags).
                                        let from = msg
                                            .prefix
                                            .as_deref()
                                            .and_then(|p| p.split('!').next())
                                            .unwrap_or("")
                                            .to_string();
                                        let mut opener_tags = msg.tags.clone();
                                        // The `batch` tag on the opener
                                        // is the PARENT batch (nesting).
                                        let parent_batch_id =
                                            opener_tags.remove("batch");
                                        multiline_batches.insert(
                                            id.to_string(),
                                            InboundMultilineBatch {
                                                target: target.clone(),
                                                from,
                                                opener_tags,
                                                lines: Vec::new(),
                                                parent_batch_id,
                                            },
                                        );
                                        // Suppress BatchStart for multiline
                                        // — the consumer gets a single
                                        // Message at close time instead.
                                    } else {
                                        let _ = event_tx.send(Event::BatchStart {
                                            id: id.to_string(),
                                            batch_type,
                                            target,
                                        }).await;
                                    }
                                } else if let Some(id) = ref_id.strip_prefix('-') {
                                    if let Some(batch) = multiline_batches.remove(id) {
                                        dispatch_assembled_multiline(&event_tx, batch).await;
                                    } else {
                                        let _ = event_tx.send(Event::BatchEnd { id: id.to_string() }).await;
                                    }
                                }
                            }
                        }
                        "001" => {
                            let nick = msg.params.first().cloned().unwrap_or_default();
                            let _ = event_tx.send(Event::Registered { nick }).await;
                            registered = true;
                            // Flush any commands that were queued before registration
                            for cmd in pending_commands.drain(..) {
                                execute_command(&mut writer, cmd, &msg_signing_key, &msg_signing_did).await?;
                            }
                        }
                        "353" => {
                            if msg.params.len() >= 4 {
                                let channel = msg.params[2].clone();
                                let nicks: Vec<String> = msg.params[3].split_whitespace().map(|s| s.to_string()).collect();
                                let _ = event_tx.send(Event::Names { channel, nicks }).await;
                            }
                        }
                        "366" => {
                            // RPL_ENDOFNAMES
                            if msg.params.len() >= 2 {
                                let channel = msg.params[1].clone();
                                let _ = event_tx.send(Event::NamesEnd { channel }).await;
                            }
                        }
                        "PING" => {
                            let token = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                            writer.write_all(format!("PONG :{token}\r\n").as_bytes()).await?;
                        }
                        "JOIN" => {
                            let channel = msg.params.first().cloned().unwrap_or_default();
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let _ = event_tx.send(Event::Joined { channel, nick }).await;
                        }
                        "PART" => {
                            let channel = msg.params.first().cloned().unwrap_or_default();
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let _ = event_tx.send(Event::Parted { channel, nick }).await;
                        }
                        "NICK" => {
                            let old_nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let new_nick = msg.params.first().cloned().unwrap_or_default();
                            if !old_nick.is_empty() && !new_nick.is_empty() {
                                let _ = event_tx.send(Event::NickChanged { old_nick, new_nick }).await;
                            }
                        }
                        // MODE change
                        "MODE" => {
                            if msg.params.len() >= 2 {
                                let target = &msg.params[0];
                                if target.starts_with('#') || target.starts_with('&') {
                                    let mode = msg.params[1].clone();
                                    let arg = msg.params.get(2).cloned();
                                    let set_by = msg.prefix.as_deref()
                                        .and_then(|p| p.split('!').next())
                                        .unwrap_or("server")
                                        .to_string();
                                    let _ = event_tx.send(Event::ModeChanged {
                                        channel: target.clone(),
                                        mode,
                                        arg,
                                        set_by,
                                    }).await;
                                }
                            }
                        }
                        // KICK
                        "KICK" => {
                            if msg.params.len() >= 2 {
                                let channel = msg.params[0].clone();
                                let kicked_nick = msg.params[1].clone();
                                let reason = msg.params.get(2).cloned().unwrap_or_default();
                                let by = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .unwrap_or("server")
                                    .to_string();
                                let _ = event_tx.send(Event::Kicked {
                                    channel,
                                    nick: kicked_nick,
                                    by,
                                    reason,
                                }).await;
                            }
                        }
                        // INVITE
                        "INVITE" => {
                            if msg.params.len() >= 2 {
                                let channel = msg.params[1].clone();
                                let by = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .unwrap_or("someone")
                                    .to_string();
                                let _ = event_tx.send(Event::Invited { channel, by }).await;
                            }
                        }
                        // AWAY (away-notify broadcast from shared channels)
                        "AWAY" => {
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let away_msg = msg.params.first().cloned();
                            let _ = event_tx.send(Event::AwayChanged { nick, away_msg }).await;
                        }
                        // TOPIC (live change from another user)
                        "TOPIC" => {
                            if let Some(channel) = msg.params.first() {
                                let topic = msg.params.get(1).cloned().unwrap_or_default();
                                let set_by = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .map(|s| s.to_string());
                                let _ = event_tx.send(Event::TopicChanged {
                                    channel: channel.clone(),
                                    topic,
                                    set_by,
                                }).await;
                            }
                        }
                        // RPL_TOPIC (on join or TOPIC query)
                        "332" => {
                            if msg.params.len() >= 3 {
                                let channel = msg.params[1].clone();
                                let topic = msg.params[2].clone();
                                let _ = event_tx.send(Event::TopicChanged {
                                    channel,
                                    topic,
                                    set_by: None,
                                }).await;
                            }
                        }
                        "331" => {
                            // RPL_NOTOPIC — no topic set, ignore or clear
                        }
                        "333" => {
                            // RPL_TOPICWHOTIME — ignore for now (info only)
                        }
                        // WHOIS numerics
                        "311" => {
                            // RPL_WHOISUSER: <nick> <user> <host> * :<realname>
                            if msg.params.len() >= 5 {
                                let nick = msg.params[1].clone();
                                let user = &msg.params[2];
                                let host = &msg.params[3];
                                let realname = &msg.params[4]; // skip the "*" at [3] — it's actually nick user host * :realname
                                let info = format!("{nick} is {user}@{host} ({realname})");
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "312" => {
                            // RPL_WHOISSERVER: <nick> <server> :<server info>
                            if msg.params.len() >= 4 {
                                let nick = msg.params[1].clone();
                                let server = &msg.params[2];
                                let info_text = &msg.params[3];
                                let info = format!("{nick} using {server} ({info_text})");
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "319" => {
                            // RPL_WHOISCHANNELS: <nick> :<channels>
                            if msg.params.len() >= 3 {
                                let nick = msg.params[1].clone();
                                let info = format!("{nick} on {}", msg.params[2]);
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "330" => {
                            // RPL_WHOISACCOUNT: <nick> <account> :is logged in as
                            if msg.params.len() >= 3 {
                                let nick = msg.params[1].clone();
                                let account = &msg.params[2];
                                let label = msg.params.get(3).map(|s| s.as_str()).unwrap_or("is authenticated as");
                                let info = format!("{nick} {label} {account}");
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "318" => {
                            // RPL_ENDOFWHOIS — ignore silently
                        }
                        "401" => {
                            // ERR_NOSUCHNICK
                            if msg.params.len() >= 3 {
                                let nick = msg.params[1].clone();
                                let _ = event_tx.send(Event::WhoisReply {
                                    nick: nick.clone(),
                                    info: format!("{nick}: No such nick"),
                                }).await;
                            }
                        }
                        "QUIT" => {
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let reason = msg.params.first().cloned().unwrap_or_default();
                            let _ = event_tx.send(Event::UserQuit { nick, reason }).await;
                        }
                        "PRIVMSG" | "NOTICE" => {
                            if msg.params.len() >= 2 {
                                let prefix = msg.prefix.as_deref().unwrap_or("");
                                let is_server_notice = msg.command == "NOTICE"
                                    && !prefix.contains('!');
                                if is_server_notice {
                                    // Server NOTICE (no hostmask in prefix) → ServerNotice
                                    let text = msg.params[1].clone();
                                    let _ = event_tx.send(Event::ServerNotice { text }).await;
                                } else {
                                    // If this PRIVMSG is a chunk of an
                                    // open `draft/multiline` batch,
                                    // accumulate it raw and defer the
                                    // Event::Message emission until the
                                    // closer fires. Decoding per-chunk
                                    // would corrupt ciphertext-chunked
                                    // bodies (each fragment is part of
                                    // one AES-GCM blob).
                                    if let Some(batch_id) = msg.tags.get("batch")
                                        && let Some(batch) =
                                            multiline_batches.get_mut(batch_id)
                                    {
                                        batch.lines.push(MultilineChunk {
                                            body: msg.params[1].clone(),
                                            concat: msg
                                                .tags
                                                .contains_key("+draft/multiline-concat"),
                                        });
                                        line_buf.clear();
                                        continue;
                                    }

                                    let from = prefix.split('!').next()
                                        .unwrap_or("")
                                        .to_string();
                                    let target = msg.params[0].clone();
                                    let mut text = msg.params[1].clone();
                                    let tags = msg.tags.clone();

                                    // Legacy `+freeq.at/multiline`: pre-spec
                                    // wire encoded `\n` as the literal two
                                    // chars `\\n`. New senders use the BATCH
                                    // path; the tag remains in use by older
                                    // senders. Normalize here so consumers
                                    // always see real `\n`.
                                    if tags.contains_key("+freeq.at/multiline") {
                                        text = text.replace("\\n", "\n");
                                    }

                                    // Check for echo-nonce match (for send_and_await_echo)
                                    if let Some(nonce) = tags.get("+freeq.at/echo-nonce")
                                        && let Some(tx) = echo_registry.lock().remove(nonce)
                                        && let Some(msgid) = tags.get("msgid")
                                    {
                                        let _ = tx.send(msgid.clone());
                                    }

                                    let _ = event_tx.send(Event::Message { from, target, text, tags }).await;
                                }
                            }
                        }
                        "TAGMSG" => {
                            if !msg.params.is_empty() {
                                let from = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .unwrap_or("")
                                    .to_string();
                                let target = msg.params[0].clone();
                                let _ = event_tx.send(Event::TagMsg { from, target, tags: msg.tags.clone() }).await;
                            }
                        }
                        "CHATHISTORY" => {
                            // CHATHISTORY TARGETS <nick> — DM conversation list
                            #[allow(clippy::collapsible_if)]
                            if msg.params.first().map(|s| s.as_str()) == Some("TARGETS") {
                                if let Some(nick) = msg.params.get(1) {
                                    let timestamp = msg.tags.get("time").cloned();
                                    let _ = event_tx
                                        .send(Event::ChatHistoryTarget {
                                            nick: nick.clone(),
                                            timestamp,
                                        })
                                        .await;
                                }
                            }
                        }
                        "FAIL" => {
                            // IRCv3 FAIL command — emit as ServerNotice
                            let text = msg.params.join(" ");
                            let _ = event_tx.send(Event::ServerNotice { text }).await;
                        }
                        _ => {
                            // Emit server error numerics (4xx, 5xx, 6xx, 9xx),
                            // MOTD lines (372/375/376), and unrecognized commands
                            // as ServerNotice so the UI can display them.
                            if let Ok(num) = msg.command.parse::<u16>() {
                                if (400..700).contains(&num) || (900..1000).contains(&num) {
                                    // Skip our nick (param[0]) and join the rest
                                    let text = if msg.params.len() > 1 {
                                        msg.params[1..].join(" ")
                                    } else {
                                        msg.params.join(" ")
                                    };
                                    let _ = event_tx.send(Event::ServerNotice { text }).await;
                                } else if num == 372 {
                                    // MOTD body line — strip "- " prefix
                                    let text = if msg.params.len() > 1 {
                                        let body = msg.params[1..].join(" ");
                                        let stripped = body.strip_prefix("- ").unwrap_or(&body);
                                        format!("MOTD:{}", stripped)
                                    } else {
                                        "MOTD:".to_string()
                                    };
                                    let _ = event_tx.send(Event::ServerNotice { text }).await;
                                } else if num == 375 {
                                    let _ = event_tx.send(Event::ServerNotice { text: "MOTD:START".to_string() }).await;
                                } else if num == 376 {
                                    let _ = event_tx.send(Event::ServerNotice { text: "MOTD:END".to_string() }).await;
                                }
                            }
                        }
                    }
                }

                line_buf.clear();
            }
            Some(cmd) = cmd_rx.recv() => {
                if registered || matches!(cmd, Command::Quit(_)) {
                    execute_command(&mut writer, cmd, &msg_signing_key, &msg_signing_did).await?;
                    if !registered {
                        break; // Quit before registration
                    }
                } else {
                    // Queue until registered — commands silently wait
                    pending_commands.push(cmd);
                }
            }
            // Periodic client-to-server PING and timeout detection
            _ = tokio::time::sleep_until(last_activity + ping_interval) => {
                if last_activity.elapsed() > ping_timeout {
                    let _ = event_tx.send(Event::Disconnected { reason: "Ping timeout".to_string() }).await;
                    break;
                }
                writer.write_all(b"PING :keepalive\r\n").await?;
            }
        }
    }

    Ok(())
}

/// Assemble a closed `draft/multiline` batch into a single
/// `Event::Message` per the spec's concat rules — a chunk with
/// `+draft/multiline-concat` joins its predecessor with no separator;
/// otherwise the join is `\n`. The opener's tags become the assembled
/// message's tags (msgid, time, sender's account, etc.).
async fn dispatch_assembled_multiline(
    event_tx: &mpsc::Sender<Event>,
    batch: InboundMultilineBatch,
) {
    let mut text = String::new();
    for (i, line) in batch.lines.iter().enumerate() {
        if i > 0 && !line.concat {
            text.push('\n');
        }
        text.push_str(&line.body);
    }
    let _ = event_tx
        .send(Event::Message {
            from: batch.from,
            target: batch.target,
            text,
            tags: batch.opener_tags,
        })
        .await;
    // Nested-batch parent (e.g. multiline inside CHATHISTORY) is
    // exposed to the consumer via the `batch` tag in `opener_tags`;
    // assembled multiline events fire at top-level regardless of
    // nesting depth.
    let _ = batch.parent_batch_id;
}

/// Execute a single IRC command on the wire.
/// If `signing_key` and `signing_did` are set, PRIVMSG gets a `+freeq.at/sig` tag.
async fn execute_command<W: AsyncWrite + Unpin>(
    writer: &mut W,
    cmd: Command,
    signing_key: &Option<ed25519_dalek::SigningKey>,
    signing_did: &Option<String>,
) -> Result<()> {
    match cmd {
        Command::Join(channel) => {
            writer
                .write_all(format!("JOIN {channel}\r\n").as_bytes())
                .await?;
        }
        Command::Privmsg { target, text } => {
            if let (Some(key), Some(did)) = (signing_key, signing_did) {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let canonical = format!("{did}\0{target}\0{text}\0{timestamp}");
                use ed25519_dalek::Signer;
                let sig = key.sign(canonical.as_bytes());
                use base64::Engine;
                let sig_b64 =
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
                // Send with IRCv3 message tag
                writer
                    .write_all(
                        format!("@+freeq.at/sig={sig_b64} PRIVMSG {target} :{text}\r\n").as_bytes(),
                    )
                    .await?;
            } else {
                writer
                    .write_all(format!("PRIVMSG {target} :{text}\r\n").as_bytes())
                    .await?;
            }
        }
        Command::SendMultiline { target, chunks, mut opener_tags } => {
            // Mint a batch id with a random suffix; collision-free
            // within a single connection at one nanosecond + random id.
            let batch_id = format!(
                "ml{:x}{:x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u32,
                rand::random::<u32>(),
            );
            // Reassemble the body per concat rules and sign it. The
            // signature rides on the BATCH opener — the server's
            // signature verification reads `+freeq.at/sig` from the
            // assembled message's tags (which are the opener tags
            // after multiline dispatch), so per-chunk sigs would not
            // verify against the canonical `{did}\0{target}\0{body}\0{ts}`.
            if let (Some(key), Some(did)) = (signing_key, signing_did) {
                let mut body = String::new();
                for (i, chunk) in chunks.iter().enumerate() {
                    if i > 0 && !chunk.concat {
                        body.push('\n');
                    }
                    body.push_str(&chunk.body);
                }
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let canonical = format!("{did}\0{target}\0{body}\0{timestamp}");
                use ed25519_dalek::Signer;
                let sig = key.sign(canonical.as_bytes());
                use base64::Engine;
                let sig_b64 =
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
                opener_tags.insert("+freeq.at/sig".to_string(), sig_b64);
            }
            let opener_tags_str = if opener_tags.is_empty() {
                String::new()
            } else {
                let s = opener_tags
                    .iter()
                    .map(|(k, v)| {
                        if v.is_empty() {
                            k.clone()
                        } else {
                            format!("{k}={v}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(";");
                format!("@{s} ")
            };
            writer
                .write_all(
                    format!("{opener_tags_str}BATCH +{batch_id} draft/multiline {target}\r\n")
                        .as_bytes(),
                )
                .await?;
            // Per-chunk PRIVMSGs carry `batch=<id>` and, if applicable,
            // `+draft/multiline-concat`. Sigs are on the opener, not here.
            for chunk in &chunks {
                let mut chunk_tags = format!("batch={batch_id}");
                if chunk.concat {
                    chunk_tags.push_str(";+draft/multiline-concat");
                }
                writer
                    .write_all(
                        format!("@{chunk_tags} PRIVMSG {target} :{}\r\n", chunk.body).as_bytes(),
                    )
                    .await?;
            }
            writer
                .write_all(format!("BATCH -{batch_id}\r\n").as_bytes())
                .await?;
        }
        Command::Raw(line) => {
            // Strip CRLF/NUL to prevent protocol injection via raw commands
            let safe: String = line.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
            tracing::debug!("[SDK] Raw command: {}", safe);
            writer.write_all(format!("{safe}\r\n").as_bytes()).await?;
            tracing::debug!("[SDK] Raw command sent OK");
        }
        Command::Quit(msg) => {
            let quit_line = match msg {
                Some(m) => format!("QUIT :{m}\r\n"),
                None => "QUIT\r\n".to_string(),
            };
            writer.write_all(quit_line.as_bytes()).await?;
        }
    }
    Ok(())
}

async fn handle_cap_response<W: AsyncWrite + Unpin>(
    msg: &Message,
    signer: &Option<Arc<dyn ChallengeSigner>>,
    web_token: &Option<String>,
    writer: &mut W,
    sasl_in_progress: &mut bool,
    caps_acked: &CapsAcked,
) -> Result<()> {
    let subcmd = msg.params.get(1).map(|s| s.to_ascii_uppercase());
    match subcmd.as_deref() {
        Some("LS") => {
            let caps_str = msg.params.last().map(|s| s.as_str()).unwrap_or("");
            let mut req_caps = Vec::new();
            if caps_str.contains("message-tags") {
                req_caps.push("message-tags");
            }
            for cap in &[
                "server-time",
                "batch",
                "echo-message",
                "away-notify",
                "account-notify",
                "account-tag",
                "extended-join",
                "draft/chathistory",
                "draft/multiline",
            ] {
                if caps_str.contains(cap) {
                    req_caps.push(cap);
                }
            }
            if caps_str.contains("sasl") && (signer.is_some() || web_token.is_some()) {
                req_caps.push("sasl");
            }
            if req_caps.is_empty() {
                // eprintln!("  No caps to request, sending CAP END");
                writer.write_all(b"CAP END\r\n").await?;
            } else {
                // eprintln!("  Requesting: {}", req_caps.join(" "));
                let req = format!("CAP REQ :{}\r\n", req_caps.join(" "));
                writer.write_all(req.as_bytes()).await?;
            }
        }
        Some("ACK") => {
            let caps = msg.params.last().map(|s| s.as_str()).unwrap_or("");
            // Record which caps the server ACKed so `ClientHandle::privmsg`
            // can route `\n`-bearing text to a draft/multiline BATCH.
            {
                let mut acked = caps_acked.lock();
                for cap in caps.split_whitespace() {
                    acked.insert(cap.to_string());
                }
            }
            if caps.contains("sasl") {
                *sasl_in_progress = true;
                // Both web-token and ATPROTO-CHALLENGE use the same SASL mechanism;
                // the method field in the JSON payload distinguishes them.
                writer
                    .write_all(b"AUTHENTICATE ATPROTO-CHALLENGE\r\n")
                    .await?;
            } else {
                writer.write_all(b"CAP END\r\n").await?;
            }
        }
        Some("NAK") => {
            // eprintln!("  Capabilities rejected, sending CAP END");
            writer.write_all(b"CAP END\r\n").await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_authenticate_challenge<W: AsyncWrite + Unpin>(
    msg: &Message,
    signer: &dyn ChallengeSigner,
    writer: &mut W,
) -> Result<()> {
    let encoded_challenge = msg.params.first().map(|s| s.as_str()).unwrap_or("");
    // eprintln!("  Received SASL challenge ({} bytes encoded)", encoded_challenge.len());

    // Decode the challenge to raw bytes — these are what we sign
    let challenge_bytes = auth::decode_challenge_bytes(encoded_challenge)?;
    // eprintln!("  Challenge decoded ({} bytes), signing with {}...", challenge_bytes.len(), signer.did());

    // Produce the response using the signer
    let response = signer.respond(&challenge_bytes)?;
    let encoded = auth::encode_response(&response);
    // eprintln!("  Sending AUTHENTICATE response ({} bytes)", encoded.len());

    writer
        .write_all(format!("AUTHENTICATE {encoded}\r\n").as_bytes())
        .await?;

    Ok(())
}

// ── Reconnect helper ──

/// Configuration for automatic reconnection.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial delay before first reconnect attempt.
    pub initial_delay: std::time::Duration,
    /// Maximum delay between reconnect attempts.
    pub max_delay: std::time::Duration,
    /// Multiplier for exponential backoff.
    pub backoff_factor: f64,
    /// Channels to rejoin after reconnecting.
    pub channels: Vec<String>,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay: std::time::Duration::from_secs(2),
            max_delay: std::time::Duration::from_secs(30),
            backoff_factor: 2.0,
            channels: Vec::new(),
        }
    }
}

/// Run an event loop with automatic reconnection.
///
/// The `handler` is called for each event. When disconnected, the loop
/// reconnects with exponential backoff and rejoins configured channels.
///
/// Returns only on unrecoverable errors or when the handler returns `Err`.
///
/// # Example
///
/// ```rust,no_run
/// use freeq_sdk::client::{ConnectConfig, ReconnectConfig, run_with_reconnect};
///
/// # async fn example() -> anyhow::Result<()> {
/// let config = ConnectConfig { /* ... */ ..Default::default() };
/// let reconnect = ReconnectConfig {
///     channels: vec!["#bots".into()],
///     ..Default::default()
/// };
///
/// run_with_reconnect(config, None, reconnect, |handle, event| {
///     Box::pin(async move {
///         // handle event
///         Ok(())
///     })
/// }).await
/// # }
/// ```
pub async fn run_with_reconnect<F>(
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
    reconnect_config: ReconnectConfig,
    handler: F,
) -> Result<()>
where
    F: Fn(
            ClientHandle,
            Event,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
{
    let mut delay = reconnect_config.initial_delay;
    let mut consecutive_failures = 0u32;

    loop {
        // Connect
        let conn = match establish_connection(&config).await {
            Ok(c) => {
                consecutive_failures = 0;
                delay = reconnect_config.initial_delay;
                c
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!(
                    error = %e,
                    attempt = consecutive_failures,
                    delay_secs = delay.as_secs(),
                    "Connection failed, retrying"
                );
                tokio::time::sleep(delay).await;
                // Exponential backoff with jitter
                let jitter = rand_jitter(delay.as_millis() as u64 / 4);
                delay = std::time::Duration::from_millis(
                    ((delay.as_millis() as f64 * reconnect_config.backoff_factor) as u64 + jitter)
                        .min(reconnect_config.max_delay.as_millis() as u64),
                );
                continue;
            }
        };

        let (handle, mut events) = connect_with_stream(conn, config.clone(), signer.clone());

        // Event loop
        let mut disconnected = false;
        while let Some(event) = events.recv().await {
            // Join configured channels once registered with the server.
            // (JOINs sent before registration are silently dropped by IRC servers.)
            if matches!(&event, Event::Registered { .. }) {
                for ch in &reconnect_config.channels {
                    let _ = handle.join(ch).await;
                }
            }
            if matches!(&event, Event::Disconnected { .. }) {
                disconnected = true;
            }
            if let Err(e) = handler(handle.clone(), event).await {
                tracing::error!(error = %e, "Handler error");
                // Non-fatal: continue processing
            }
            if disconnected {
                break;
            }
        }

        tracing::info!(delay_secs = delay.as_secs(), "Disconnected, will reconnect");
        tokio::time::sleep(delay).await;
        let jitter = rand_jitter(delay.as_millis() as u64 / 4);
        delay = std::time::Duration::from_millis(
            ((delay.as_millis() as f64 * reconnect_config.backoff_factor) as u64 + jitter)
                .min(reconnect_config.max_delay.as_millis() as u64),
        );
    }
}

/// Simple jitter: random value 0..max using thread_rng.
fn rand_jitter(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    rand::random::<u64>() % max
}

#[cfg(test)]
mod multiline_tests {
    use super::*;

    /// Assemble per concat rules — concat=true joins with no separator,
    /// concat=false joins with `\n`. The first chunk's `concat` is
    /// irrelevant (no predecessor).
    #[tokio::test]
    async fn assemble_no_concat_joins_with_newline() {
        let (tx, mut rx) = mpsc::channel(8);
        let batch = InboundMultilineBatch {
            target: "#room".to_string(),
            from: "bob".to_string(),
            opener_tags: std::collections::HashMap::new(),
            lines: vec![
                MultilineChunk { body: "hello".into(), concat: false },
                MultilineChunk { body: "world".into(), concat: false },
                MultilineChunk { body: "foo".into(), concat: false },
            ],
            parent_batch_id: None,
        };
        dispatch_assembled_multiline(&tx, batch).await;
        match rx.recv().await.unwrap() {
            Event::Message { from, target, text, .. } => {
                assert_eq!(from, "bob");
                assert_eq!(target, "#room");
                assert_eq!(text, "hello\nworld\nfoo");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn assemble_concat_joins_without_separator() {
        let (tx, mut rx) = mpsc::channel(8);
        let batch = InboundMultilineBatch {
            target: "#room".to_string(),
            from: "bob".to_string(),
            opener_tags: std::collections::HashMap::new(),
            lines: vec![
                MultilineChunk { body: "alpha".into(), concat: false },
                MultilineChunk { body: "beta".into(), concat: true },
                MultilineChunk { body: "gamma".into(), concat: false },
            ],
            parent_batch_id: None,
        };
        dispatch_assembled_multiline(&tx, batch).await;
        match rx.recv().await.unwrap() {
            Event::Message { text, .. } => assert_eq!(text, "alphabeta\ngamma"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Opener tags carry through to the assembled Message — receivers
    /// see msgid, time, etc. on the one synthetic event, not on the
    /// individual chunks.
    #[tokio::test]
    async fn assemble_threads_opener_tags() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut opener_tags = std::collections::HashMap::new();
        opener_tags.insert("msgid".to_string(), "01XYZ".to_string());
        opener_tags.insert("time".to_string(), "2026-05-29T17:00:00.000Z".to_string());
        opener_tags.insert("+freeq.at/payload".to_string(), "{}".to_string());
        let batch = InboundMultilineBatch {
            target: "#room".to_string(),
            from: "bob".to_string(),
            opener_tags,
            lines: vec![
                MultilineChunk { body: "x".into(), concat: false },
                MultilineChunk { body: "y".into(), concat: false },
            ],
            parent_batch_id: None,
        };
        dispatch_assembled_multiline(&tx, batch).await;
        match rx.recv().await.unwrap() {
            Event::Message { tags, .. } => {
                assert_eq!(tags.get("msgid").map(String::as_str), Some("01XYZ"));
                assert_eq!(
                    tags.get("time").map(String::as_str),
                    Some("2026-05-29T17:00:00.000Z")
                );
                assert_eq!(tags.get("+freeq.at/payload").map(String::as_str), Some("{}"));
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Outbound: Command::SendMultiline writes BATCH + N PRIVMSGs + BATCH-
    /// to the wire, with the batch tag on each chunk and concat tags
    /// passed through.
    #[tokio::test]
    async fn send_multiline_emits_batch_frames() {
        let mut buf: Vec<u8> = Vec::new();
        let cmd = Command::SendMultiline {
            target: "#room".into(),
            chunks: vec![
                MultilineChunk { body: "one".into(), concat: false },
                MultilineChunk { body: "two".into(), concat: false },
                MultilineChunk { body: "three".into(), concat: false },
            ],
            opener_tags: std::collections::HashMap::new(),
        };
        execute_command(&mut buf, cmd, &None, &None).await.unwrap();
        let wire = String::from_utf8(buf).unwrap();
        // Find the batch id from the opener
        let opener_line = wire.lines().next().unwrap();
        assert!(opener_line.starts_with("BATCH +"));
        assert!(opener_line.contains("draft/multiline #room"));
        let batch_id = opener_line
            .strip_prefix("BATCH +")
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        // 3 chunks, all with batch=<id>
        for line in &["one", "two", "three"] {
            let pat = format!("@batch={batch_id} PRIVMSG #room :{line}");
            assert!(
                wire.contains(&pat),
                "expected line `{pat}` in wire:\n{wire}"
            );
        }
        // Closer
        assert!(wire.contains(&format!("BATCH -{batch_id}")));
    }

    #[tokio::test]
    async fn send_multiline_concat_tag_on_chunks() {
        let mut buf: Vec<u8> = Vec::new();
        let cmd = Command::SendMultiline {
            target: "#room".into(),
            chunks: vec![
                MultilineChunk { body: "ENC1:abc".into(), concat: false },
                MultilineChunk { body: "def".into(), concat: true },
                MultilineChunk { body: "ghi".into(), concat: true },
            ],
            opener_tags: std::collections::HashMap::new(),
        };
        execute_command(&mut buf, cmd, &None, &None).await.unwrap();
        let wire = String::from_utf8(buf).unwrap();
        // First chunk: no concat tag
        assert!(wire.contains(":ENC1:abc"));
        let first_chunk_line = wire
            .lines()
            .find(|l| l.contains(":ENC1:abc"))
            .unwrap();
        assert!(!first_chunk_line.contains("+draft/multiline-concat"));
        // Second + third chunks: concat tag present
        let def_line = wire.lines().find(|l| l.ends_with(":def")).unwrap();
        let ghi_line = wire.lines().find(|l| l.ends_with(":ghi")).unwrap();
        assert!(def_line.contains("+draft/multiline-concat"));
        assert!(ghi_line.contains("+draft/multiline-concat"));
    }

    #[tokio::test]
    async fn send_multiline_opener_tags_on_opener_only() {
        let mut buf: Vec<u8> = Vec::new();
        let mut opener_tags = std::collections::HashMap::new();
        opener_tags.insert("+reply".to_string(), "01ORIG".to_string());
        let cmd = Command::SendMultiline {
            target: "#room".into(),
            chunks: vec![
                MultilineChunk { body: "a".into(), concat: false },
                MultilineChunk { body: "b".into(), concat: false },
            ],
            opener_tags,
        };
        execute_command(&mut buf, cmd, &None, &None).await.unwrap();
        let wire = String::from_utf8(buf).unwrap();
        let opener = wire.lines().find(|l| l.contains("BATCH +")).unwrap();
        let chunk_a = wire.lines().find(|l| l.ends_with(":a")).unwrap();
        let chunk_b = wire.lines().find(|l| l.ends_with(":b")).unwrap();
        assert!(opener.contains("+reply=01ORIG"));
        assert!(!chunk_a.contains("+reply=01ORIG"));
        assert!(!chunk_b.contains("+reply=01ORIG"));
    }

    /// When a signing key is present, the BATCH opener carries a
    /// `+freeq.at/sig` tag computed over the ASSEMBLED body (not any
    /// single chunk). The server's verification reads the sig from
    /// the assembled-message tags after multiline dispatch, so this
    /// is the only placement that lets client sigs verify.
    #[tokio::test]
    async fn send_multiline_signs_assembled_body_on_opener() {
        use ed25519_dalek::SigningKey;
        // ed25519-dalek 2.x re-exports the rand_core trait it needs.
        use ed25519_dalek::ed25519::signature::rand_core::OsRng;
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let did = "did:plc:testkey".to_string();
        let mut buf: Vec<u8> = Vec::new();
        let cmd = Command::SendMultiline {
            target: "#room".into(),
            chunks: vec![
                MultilineChunk { body: "hello".into(), concat: false },
                MultilineChunk { body: "world".into(), concat: false },
                MultilineChunk { body: "foo".into(), concat: false },
            ],
            opener_tags: std::collections::HashMap::new(),
        };
        execute_command(&mut buf, cmd, &Some(key.clone()), &Some(did.clone()))
            .await
            .unwrap();
        let wire = String::from_utf8(buf).unwrap();
        let opener = wire.lines().find(|l| l.contains("BATCH +")).unwrap();
        // Sig present on opener
        assert!(opener.contains("+freeq.at/sig="), "opener: {opener}");
        // NOT on any chunk
        for line in &["hello", "world", "foo"] {
            let chunk_line = wire
                .lines()
                .find(|l| l.ends_with(&format!(":{line}")))
                .unwrap();
            assert!(
                !chunk_line.contains("+freeq.at/sig"),
                "chunk should not carry sig: {chunk_line}"
            );
        }
        // The sig MUST verify over the assembled body — extract and check.
        let sig_b64 = opener
            .split_whitespace()
            .find(|tok| tok.contains("+freeq.at/sig="))
            .unwrap()
            .split("+freeq.at/sig=")
            .nth(1)
            .unwrap()
            .trim_end_matches(';');
        // The opener tag block may pack multiple tags; pull the sig
        // value out cleanly by splitting on ; first.
        let sig_b64 = sig_b64.split(';').next().unwrap();
        use base64::Engine;
        let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(sig_b64)
            .unwrap();
        let sig_array: [u8; 64] = sig_bytes.try_into().unwrap();
        let sig = ed25519_dalek::Signature::from_bytes(&sig_array);
        // Try every plausible timestamp around now — the test runs in
        // <1s so the timestamp written by execute_command is within a
        // narrow window. We accept any second in the last 5.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        use ed25519_dalek::Verifier;
        let vk = key.verifying_key();
        let mut verified = false;
        for delta in 0..=5 {
            let ts = now - delta;
            let canonical = format!("{did}\0#room\0hello\nworld\nfoo\0{ts}");
            if vk.verify(canonical.as_bytes(), &sig).is_ok() {
                verified = true;
                break;
            }
        }
        assert!(verified, "sig did not verify over assembled body");
    }

    #[tokio::test]
    async fn send_multiline_empty_opener_tags_omits_tag_block() {
        let mut buf: Vec<u8> = Vec::new();
        let cmd = Command::SendMultiline {
            target: "#room".into(),
            chunks: vec![MultilineChunk { body: "x".into(), concat: false }],
            opener_tags: std::collections::HashMap::new(),
        };
        execute_command(&mut buf, cmd, &None, &None).await.unwrap();
        let wire = String::from_utf8(buf).unwrap();
        let opener = wire.lines().next().unwrap();
        // No leading "@..." tag block when there are no opener tags
        assert!(opener.starts_with("BATCH +"));
    }

    /// Auto-routing: `privmsg` with `\n`-bearing text and the
    /// `draft/multiline` + `batch` caps acked sends `SendMultiline`,
    /// one chunk per source line.
    #[tokio::test]
    async fn privmsg_auto_routes_to_multiline_when_cap_acked() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let caps_acked: CapsAcked = Arc::new(parking_lot::Mutex::new(HashSet::new()));
        caps_acked.lock().insert("draft/multiline".to_string());
        caps_acked.lock().insert("batch".to_string());
        let handle = ClientHandle {
            cmd_tx,
            echo_registry: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            caps_acked,
        };
        handle.privmsg("#test", "alpha\nbeta\ngamma").await.unwrap();
        match cmd_rx.recv().await.unwrap() {
            Command::SendMultiline { target, chunks, opener_tags } => {
                assert_eq!(target, "#test");
                assert!(opener_tags.is_empty());
                assert_eq!(chunks.len(), 3);
                assert_eq!(chunks[0].body, "alpha");
                assert_eq!(chunks[1].body, "beta");
                assert_eq!(chunks[2].body, "gamma");
                for c in &chunks {
                    assert!(!c.concat, "auto-routed chunks default to concat=false");
                }
            }
            other => panic!("expected SendMultiline, got {other:?}"),
        }
    }

    /// Without the multiline cap, `privmsg` falls back to a single
    /// `Privmsg` (existing behavior preserved — non-breaking).
    #[tokio::test]
    async fn privmsg_falls_back_to_single_when_cap_not_acked() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let caps_acked: CapsAcked = Arc::new(parking_lot::Mutex::new(HashSet::new()));
        // No caps acked
        let handle = ClientHandle {
            cmd_tx,
            echo_registry: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            caps_acked,
        };
        handle.privmsg("#test", "a\nb").await.unwrap();
        match cmd_rx.recv().await.unwrap() {
            Command::Privmsg { target, text } => {
                assert_eq!(target, "#test");
                assert_eq!(text, "a\nb");
            }
            other => panic!("expected Privmsg, got {other:?}"),
        }
    }

    /// send_tagged with `\n`-bearing text auto-routes to SendMultiline
    /// with the caller's tags moved onto the BATCH opener — preserving
    /// the tag semantics (e.g. commit-reveal payloads) under the
    /// multiline path.
    #[tokio::test]
    async fn send_tagged_auto_routes_to_multiline_with_opener_tags() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let caps_acked: CapsAcked = Arc::new(parking_lot::Mutex::new(HashSet::new()));
        caps_acked.lock().insert("draft/multiline".to_string());
        caps_acked.lock().insert("batch".to_string());
        let handle = ClientHandle {
            cmd_tx,
            echo_registry: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            caps_acked,
        };
        let mut tags = std::collections::HashMap::new();
        tags.insert("+freeq.at/event".to_string(), "reveal".to_string());
        tags.insert("+freeq.at/payload".to_string(), "%7B%7D".to_string());
        handle
            .send_tagged("#test", "x\ny\nz", tags)
            .await
            .unwrap();
        match cmd_rx.recv().await.unwrap() {
            Command::SendMultiline {
                target,
                chunks,
                opener_tags,
            } => {
                assert_eq!(target, "#test");
                assert_eq!(chunks.len(), 3);
                assert_eq!(
                    opener_tags.get("+freeq.at/event").map(String::as_str),
                    Some("reveal")
                );
                assert_eq!(
                    opener_tags.get("+freeq.at/payload").map(String::as_str),
                    Some("%7B%7D")
                );
            }
            other => panic!("expected SendMultiline, got {other:?}"),
        }
    }

    /// Single-line text always uses `Privmsg`, never `SendMultiline`,
    /// regardless of cap state.
    #[tokio::test]
    async fn privmsg_single_line_never_routes_to_multiline() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let caps_acked: CapsAcked = Arc::new(parking_lot::Mutex::new(HashSet::new()));
        caps_acked.lock().insert("draft/multiline".to_string());
        caps_acked.lock().insert("batch".to_string());
        let handle = ClientHandle {
            cmd_tx,
            echo_registry: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            caps_acked,
        };
        handle.privmsg("#test", "hello world").await.unwrap();
        match cmd_rx.recv().await.unwrap() {
            Command::Privmsg { target, text } => {
                assert_eq!(target, "#test");
                assert_eq!(text, "hello world");
            }
            other => panic!("expected Privmsg, got {other:?}"),
        }
    }

    /// CHATHISTORY replays multi-line messages as a `draft/multiline`
    /// BATCH nested inside the outer `chathistory` BATCH. This test
    /// drives that exact wire shape through `run_irc` and verifies the
    /// SDK assembles the inner batch into one `Event::Message` with
    /// the full body — the bug we suspected on Android shows up here
    /// if it's actually in the SDK rather than the Android UI.
    #[tokio::test]
    async fn nested_chathistory_batch_assembles_inner_multiline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

        let (client_side, mut server_side) = tokio::io::duplex(8192);
        let (event_tx, mut event_rx) = mpsc::channel::<Event>(32);
        let (_cmd_tx, cmd_rx) = mpsc::channel::<Command>(16);
        let echo_registry: EchoRegistry =
            Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let caps_acked: CapsAcked =
            Arc::new(parking_lot::Mutex::new(HashSet::new()));
        let config = ConnectConfig {
            server_addr: "test".to_string(),
            nick: "tester".to_string(),
            user: "tester".to_string(),
            realname: "tester".to_string(),
            tls: false,
            tls_insecure: false,
            web_token: None,
            websocket_url: None,
        };
        let (reader, writer) = tokio::io::split(client_side);

        tokio::spawn(async move {
            let _ = run_irc(
                BufReader::new(reader),
                writer,
                &config,
                None,
                event_tx,
                cmd_rx,
                echo_registry,
                caps_acked,
            )
            .await;
        });

        // Drain whatever run_irc writes on startup (CAP LS / NICK / USER)
        // so the duplex buffer doesn't fill and block.
        let mut drain = vec![0u8; 1024];
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(150),
            server_side.read(&mut drain),
        )
        .await;

        // Server-side wire: chathistory BATCH containing an inner
        // draft/multiline BATCH (two chunks) plus a regular PRIVMSG
        // sibling, then the chathistory closer. Matches what
        // freeq-server emits for stored multi-line rows during
        // CHATHISTORY replay (see freeq-server src/connection/messaging.rs
        // around the "nested BATCH path" comment).
        let wire = concat!(
            ":srv BATCH +cht1 chathistory #room\r\n",
            "@msgid=ML1;time=2026-05-30T18:00:00.000Z;batch=cht1 ",
            ":alice!a@h BATCH +ml1 draft/multiline #room\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :first line\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :second line\r\n",
            "@batch=cht1 BATCH -ml1\r\n",
            "@batch=cht1;msgid=R2 :bob!b@h PRIVMSG #room :sibling regular msg\r\n",
            ":srv BATCH -cht1\r\n",
        );
        server_side.write_all(wire.as_bytes()).await.unwrap();
        server_side.flush().await.unwrap();

        // Collect events until we hit the BatchEnd for cht1, or timeout.
        let mut events = Vec::new();
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(800),
            async {
                while let Some(ev) = event_rx.recv().await {
                    let done = matches!(&ev, Event::BatchEnd { id } if id == "cht1");
                    events.push(ev);
                    if done {
                        break;
                    }
                }
            },
        )
        .await;

        // The assembled multi-line should arrive as Event::Message with
        // text = "first line\nsecond line". Filter out RawLine noise.
        let assembled: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Message { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            assembled.iter().any(|t| *t == "first line\nsecond line"),
            "assembled multi-line message not found. messages dispatched: {assembled:#?}\nall events: {events:#?}",
        );
        assert!(
            assembled.iter().any(|t| t.contains("sibling regular msg")),
            "regular sibling PRIVMSG should also have been dispatched. got: {assembled:#?}",
        );
    }

    /// Same as the prior test but with the exact wire shape that smoke
    /// scenario B10 produces during CHATHISTORY replay: a multi-line
    /// message whose body is a label + a fenced code block (backticks +
    /// rust source). If the SDK assembles this with the full text
    /// preserved byte-exact, any "only line 1 shows" symptom on a
    /// client is the client's render layer, not the SDK.
    #[tokio::test]
    async fn nested_chathistory_batch_assembles_codeblock_byte_exact() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

        let (client_side, mut server_side) = tokio::io::duplex(8192);
        let (event_tx, mut event_rx) = mpsc::channel::<Event>(32);
        let (_cmd_tx, cmd_rx) = mpsc::channel::<Command>(16);
        let echo_registry: EchoRegistry =
            Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let caps_acked: CapsAcked =
            Arc::new(parking_lot::Mutex::new(HashSet::new()));
        let config = ConnectConfig {
            server_addr: "test".to_string(),
            nick: "tester".to_string(),
            user: "tester".to_string(),
            realname: "tester".to_string(),
            tls: false,
            tls_insecure: false,
            web_token: None,
            websocket_url: None,
        };
        let (reader, writer) = tokio::io::split(client_side);

        tokio::spawn(async move {
            let _ = run_irc(
                BufReader::new(reader),
                writer,
                &config,
                None,
                event_tx,
                cmd_rx,
                echo_registry,
                caps_acked,
            )
            .await;
        });

        let mut drain = vec![0u8; 1024];
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(150),
            server_side.read(&mut drain),
        )
        .await;

        // Exact b10 body: label + fenced rust block, as the smoke sends.
        // Six lines = six chunks in CHATHISTORY replay.
        let wire = concat!(
            ":srv BATCH +cht1 chathistory #room\r\n",
            "@msgid=B10;time=2026-05-30T18:00:00.000Z;batch=cht1 ",
            ":alice!a@h BATCH +ml1 draft/multiline #room\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :b10-stamp\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :```\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :fn main() {\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :    println!(\"hello\");\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :}\r\n",
            "@batch=ml1 :alice!a@h PRIVMSG #room :```\r\n",
            "@batch=cht1 BATCH -ml1\r\n",
            ":srv BATCH -cht1\r\n",
        );
        server_side.write_all(wire.as_bytes()).await.unwrap();
        server_side.flush().await.unwrap();

        let mut events = Vec::new();
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(800),
            async {
                while let Some(ev) = event_rx.recv().await {
                    let done = matches!(&ev, Event::BatchEnd { id } if id == "cht1");
                    events.push(ev);
                    if done {
                        break;
                    }
                }
            },
        )
        .await;

        let expected = "b10-stamp\n```\nfn main() {\n    println!(\"hello\");\n}\n```";
        let got = events.iter().find_map(|e| match e {
            Event::Message { text, .. } if text.starts_with("b10-stamp") => Some(text.clone()),
            _ => None,
        });
        assert_eq!(
            got.as_deref(),
            Some(expected),
            "codeblock body should be assembled byte-exact. events: {events:#?}",
        );
    }
}

#[cfg(test)]
mod connect_config_tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert!(ConnectConfig::default().validate().is_ok());
    }

    #[test]
    fn rejects_empty_and_oversized_nick() {
        let mut c = ConnectConfig::default();
        c.nick = String::new();
        assert!(c.validate().is_err());
        c.nick = "x".repeat(65);
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_nick_with_protocol_characters() {
        for bad in ["a b", "a,b", "a*b", "a?b", "a!b", "a@b", "a#b", "a\rb"] {
            let mut c = ConnectConfig::default();
            c.nick = bad.to_string();
            assert!(c.validate().is_err(), "nick {bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_empty_server_addr_and_user() {
        let mut c = ConnectConfig::default();
        c.server_addr = String::new();
        assert!(c.validate().is_err());

        let mut c = ConnectConfig::default();
        c.user = String::new();
        assert!(c.validate().is_err());
    }

    #[tokio::test]
    async fn establish_connection_enforces_validation() {
        let mut c = ConnectConfig::default();
        c.nick = "bad nick".to_string();
        let err = match establish_connection(&c).await {
            Ok(_) => panic!("invalid config must not connect"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("invalid ConnectConfig"),
            "got: {err}"
        );
    }
}
