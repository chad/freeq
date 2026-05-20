//! IRC + AV orchestrator.
//!
//! Runs a single IRC connection, watches every channel the bot is in
//! for `+freeq.at/av-state` TAGMSGs, and — when a session starts —
//! sends `av-join`, opens a MoQ subscriber, taps the audio of every
//! remote participant, runs whisper on rolling windows, and posts the
//! transcript back to the channel.
//!
//! At most one active call at a time. If a second channel starts a
//! call while we're transcribing one, we log and skip.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use freeq_sdk::auth::KeySigner;
use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;
use iroh_live::media::subscribe::RemoteBroadcast;
use rand::RngCore;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::audio_tap::{TapBackend, to_whisper_pcm};
use crate::identity::Identity;
use crate::stt::Whisper;
use crate::summary;

pub struct RunConfig {
    pub server: String,
    pub channels: Vec<String>,
    pub nick: String,
    pub ident: Identity,
    pub stt: Arc<Whisper>,
    pub window_secs: f32,
    pub summary_model: String,
    pub anthropic_key: Option<String>,
}

/// Subset of [`RunConfig`] shared with inner tasks. Excludes the
/// PrivateKey (already moved into the signer) so it's `Clone`-friendly
/// inside an `Arc`.
struct SharedConfig {
    server: String,
    channels: Vec<String>,
    nick: String,
    stt: Arc<Whisper>,
    window_secs: f32,
    summary_model: String,
    anthropic_key: Option<String>,
}

/// Active-call state. Held inside an `Arc<AsyncMutex<Option<...>>>`
/// because the av-state handler and the av-state=ended handler need
/// to mutate it from different async paths.
struct ActiveCall {
    channel: String,
    session_id: String,
    instance_id: String,
    /// Lines of `<nick>: <utterance>` we've posted so far, used to
    /// build the end-of-call summary.
    transcript: Vec<String>,
    /// Drop on call end → cancels the MoQ subscriber + per-nick tap
    /// tasks via tokio's normal task-drop semantics.
    _moq_task: JoinHandle<()>,
}

pub async fn run(cfg: RunConfig) -> Result<()> {
    // Destructure up front so we own the individual fields; the cfg
    // we hand to the inner tasks (wrapped in Arc) is rebuilt below
    // without the moved-out PrivateKey.
    let RunConfig {
        server,
        channels,
        nick,
        ident: Identity { did, private_key },
        stt,
        window_secs,
        summary_model,
        anthropic_key,
    } = cfg;

    // Pick websocket vs raw-TCP transport based on URL scheme — mirrors
    // freeq-av-client's heuristic.
    let websocket_url = if server.starts_with("ws://")
        || server.starts_with("wss://")
        || server.starts_with("http://")
        || server.starts_with("https://")
    {
        Some(server.clone())
    } else {
        None
    };
    let server_addr = if let Some(ref ws) = websocket_url {
        // server_addr is unused on the WS path; pass a synthetic so
        // ConnectConfig::validate is happy.
        let u: url::Url = ws.parse().context("parsing WebSocket URL")?;
        let host = u.host_str().unwrap_or("localhost");
        format!("{host}:443")
    } else {
        server.clone()
    };

    let conn_config = ConnectConfig {
        server_addr,
        nick: nick.clone(),
        user: nick.clone(),
        realname: "freeq-transcriber-bot".to_string(),
        tls: websocket_url.is_some()
            || server.starts_with("https://")
            || server.starts_with("wss://"),
        tls_insecure: false,
        web_token: None,
        websocket_url,
    };

    let signer = Arc::new(KeySigner::new(did, private_key));
    let (handle, mut events) = client::connect(conn_config, Some(signer));

    // Wait for registration.
    let nick = wait_for_registration(&mut events).await?;
    tracing::info!(%nick, "registered with server");

    // Register as agent + minimal provenance so users can /whois us.
    let _ = handle.register_agent("agent").await;
    let _ = handle
        .submit_provenance(&serde_json::json!({
            "name": "freeq-transcriber-bot",
            "version": env!("CARGO_PKG_VERSION"),
            "runtime": "freeq-sdk/rust",
            "capabilities": ["av-transcription", "summary"],
        }))
        .await;
    let _ = handle
        .set_presence("active", Some("Listening for AV sessions"), None)
        .await;

    for ch in &channels {
        handle.join(ch).await.with_context(|| format!("joining {ch}"))?;
        tracing::info!(channel = %ch, "joined");
    }

    let active: Arc<AsyncMutex<Option<ActiveCall>>> = Arc::new(AsyncMutex::new(None));
    // Reassemble a sharable config without the (already-moved) private
    // key for the inner tasks.
    let cfg = Arc::new(SharedConfig {
        server,
        channels,
        nick,
        stt,
        window_secs,
        summary_model,
        anthropic_key,
    });
    let handle_arc = Arc::new(handle);

    loop {
        let Some(event) = events.recv().await else {
            tracing::warn!("event stream closed");
            return Ok(());
        };
        match event {
            Event::TagMsg { from: _, target, tags } => {
                let actor = tags
                    .get("+freeq.at/av-actor")
                    .cloned()
                    .unwrap_or_default();
                match classify_av_event(&target, &tags, &cfg.channels, &cfg.nick) {
                    AvAction::Start { channel, session_id } => {
                        let mut active_guard = active.lock().await;
                        if active_guard.is_some() {
                            tracing::info!(channel = %channel, "already in a call; ignoring new session");
                            continue;
                        }
                        match start_transcription(
                            cfg.clone(),
                            handle_arc.clone(),
                            channel.clone(),
                            session_id.clone(),
                            active.clone(),
                        )
                        .await
                        {
                            Ok(call) => {
                                tracing::info!(
                                    channel = %channel,
                                    session_id = %session_id,
                                    "started transcription"
                                );
                                *active_guard = Some(call);
                                let _ = handle_arc
                                    .privmsg(
                                        &channel,
                                        "[transcript] listening — I'll post utterances as I hear them.",
                                    )
                                    .await;
                            }
                            Err(e) => {
                                tracing::warn!(error = ?e, "failed to start transcription");
                            }
                        }
                    }
                    AvAction::End { channel, session_id } => {
                        let mut active_guard = active.lock().await;
                        let Some(call) = active_guard.take() else { continue };
                        if call.session_id != session_id {
                            // ended event for a different session
                            *active_guard = Some(call);
                            continue;
                        }
                        let cfg = cfg.clone();
                        let handle = handle_arc.clone();
                        let channel_for_post = channel.clone();
                        let transcript = call.transcript.join("\n");
                        // Drop the active call (tears down MoQ task).
                        drop(call);
                        drop(active_guard);

                        if !cfg.anthropic_key.is_some() || transcript.is_empty() {
                            let _ = handle
                                .privmsg(&channel_for_post, "[transcript] session ended.")
                                .await;
                            continue;
                        }
                        tokio::spawn(async move {
                            if let Some(key) = &cfg.anthropic_key {
                                match summary::summarize(
                                    key,
                                    &cfg.summary_model,
                                    &channel_for_post,
                                    &transcript,
                                )
                                .await
                                {
                                    Ok(s) => {
                                        let _ = handle
                                            .privmsg(&channel_for_post, "[transcript] session ended.")
                                            .await;
                                        post_long(&handle, &channel_for_post, &s).await;
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = ?e, "summary failed");
                                        let _ = handle
                                            .privmsg(
                                                &channel_for_post,
                                                &format!(
                                                    "[transcript] session ended; summary failed: {e}"
                                                ),
                                            )
                                            .await;
                                    }
                                }
                            }
                        });
                    }
                    AvAction::Noop => {
                        tracing::debug!(channel = %target, %actor, "av-state");
                    }
                    AvAction::Skip => {}
                }
            }
            Event::Disconnected { reason } => {
                tracing::warn!(%reason, "disconnected");
                return Ok(());
            }
            _ => {}
        }
    }
}

/// Classification of an incoming `+freeq.at/av-state` TAGMSG. Pulled
/// out of [`run`]'s big match so it's unit-testable without standing
/// up a full IRC client.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum AvAction {
    /// Skip this event — wrong target shape, missing tags, not one of
    /// our channels, or the actor is the bot itself (avoid self-loop).
    Skip,
    /// Start transcription for `(channel, session_id)`.
    Start { channel: String, session_id: String },
    /// End transcription for `(channel, session_id)`.
    End { channel: String, session_id: String },
    /// Anything else we don't act on (joined/left/unknown state) but
    /// shouldn't surface as a hard skip — useful for tracing.
    Noop,
}

/// Pure classifier for av-state TAGMSGs. Centralises:
///   - target must be a channel target (`#` / `&`),
///   - required tags must be present,
///   - target must match one of our joined channels (case-insensitive),
///   - we ignore events whose `+freeq.at/av-actor` equals our own
///     nick — the bot's own av-join lands as a TAGMSG too, and without
///     this guard we'd recursively join ourselves.
pub(crate) fn classify_av_event(
    target: &str,
    tags: &std::collections::HashMap<String, String>,
    my_channels: &[String],
    my_nick: &str,
) -> AvAction {
    if !target.starts_with('#') && !target.starts_with('&') {
        return AvAction::Skip;
    }
    let Some(state) = tags.get("+freeq.at/av-state") else { return AvAction::Skip };
    let Some(av_id) = tags.get("+freeq.at/av-id") else { return AvAction::Skip };

    // Self-loop guard: if the actor is us (case-insensitive — IRC nicks
    // are ASCII-case-insensitive), drop the event. Without this, the
    // bot's own av-join would look like a new av-state to itself.
    if let Some(actor) = tags.get("+freeq.at/av-actor") {
        if actor.eq_ignore_ascii_case(my_nick) {
            return AvAction::Skip;
        }
    }

    match state.as_str() {
        "started" => {
            if !my_channels.iter().any(|c| c.eq_ignore_ascii_case(target)) {
                return AvAction::Skip;
            }
            AvAction::Start {
                channel: target.to_string(),
                session_id: av_id.clone(),
            }
        }
        "ended" => AvAction::End {
            channel: target.to_string(),
            session_id: av_id.clone(),
        },
        _ => AvAction::Noop,
    }
}

async fn wait_for_registration(events: &mut tokio::sync::mpsc::Receiver<Event>) -> Result<String> {
    wait_for_registration_with_timeout(events, Duration::from_secs(30)).await
}

/// Timeout-parameterised flavour so tests don't have to wait 30s of
/// wall-clock to exercise the deadline path. Public-in-crate only.
pub(crate) async fn wait_for_registration_with_timeout(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    timeout: Duration,
) -> Result<String> {
    loop {
        match tokio::time::timeout(timeout, events.recv()).await {
            Ok(Some(Event::Registered { nick })) => return Ok(nick),
            Ok(Some(Event::AuthFailed { reason })) => anyhow::bail!("SASL auth failed: {reason}"),
            Ok(Some(_)) => continue,
            Ok(None) => anyhow::bail!("connection closed during registration"),
            Err(_) => anyhow::bail!("registration timeout"),
        }
    }
}

/// Send av-join, open a MoQ subscriber via the SFU, and spawn the
/// audio-tap → whisper → PRIVMSG pipeline. Returns an `ActiveCall`
/// whose `_moq_task` field's drop tears everything down.
async fn start_transcription(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    session_id: String,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<ActiveCall> {
    let instance_id = generate_instance_id();
    let mut tags = HashMap::new();
    tags.insert("+freeq.at/av-join".to_string(), String::new());
    tags.insert("+freeq.at/av-id".to_string(), session_id.clone());
    tags.insert("+freeq.at/av-instance".to_string(), instance_id.clone());
    handle
        .send_tagmsg(&channel, tags)
        .await
        .context("sending av-join")?;

    // Build the MoQ URL. ConnectConfig.server is the IRC server URL;
    // the SFU lives at /av/moq on the same host.
    let sfu_url = sfu_url_from_server(&cfg.server)?;
    let our_broadcast = format!("{session_id}/{}~{instance_id}", cfg.nick);
    let stt = cfg.stt.clone();
    let window_secs = cfg.window_secs;
    let channel_for_task = channel.clone();
    let handle_for_task = handle.clone();
    let active_for_task = active.clone();

    let task = tokio::spawn(async move {
        if let Err(e) = run_moq_subscriber(
            sfu_url,
            our_broadcast,
            channel_for_task,
            stt,
            window_secs,
            handle_for_task,
            active_for_task,
        )
        .await
        {
            tracing::warn!(error = ?e, "MoQ subscriber task ended");
        }
    });

    Ok(ActiveCall {
        channel,
        session_id,
        instance_id,
        transcript: Vec::new(),
        _moq_task: task,
    })
}

/// Long-lived per-call MoQ subscriber. Listens for participant
/// broadcasts and spawns a per-nick tap task for each.
async fn run_moq_subscriber(
    sfu_url: url::Url,
    our_broadcast: String,
    channel: String,
    stt: Arc<Whisper>,
    window_secs: f32,
    handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<()> {
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init()?;

    // We don't publish (silent observer) — but moq-native still wants a
    // publish-side origin. Hand it an empty one.
    let pub_origin = moq_lite::Origin::produce();
    let sub_origin = moq_lite::Origin::produce();
    let mut sub_consumer = sub_origin.consume();

    let session_handle = client
        .with_publish(pub_origin.consume())
        .with_consume(sub_origin)
        .connect(sfu_url)
        .await
        .context("MoQ connect")?;

    tracing::info!("MoQ subscriber connected; watching for participant broadcasts");

    let active_tasks: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    loop {
        tokio::select! {
            announced = sub_consumer.announced() => {
                match announced {
                    Some((path, Some(broadcast_consumer))) => {
                        let path_str = path.to_string();
                        if path_str == our_broadcast {
                            continue;
                        }
                        // Strip the `{session}/` prefix and the
                        // `~instance` suffix to get the display nick.
                        let last = path_str.split('/').last().unwrap_or("unknown");
                        let nick = last.split('~').next().unwrap_or(last).to_string();
                        let key = path_str.clone();
                        {
                            let mut set = active_tasks.lock().unwrap();
                            if !set.insert(key.clone()) {
                                continue;
                            }
                        }
                        tracing::info!(%nick, %path_str, "subscribing to participant");

                        let stt = stt.clone();
                        let channel = channel.clone();
                        let handle = handle.clone();
                        let active = active.clone();
                        tokio::spawn(async move {
                            if let Err(e) = tap_participant(
                                path_str,
                                broadcast_consumer,
                                nick,
                                channel,
                                stt,
                                window_secs,
                                handle,
                                active,
                            )
                            .await
                            {
                                tracing::warn!(error = ?e, "tap task ended");
                            }
                        });
                    }
                    Some((path, None)) => {
                        tracing::info!(path = %path.to_string(), "participant broadcast removed");
                    }
                    None => {
                        tracing::info!("subscription stream closed");
                        break;
                    }
                }
            }
            res = session_handle.closed() => {
                if let Err(e) = res {
                    tracing::warn!(error = ?e, "MoQ session closed");
                }
                break;
            }
        }
    }

    Ok(())
}

/// One per remote broadcast: subscribes to its audio, drains decoded
/// PCM, runs whisper, posts transcript lines.
async fn tap_participant(
    path_str: String,
    broadcast_consumer: moq_lite::BroadcastConsumer,
    nick: String,
    channel: String,
    stt: Arc<Whisper>,
    window_secs: f32,
    handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<()> {
    let remote = RemoteBroadcast::new(&path_str, broadcast_consumer)
        .await
        .context("RemoteBroadcast::new")?;
    let (backend, mut rx) = TapBackend::channel();
    // Hold the audio track alive — dropping it stops the decoder.
    let _audio_track = match remote.audio(&backend).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(%nick, error = ?e, "audio subscribe failed");
            return Ok(());
        }
    };

    let mut buf: Vec<f32> = Vec::new();
    let target_samples = (16_000.0 * window_secs) as usize;

    while let Some(frame) = rx.recv().await {
        let pcm = to_whisper_pcm(&frame.samples, frame.format);
        buf.extend_from_slice(&pcm);
        if buf.len() < target_samples {
            continue;
        }
        // Run whisper on the buffered window. Drain the buffer; we
        // accept a small amount of word-boundary drift in exchange for
        // not re-decoding the same audio twice.
        let chunk = std::mem::take(&mut buf);
        let stt = stt.clone();
        let nick = nick.clone();
        let channel = channel.clone();
        let handle = handle.clone();
        let active = active.clone();
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(move || stt.transcribe(&chunk)).await {
                Ok(Ok(text)) => {
                    if text.is_empty() {
                        return;
                    }
                    let line = format!("[transcript] {nick}: {text}");
                    let _ = handle.privmsg(&channel, &line).await;
                    let log_line = format!("{nick}: {text}");
                    let mut guard = active.lock().await;
                    if let Some(call) = guard.as_mut() {
                        call.transcript.push(log_line);
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(%nick, error = ?e, "whisper failed");
                }
                Err(e) => {
                    tracing::warn!(%nick, error = ?e, "whisper task joined with error");
                }
            }
        });
    }
    Ok(())
}

/// Generate an 8-char hex instance id — same shape as the iOS/web
/// clients use.
pub(crate) fn generate_instance_id() -> String {
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Derive the MoQ SFU URL from the IRC server URL. Same host, /av/moq
/// path, ws/wss based on scheme.
///
/// Adversarial input handling:
///   - empty / whitespace-only string → clean error (was previously
///     producing the bogus URL `ws://`),
///   - garbage like `"://"`, `"ws://"`, `"https://"` (scheme only, no
///     host) → clean error,
///   - any URL we can't extract a non-empty host from → clean error.
pub(crate) fn sfu_url_from_server(server: &str) -> Result<url::Url> {
    let trimmed = server.trim();
    if trimmed.is_empty() {
        anyhow::bail!("server URL is empty");
    }
    let normalized = if trimmed.starts_with("ws://")
        || trimmed.starts_with("wss://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        trimmed.to_string()
    } else {
        // raw host:port — assume non-TLS local dev
        format!("ws://{trimmed}")
    };
    let mut u: url::Url = normalized
        .parse()
        .with_context(|| format!("parsing server URL for SFU: {trimmed:?}"))?;
    // Reject schemes that don't make sense for the SFU. `url::Url`
    // happily accepts `file://`, `mailto:`, etc. — pin the allowed set.
    match u.scheme() {
        "https" => {
            u.set_scheme("wss").ok();
        }
        "http" => {
            u.set_scheme("ws").ok();
        }
        "ws" | "wss" => {}
        other => anyhow::bail!("unsupported scheme for SFU URL: {other:?}"),
    }
    // A URL like `ws://` parses but has an empty host; that would make
    // moq-native connect to nothing. Refuse it here.
    if u.host_str().map(|h| h.is_empty()).unwrap_or(true) {
        anyhow::bail!("server URL has no host: {trimmed:?}");
    }
    u.set_path("/av/moq");
    Ok(u)
}

/// PRIVMSG has a length cap (~400-500 chars depending on prefix length).
/// Split long messages on newlines and post chunks; the summary is
/// usually 2-4 short paragraphs, well under the limit per line.
async fn post_long(handle: &ClientHandle, channel: &str, text: &str) {
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let _ = handle.privmsg(channel, line).await;
        // Brief pacing so we don't flood-trip the server.
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

// Silence the unused-fields lint on ActiveCall — we keep the fields
// even though only `transcript` and `_moq_task` are read by code.
// (`channel`/`session_id`/`instance_id` are useful for diagnostics
// when adding tracing later.)
#[allow(dead_code)]
fn _used(c: &ActiveCall) -> (&str, &str, &str) {
    (&c.channel, &c.session_id, &c.instance_id)
}

// Silence the unused-import lint when the optional `summary` feature is
// the only consumer of HashMap.
#[allow(dead_code)]
fn _hashmap_marker() -> HashMap<String, String> {
    HashMap::new()
}

// Silence PathBuf unused-import warning if we move things around.
#[allow(dead_code)]
fn _pathbuf_marker() -> PathBuf {
    PathBuf::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tokio::sync::mpsc;

    // ---------- sfu_url_from_server ----------

    #[test]
    fn sfu_wss_irc_to_wss_avmoq() {
        let u = sfu_url_from_server("wss://irc.freeq.at/irc").unwrap();
        assert_eq!(u.as_str(), "wss://irc.freeq.at/av/moq");
    }

    #[test]
    fn sfu_https_to_wss() {
        let u = sfu_url_from_server("https://irc.freeq.at").unwrap();
        assert_eq!(u.as_str(), "wss://irc.freeq.at/av/moq");
    }

    #[test]
    fn sfu_http_to_ws() {
        let u = sfu_url_from_server("http://localhost").unwrap();
        assert_eq!(u.as_str(), "ws://localhost/av/moq");
    }

    #[test]
    fn sfu_raw_host_port_to_ws() {
        let u = sfu_url_from_server("localhost:6667").unwrap();
        assert_eq!(u.as_str(), "ws://localhost:6667/av/moq");
    }

    #[test]
    fn sfu_strips_existing_path_and_query() {
        // The bot must replace /irc with /av/moq even when the input
        // URL carries a query string. Without `set_path` this would
        // leak `?token=...` into the SFU URL and break the connect.
        let u = sfu_url_from_server("wss://irc.freeq.at/irc?token=abc").unwrap();
        assert_eq!(u.path(), "/av/moq");
    }

    #[test]
    fn sfu_preserves_nondefault_port() {
        let u = sfu_url_from_server("wss://example.com:8443/irc").unwrap();
        assert_eq!(u.host_str(), Some("example.com"));
        assert_eq!(u.port(), Some(8443));
        assert_eq!(u.path(), "/av/moq");
    }

    #[test]
    fn sfu_trims_surrounding_whitespace() {
        let u = sfu_url_from_server("  wss://irc.freeq.at/irc  ").unwrap();
        assert_eq!(u.as_str(), "wss://irc.freeq.at/av/moq");
    }

    #[test]
    fn sfu_rejects_empty_string() {
        let err = sfu_url_from_server("").err().expect("expected error");
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn sfu_rejects_only_whitespace() {
        let err = sfu_url_from_server("   ").err().expect("expected error");
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn sfu_rejects_scheme_only_garbage() {
        // `wss://` parses as a URL with no host — moq-native would
        // happily connect to the empty string and burn cycles.
        let err = sfu_url_from_server("wss://").err().expect("expected error");
        assert!(format!("{err:#}").contains("host"), "got: {err:#}");
    }

    #[test]
    fn sfu_rejects_double_slash_only() {
        // `://` alone isn't a URL at all.
        let err = sfu_url_from_server("://").err().expect("expected error");
        let s = format!("{err:#}");
        assert!(s.contains("parsing") || s.contains("host"));
    }

    #[test]
    fn sfu_garbage_with_unknown_scheme_does_not_panic() {
        // Inputs that don't start with one of our four supported
        // schemes get treated as `host:port` and prepended with `ws://`.
        // For `file:///etc/passwd` that produces an absurd-but-parsable
        // URL. We only need to assert we don't panic and don't produce
        // a URL that points at an attacker-controlled host.
        let result = sfu_url_from_server("file:///etc/passwd");
        if let Ok(u) = result {
            // If it parses, the host MUST not be "etc" or "passwd" —
            // anything that would let an adversary aim moq-native at
            // a chosen target. In practice the URL becomes
            // ws://file:///etc/passwd which has host == "file".
            assert_eq!(u.host_str(), Some("file"));
            // And the path is rewritten to /av/moq regardless.
            assert_eq!(u.path(), "/av/moq");
        }
    }

    #[test]
    fn sfu_invalid_port_errors() {
        // url::Url rejects this at parse time.
        let err = sfu_url_from_server("wss://example.com:99999/irc")
            .err()
            .expect("expected error");
        assert!(format!("{err:#}").contains("parsing"), "got: {err:#}");
    }

    // ---------- generate_instance_id ----------

    #[test]
    fn instance_id_is_8_lowercase_hex_chars() {
        for _ in 0..200 {
            let id = generate_instance_id();
            assert_eq!(id.len(), 8, "got {id:?}");
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "got {id:?}"
            );
        }
    }

    #[test]
    fn instance_ids_do_not_collide_in_a_thousand_calls() {
        // 4 bytes of randomness ⇒ ~2.3e-5 collision probability over
        // 1000 trials. If we ever drop to 2-byte ids the test starts
        // flaking, which is the warning signal we want.
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(generate_instance_id()));
        }
    }

    // ---------- wait_for_registration ----------

    #[tokio::test]
    async fn registration_succeeds_on_registered_event() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.send(Event::Connected).await.unwrap();
        tx.send(Event::Registered {
            nick: "tbot".to_string(),
        })
        .await
        .unwrap();
        let nick = wait_for_registration_with_timeout(&mut rx, Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(nick, "tbot");
    }

    #[tokio::test]
    async fn registration_surfaces_authfailed_reason_verbatim() {
        // The SASL error message is the only thing telling the user
        // *why* auth was rejected (handle vs key mismatch, expired
        // challenge, etc.) — pin that it bubbles up unmodified.
        let (tx, mut rx) = mpsc::channel(2);
        tx.send(Event::AuthFailed {
            reason: "invalid signature: bad key type".to_string(),
        })
        .await
        .unwrap();
        let err = wait_for_registration_with_timeout(&mut rx, Duration::from_millis(500))
            .await
            .err()
            .expect("expected auth error");
        let s = format!("{err:#}");
        assert!(s.contains("invalid signature: bad key type"), "got: {s}");
        assert!(s.contains("SASL auth failed"), "got: {s}");
    }

    #[tokio::test]
    async fn registration_errors_on_closed_channel() {
        // Disconnect mid-handshake: must not hang and must not panic.
        let (tx, mut rx) = mpsc::channel::<Event>(1);
        drop(tx);
        let err = wait_for_registration_with_timeout(&mut rx, Duration::from_millis(500))
            .await
            .err()
            .expect("expected error");
        assert!(format!("{err:#}").contains("connection closed"), "got: {err:#}");
    }

    #[tokio::test]
    async fn registration_times_out_when_silent() {
        let (_tx, mut rx) = mpsc::channel::<Event>(1);
        let start = std::time::Instant::now();
        let err = wait_for_registration_with_timeout(&mut rx, Duration::from_millis(50))
            .await
            .err()
            .expect("expected timeout");
        assert!(start.elapsed() >= Duration::from_millis(40));
        assert!(format!("{err:#}").contains("timeout"), "got: {err:#}");
    }

    #[tokio::test]
    async fn registration_ignores_intermediate_events() {
        // Pre-registration we may see Connected, Authenticated, etc.
        // None of them should resolve the wait — only Registered does.
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(Event::Connected).await.unwrap();
        tx.send(Event::Authenticated {
            did: "did:key:zfoo".to_string(),
        })
        .await
        .unwrap();
        tx.send(Event::Registered {
            nick: "n".to_string(),
        })
        .await
        .unwrap();
        let nick = wait_for_registration_with_timeout(&mut rx, Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(nick, "n");
    }

    // ---------- classify_av_event ----------

    fn tags(items: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn classify_skips_non_channel_target() {
        // av-state on a direct message — never trust it; we don't
        // 1:1-transcribe.
        let t = tags(&[("+freeq.at/av-state", "started"), ("+freeq.at/av-id", "x")]);
        assert_eq!(
            classify_av_event("alice", &t, &["#room".into()], "tbot"),
            AvAction::Skip
        );
    }

    #[test]
    fn classify_skips_missing_av_id() {
        let t = tags(&[("+freeq.at/av-state", "started")]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::Skip
        );
    }

    #[test]
    fn classify_skips_missing_av_state() {
        let t = tags(&[("+freeq.at/av-id", "x")]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::Skip
        );
    }

    #[test]
    fn classify_skips_self_actor_loop() {
        // The bot's own av-join lands as a TAGMSG. Without this
        // guard the bot would respond to itself and recurse.
        let t = tags(&[
            ("+freeq.at/av-state", "started"),
            ("+freeq.at/av-id", "s1"),
            ("+freeq.at/av-actor", "TBot"),
        ]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::Skip,
            "case-insensitive nick match should self-skip"
        );
    }

    #[test]
    fn classify_does_not_skip_other_actor() {
        let t = tags(&[
            ("+freeq.at/av-state", "started"),
            ("+freeq.at/av-id", "s1"),
            ("+freeq.at/av-actor", "alice"),
        ]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::Start {
                channel: "#room".into(),
                session_id: "s1".into()
            }
        );
    }

    #[test]
    fn classify_skips_started_in_unknown_channel() {
        // We must NOT av-join into channels we aren't a member of —
        // that would let any random user with a +freeq.at/av-state
        // tag drag the bot anywhere.
        let t = tags(&[
            ("+freeq.at/av-state", "started"),
            ("+freeq.at/av-id", "s1"),
        ]);
        assert_eq!(
            classify_av_event("#elsewhere", &t, &["#room".into()], "tbot"),
            AvAction::Skip
        );
    }

    #[test]
    fn classify_channel_match_is_case_insensitive() {
        let t = tags(&[
            ("+freeq.at/av-state", "started"),
            ("+freeq.at/av-id", "s1"),
        ]);
        assert_eq!(
            classify_av_event("#RoOm", &t, &["#room".into()], "tbot"),
            AvAction::Start {
                channel: "#RoOm".into(),
                session_id: "s1".into()
            }
        );
    }

    #[test]
    fn classify_emits_end_for_ended_state() {
        let t = tags(&[
            ("+freeq.at/av-state", "ended"),
            ("+freeq.at/av-id", "s9"),
        ]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::End {
                channel: "#room".into(),
                session_id: "s9".into()
            }
        );
    }

    #[test]
    fn classify_emits_noop_for_unknown_state() {
        // `joined`, `left`, or anything else — we log but don't act.
        // Pin so a careless `_ => AvAction::Start` regression is caught.
        for state in ["joined", "left", "weird"] {
            let t = tags(&[
                ("+freeq.at/av-state", state),
                ("+freeq.at/av-id", "s"),
            ]);
            assert_eq!(
                classify_av_event("#room", &t, &["#room".into()], "tbot"),
                AvAction::Noop,
                "state {state:?}"
            );
        }
    }

    #[test]
    fn classify_ampersand_channel_target_accepted() {
        // `&local` is an IRC local channel prefix; the orchestrator
        // accepts both `#` and `&`.
        let t = tags(&[
            ("+freeq.at/av-state", "ended"),
            ("+freeq.at/av-id", "x"),
        ]);
        assert_eq!(
            classify_av_event("&local", &t, &["#room".into()], "tbot"),
            AvAction::End {
                channel: "&local".into(),
                session_id: "x".into()
            }
        );
    }
}
