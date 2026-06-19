//! External-brain seam — the unix-socket bridge to yokota.
//!
//! When `--external-brain` is set, eliza stops answering with its own
//! model. Instead it CONNECTS (as a client) to the unix socket yokota
//! listens on (`--brain-sock`), forwards every addressed utterance, and
//! speaks only the lines yokota sends back.
//!
//! Wire protocol — newline-delimited JSON, one object per line:
//!
//!   eliza → yokota:
//!     {"type":"ready","channel":"#room"}
//!     {"type":"utterance","nick":"chad","text":"what's the weather"}
//!     {"type":"left"} / {"type":"ended"}            (best-effort)
//!
//!   yokota → eliza:
//!     {"type":"say","text":"It's sunny."}           → spoken (dry voice)
//!     {"type":"show","topic":"planet Earth"}         → picture on the video tile
//!     {"type":"state","thinking":true}              → tile shows "thinking" mood
//!
//! eliza is the CLIENT; yokota is the SERVER. If the socket drops, the
//! seam reconnects with a simple fixed backoff.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;

use crate::irc::{ActiveCall, SharedConfig, spawn_scene_image, speak_text};
use crate::video::{SceneKind, SceneSpec};

/// A line eliza sends up to yokota.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum Outbound {
    #[serde(rename = "ready")]
    Ready { channel: String },
    #[serde(rename = "utterance")]
    Utterance { nick: String, text: String },
    /// A request the router classified as an agentic TASK — hand it to the
    /// Claude Code brain (tools + file access). yokota's `onDelegate` speaks
    /// an ack, runs it, and sends the result back as a `say`.
    #[serde(rename = "delegate")]
    Delegate { nick: String, text: String },
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "ended")]
    Ended,
}

/// A line yokota sends down to eliza.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Inbound {
    #[serde(rename = "say")]
    Say { text: String },
    /// yokota's brain state → drives the tile's "thinking"/working mood while
    /// the (remote) brain is composing a reply.
    #[serde(rename = "state")]
    State { thinking: bool },
    /// Put a generated picture up on the call's video tile. `topic` is a
    /// short subject (e.g. "planet Earth") used both as the scene-card
    /// title and the image-generation query. Rendered off the hot path —
    /// the spoken reply never waits on it.
    #[serde(rename = "show")]
    Show { topic: String },
    /// Anything else is ignored (forward-compatible).
    #[serde(other)]
    Other,
}

/// Cheap, clonable handle the IRC emit site uses to push lines to
/// yokota. Backed by an unbounded channel drained by the connect loop,
/// so sending never blocks the audio/STT path and survives reconnects.
#[derive(Clone)]
pub struct SeamHandle {
    tx: mpsc::UnboundedSender<Outbound>,
}

impl SeamHandle {
    /// Queue `msg` for delivery to yokota. Non-blocking; drops silently
    /// if the seam task has gone away (the bot keeps running regardless).
    pub fn send(&self, msg: Outbound) {
        if let Err(e) = self.tx.send(msg) {
            tracing::debug!(error = ?e, "brain seam: send dropped (seam task gone)");
        }
    }
}

/// Connect to yokota's unix socket and run the seam.
///
/// Returns a [`SeamHandle`] for the emit site immediately and spawns the
/// connect/reader loop in the background. `active` is the SAME live-call
/// slot the IRC loop uses, so an inbound `say` is spoken against whatever
/// call is currently active.
pub(crate) fn connect(
    cfg: Arc<SharedConfig>,
    sock_path: String,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> SeamHandle {
    let (tx, rx) = mpsc::unbounded_channel::<Outbound>();
    tokio::spawn(run(cfg, sock_path, active, rx));
    SeamHandle { tx }
}

async fn run(
    cfg: Arc<SharedConfig>,
    sock_path: String,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
    mut rx: mpsc::UnboundedReceiver<Outbound>,
) {
    const BACKOFF: Duration = Duration::from_secs(2);
    loop {
        let stream = match UnixStream::connect(&sock_path).await {
            Ok(s) => {
                tracing::info!(sock = %sock_path, "brain seam: connected to yokota");
                s
            }
            Err(e) => {
                tracing::warn!(sock = %sock_path, error = ?e, "brain seam: connect failed; retrying");
                tokio::time::sleep(BACKOFF).await;
                continue;
            }
        };

        let (read_half, mut write_half) = stream.into_split();

        // Announce ourselves with the first configured channel, if any.
        if let Some(channel) = cfg.channels.first().cloned() {
            let ready = Outbound::Ready { channel };
            if let Ok(mut line) = serde_json::to_string(&ready) {
                line.push('\n');
                let _ = write_half.write_all(line.as_bytes()).await;
            }
        }

        let mut reader = BufReader::new(read_half).lines();

        loop {
            tokio::select! {
                // Outbound: drain the queue to yokota.
                msg = rx.recv() => {
                    let Some(msg) = msg else {
                        // The handle was dropped — the bot is shutting
                        // down. Nothing left to forward.
                        tracing::info!("brain seam: outbound channel closed; stopping");
                        return;
                    };
                    match serde_json::to_string(&msg) {
                        Ok(mut line) => {
                            line.push('\n');
                            if let Err(e) = write_half.write_all(line.as_bytes()).await {
                                tracing::warn!(error = ?e, "brain seam: write failed; reconnecting");
                                break;
                            }
                        }
                        Err(e) => tracing::warn!(error = ?e, "brain seam: failed to encode outbound"),
                    }
                }
                // Inbound: yokota tells us what to say.
                line = reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            handle_inbound(&cfg, &active, &line).await;
                        }
                        Ok(None) => {
                            tracing::warn!("brain seam: yokota closed the socket; reconnecting");
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(error = ?e, "brain seam: read error; reconnecting");
                            break;
                        }
                    }
                }
            }
        }

        tokio::time::sleep(BACKOFF).await;
    }
}

/// Parse one inbound line and act on it. A `say` is spoken against the
/// currently active call; everything else is ignored.
async fn handle_inbound(
    cfg: &Arc<SharedConfig>,
    active: &Arc<AsyncMutex<Option<ActiveCall>>>,
    line: &str,
) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let msg: Inbound = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, line = %line, "brain seam: bad inbound line");
            return;
        }
    };
    match msg {
        Inbound::Say { text } => speak_inbound(cfg, active, text).await,
        Inbound::Show { topic } => show_inbound(cfg, active, topic).await,
        Inbound::State { thinking } => {
            let video = {
                let guard = active.lock().await;
                guard.as_ref().map(|call| call.video.clone())
            };
            if let Some(video) = video {
                video.set_thinking(thinking);
            }
        }
        Inbound::Other => {}
    }
}

/// Speak a `say` line against the currently active call. No-op if the
/// text is empty or no call is live.
async fn speak_inbound(
    cfg: &Arc<SharedConfig>,
    active: &Arc<AsyncMutex<Option<ActiveCall>>>,
    text: String,
) {
    if text.trim().is_empty() {
        return;
    }
    // Snapshot the active call's speaker + video (cheap clones) without
    // holding the lock across the (awaiting) TTS work.
    let snapshot = {
        let guard = active.lock().await;
        guard
            .as_ref()
            .map(|call| (call.speaker.clone(), call.video.clone()))
    };
    let Some((speaker, video)) = snapshot else {
        tracing::info!(%text, "brain seam: 'say' arrived with no active call; ignoring");
        return;
    };
    tracing::info!(%text, "brain seam: speaking yokota's line");
    if let Err(e) = speak_text(cfg, &speaker, Some(&video), &text).await {
        tracing::warn!(error = ?e, "brain seam: speak failed");
    }
}

/// Put a generated picture on the call's video tile. The scene card
/// appears immediately (title only); the backdrop image is fetched off
/// the hot path and attached when ready — so a `show` never blocks the
/// voice loop. No-op if no call is live.
async fn show_inbound(
    cfg: &Arc<SharedConfig>,
    active: &Arc<AsyncMutex<Option<ActiveCall>>>,
    topic: String,
) {
    let topic = topic.trim().to_string();
    if topic.is_empty() {
        return;
    }
    let video = {
        let guard = active.lock().await;
        guard.as_ref().map(|call| call.video.clone())
    };
    let Some(video) = video else {
        tracing::info!(%topic, "brain seam: 'show' arrived with no active call; ignoring");
        return;
    };
    tracing::info!(%topic, "brain seam: showing yokota's picture");
    let spec = SceneSpec {
        kind: SceneKind::Hero,
        title: topic.clone(),
        subtitle: String::new(),
        points: Vec::new(),
        accent: "#3effd6".to_string(),
        image_query: topic.clone(),
    };
    let scene_id = video.show_scene(spec);
    spawn_scene_image(cfg, &video, scene_id, topic);
}
