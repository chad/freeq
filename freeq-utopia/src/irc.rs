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
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use freeq_sdk::auth::KeySigner;
use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;
use iroh_live::media::codec::AudioCodec;
use iroh_live::media::format::AudioPreset;
use iroh_live::media::publish::LocalBroadcast;
use iroh_live::media::subscribe::RemoteBroadcast;
use rand::RngCore;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::audio_tap::{PushAudioSource, Speaker, TapBackend, to_whisper_pcm};
use crate::identity::Identity;
use crate::stt::SttEngine;
use crate::{qa, summary, tts};

pub struct RunConfig {
    pub server: String,
    pub channels: Vec<String>,
    pub nick: String,
    pub ident: Identity,
    pub stt: Arc<SttEngine>,
    pub window_secs: f32,
    pub summary_model: String,
    pub anthropic_key: Option<String>,
    /// When set, the bot sends an `av-start` for this channel right
    /// after joining — it initiates a call rather than only watching
    /// for one. The channel must also appear in `channels`. The
    /// server's `av-state=started` echo then drives the normal
    /// join/subscribe path.
    pub start_session_in: Option<String>,
    /// Override the MoQ SFU URL. When `None` it's derived from `server`
    /// via [`sfu_url_from_server`]. Set this to the SFU's QUIC port
    /// (e.g. `https://host:4443/av/moq`) to use QUIC instead of the
    /// WebSocket fallback.
    pub sfu_url_override: Option<String>,
    /// Groq API key — powers question-answering (chat). When `None`,
    /// the bot can't answer addressed questions.
    pub groq_api_key: Option<String>,
    /// Groq chat model for answering addressed questions.
    pub groq_chat_model: String,
    /// ElevenLabs API key + voice + model for speaking answers aloud.
    /// When the key is `None`, answers are posted as text only.
    pub elevenlabs_api_key: Option<String>,
    pub elevenlabs_voice_id: String,
    pub elevenlabs_model: String,
}

/// Subset of [`RunConfig`] shared with inner tasks. Excludes the
/// PrivateKey (already moved into the signer) so it's `Clone`-friendly
/// inside an `Arc`.
struct SharedConfig {
    server: String,
    channels: Vec<String>,
    nick: String,
    stt: Arc<SttEngine>,
    window_secs: f32,
    summary_model: String,
    anthropic_key: Option<String>,
    sfu_url_override: Option<String>,
    groq_api_key: Option<String>,
    groq_chat_model: String,
    elevenlabs_api_key: Option<String>,
    elevenlabs_voice_id: String,
    elevenlabs_model: String,
    /// Shared HTTP client for Groq QA + ElevenLabs TTS calls.
    http: reqwest::Client,
}

/// Active-call state. Held inside an `Arc<AsyncMutex<Option<...>>>`
/// because the av-state handler and the av-state=ended handler need
/// to mutate it from different async paths.
struct ActiveCall {
    channel: String,
    session_id: String,
    instance_id: String,
    /// Lines of `<nick>: <utterance>` heard so far. Buffered (not
    /// firehosed to the channel) and used to build the end-of-call
    /// summary + answer `dump` requests.
    transcript: Vec<String>,
    /// Index of the first `transcript` line not yet posted by a `dump`
    /// request — a `dump` posts `transcript[dumped_upto..]` and advances
    /// this so a repeat dump only shows what's new.
    dumped_upto: usize,
    /// Feeds the bot's outbound broadcast — `enqueue` makes it speak.
    speaker: Speaker,
    /// The MoQ subscriber/publisher task. Aborted by `Drop` on call
    /// end — a plain `JoinHandle` drop only *detaches*, which would
    /// leave the reconnect loop running forever after the call ends.
    moq_task: JoinHandle<()>,
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.moq_task.abort();
    }
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
        start_session_in,
        sfu_url_override,
        groq_api_key,
        groq_chat_model,
        elevenlabs_api_key,
        elevenlabs_voice_id,
        elevenlabs_model,
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
        realname: "freeq-utopia".to_string(),
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
            "name": "freeq-utopia",
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
        sfu_url_override,
        groq_api_key,
        groq_chat_model,
        elevenlabs_api_key,
        elevenlabs_voice_id,
        elevenlabs_model,
        http: reqwest::Client::new(),
    });
    let handle_arc = Arc::new(handle);

    // Discover-or-start. If `--start-session-in` is set we want a call
    // running — but a blind `av-start` is rejected by the server when
    // the channel already has an active session (e.g. a previous test,
    // or a human already calling). So: ask the REST API first.
    //   - active session exists → join it directly (av-join + subscribe)
    //   - no session → send av-start; the `av-state=started` echo drives
    //     the subscribe path via the normal Start handler.
    // `self_start` carries the av-start instance so the Start handler
    // reuses it and skips a redundant av-join (no double-appearance).
    let mut self_start: Option<(String, String)> = None;
    if let Some(ref start_ch) = start_session_in {
        if !cfg.channels.iter().any(|c| c.eq_ignore_ascii_case(start_ch)) {
            tracing::warn!(channel = %start_ch, "start-session channel is not in --channel; skipping");
        } else if let Some(session_id) = discover_active_session(&cfg, start_ch).await {
            tracing::info!(channel = %start_ch, %session_id, "joining existing session");
            match start_transcription(
                cfg.clone(),
                handle_arc.clone(),
                start_ch.clone(),
                session_id.clone(),
                None,
                active.clone(),
            )
            .await
            {
                Ok(call) => {
                    *active.lock().await = Some(call);
                    let _ = handle_arc
                        .privmsg(
                            start_ch,
                            "[transcript] listening (joined a call in progress). \
                             Say or type \"utopia: dump\" for the transcript so far.",
                        )
                        .await;
                }
                Err(e) => tracing::warn!(error = ?e, "failed to join existing session"),
            }
        } else {
            let instance = generate_instance_id();
            let mut tags = HashMap::new();
            tags.insert("+freeq.at/av-start".to_string(), String::new());
            tags.insert("+freeq.at/av-instance".to_string(), instance.clone());
            tags.insert(
                "+freeq.at/av-title".to_string(),
                "transcribed session".to_string(),
            );
            handle_arc
                .send_tagmsg(start_ch, tags)
                .await
                .with_context(|| format!("sending av-start to {start_ch}"))?;
            tracing::info!(channel = %start_ch, %instance, "sent av-start — initiating a call");
            self_start = Some((start_ch.to_lowercase(), instance));
        }
    }

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
                        // If this is the session WE started, the av-start
                        // already registered us as the creator participant.
                        // Reuse that instance and skip the redundant
                        // av-join — otherwise the bot occupies two slots
                        // and shows up twice in every client.
                        let existing_instance = match &self_start {
                            Some((ch, inst)) if ch.eq_ignore_ascii_case(&channel) => {
                                Some(inst.clone())
                            }
                            _ => None,
                        };
                        match start_transcription(
                            cfg.clone(),
                            handle_arc.clone(),
                            channel.clone(),
                            session_id.clone(),
                            existing_instance,
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
                                        "[transcript] listening. I'll stay quiet — \
                                         say or type \"utopia: dump\" anytime for \
                                         the transcript so far.",
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
            Event::Message { from, target, text, .. } => {
                // Answer when a participant addresses the bot by name in
                // channel chat. Ignore non-channel targets and our own
                // messages (the bot posts to the channel too).
                if !target.starts_with('#') && !target.starts_with('&') {
                    continue;
                }
                if from.eq_ignore_ascii_case(&cfg.nick) {
                    continue;
                }
                let Some(question) = qa::extract_addressed(&text, &cfg.nick) else {
                    continue;
                };
                // "utopia: dump" → post the buffered transcript
                // rather than running it through Q&A.
                if is_transcript_request(question) {
                    let handle = handle_arc.clone();
                    let active = active.clone();
                    let channel = target.clone();
                    tokio::spawn(async move {
                        dump_transcript(&handle, &channel, &active).await;
                    });
                    continue;
                }
                if cfg.groq_api_key.is_none() {
                    let _ = handle_arc
                        .privmsg(&target, &format!("{from}: Q&A needs a Groq key — not configured."))
                        .await;
                    continue;
                }
                // Snapshot transcript + speaker handle from the active
                // call, then answer + speak off the event loop.
                let (transcript, speaker) = {
                    let guard = active.lock().await;
                    match guard.as_ref() {
                        Some(call) => (call.transcript.join("\n"), Some(call.speaker.clone())),
                        None => (String::new(), None),
                    }
                };
                let cfg = cfg.clone();
                let handle = handle_arc.clone();
                let channel = target.clone();
                let question = question.to_string();
                let asker = from.clone();
                tokio::spawn(async move {
                    answer_and_speak(cfg, handle, channel, asker, question, transcript, speaker)
                        .await;
                });
            }
            Event::Disconnected { reason } => {
                tracing::warn!(%reason, "disconnected");
                return Ok(());
            }
            _ => {}
        }
    }
}

/// Handle one addressed question: ask Groq, post the answer to chat,
/// and (if we're in a call) speak it aloud through the bot's broadcast.
async fn answer_and_speak(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    asker: String,
    question: String,
    transcript: String,
    speaker: Option<Speaker>,
) {
    let Some(key) = cfg.groq_api_key.as_deref() else { return };
    tracing::info!(%asker, %question, "answering addressed question");

    let answer = match qa::answer(
        &cfg.http,
        key,
        &cfg.groq_chat_model,
        &transcript,
        &question,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = ?e, "QA failed");
            let _ = handle
                .privmsg(&channel, &format!("{asker}: sorry — I couldn't answer that ({e})."))
                .await;
            return;
        }
    };

    // Post the text answer regardless of whether speech works.
    let _ = handle
        .privmsg(&channel, &format!("[utopia→{asker}] {answer}"))
        .await;

    // Speak it, if we have a broadcast to speak through + an EL key.
    let Some(speaker) = speaker else {
        tracing::info!("no active call — answered in text only");
        return;
    };
    let Some(el_key) = cfg.elevenlabs_api_key.as_deref() else {
        tracing::info!("no ElevenLabs key — answered in text only");
        return;
    };
    match tts::synthesize(
        &cfg.http,
        el_key,
        &cfg.elevenlabs_voice_id,
        &cfg.elevenlabs_model,
        &answer,
    )
    .await
    {
        Ok(audio) => {
            // Dump the exact synthesized PCM so a "static" report can be
            // bisected: if /tmp/freeq-tts-last.wav plays clean, the
            // static is introduced downstream (Opus encode / WebSocket
            // transport / receiver playout), not by TTS.
            match std::fs::write(
                "/tmp/freeq-tts-last.wav",
                tts::encode_wav(&audio.pcm, audio.sample_rate),
            ) {
                Ok(()) => tracing::info!(
                    samples = audio.pcm.len(),
                    rate = audio.sample_rate,
                    "saved TTS audio to /tmp/freeq-tts-last.wav"
                ),
                Err(e) => tracing::warn!(error = ?e, "could not save TTS debug WAV"),
            }
            speaker.enqueue(&audio.pcm, audio.sample_rate);
            tracing::info!(
                queued_secs = speaker.queued_secs(),
                "spoke answer into the call"
            );
        }
        Err(e) => {
            tracing::warn!(error = ?e, "TTS failed — answer posted as text only");
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
///   - `started` is acted on only for one of our joined channels.
///
/// We deliberately do NOT skip events whose `+freeq.at/av-actor` is the
/// bot's own nick. The bot must react to a session *it* started (the
/// `--start-session-in` flow) — that `av-state=started` is attributed
/// to the bot. There's no self-recursion risk: the bot's own av-join
/// produces an `av-state=joined` broadcast, which maps to `Noop`
/// below (only `started`/`ended` are actioned), and the run loop's
/// `already in a call` guard absorbs any duplicate `started`.
///
/// `my_nick` is retained in the signature for callers/tests; it is no
/// longer used for filtering.
pub(crate) fn classify_av_event(
    target: &str,
    tags: &std::collections::HashMap<String, String>,
    my_channels: &[String],
    _my_nick: &str,
) -> AvAction {
    if !target.starts_with('#') && !target.starts_with('&') {
        return AvAction::Skip;
    }
    let Some(state) = tags.get("+freeq.at/av-state") else { return AvAction::Skip };
    let Some(av_id) = tags.get("+freeq.at/av-id") else { return AvAction::Skip };

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

/// Open a MoQ subscriber via the SFU and spawn the audio-tap → STT →
/// PRIVMSG pipeline. Returns an `ActiveCall` whose `_moq_task` field's
/// drop tears everything down.
///
/// `existing_instance`: when `Some`, the bot is already a participant
/// in this session (it sent the `av-start`), so we reuse that instance
/// and do NOT send an `av-join` — sending one would mint a second slot
/// and the bot would appear in the call twice. When `None` (joining a
/// session someone else started), we mint a fresh instance and join.
async fn start_transcription(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    session_id: String,
    existing_instance: Option<String>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<ActiveCall> {
    let instance_id = match existing_instance {
        Some(inst) => {
            tracing::info!(%inst, "reusing av-start instance — skipping redundant av-join");
            inst
        }
        None => {
            let instance_id = generate_instance_id();
            let mut tags = HashMap::new();
            tags.insert("+freeq.at/av-join".to_string(), String::new());
            tags.insert("+freeq.at/av-id".to_string(), session_id.clone());
            tags.insert("+freeq.at/av-instance".to_string(), instance_id.clone());
            handle
                .send_tagmsg(&channel, tags)
                .await
                .context("sending av-join")?;
            instance_id
        }
    };

    // Build the MoQ URL. Use the explicit override if given (e.g. the
    // SFU's QUIC port), else derive `/av/moq` on the IRC server's host.
    let sfu_url = match &cfg.sfu_url_override {
        Some(u) => u.parse().with_context(|| format!("parsing --sfu-url {u:?}"))?,
        None => sfu_url_from_server(&cfg.server)?,
    };
    let our_broadcast = format!("{session_id}/{}~{instance_id}", cfg.nick);
    let cfg_for_task = cfg.clone();
    let channel_for_task = channel.clone();
    let handle_for_task = handle.clone();
    let active_for_task = active.clone();
    let session_for_task = session_id.clone();

    // Pair a Speaker (kept here) with a PushAudioSource (handed to the
    // MoQ task, which publishes it as the bot's broadcast). Enqueueing
    // on the Speaker makes the bot talk.
    let (speaker, push_source) = Speaker::new();

    let task = tokio::spawn(async move {
        if let Err(e) = run_moq_subscriber(
            cfg_for_task,
            sfu_url,
            session_for_task,
            our_broadcast,
            channel_for_task,
            push_source,
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
        dumped_upto: 0,
        speaker,
        moq_task: task,
    })
}

/// Long-lived MoQ subscriber/publisher with automatic reconnect.
///
/// The MoQ session over the SFU does occasionally drop (network blip,
/// SFU restart, transport idle). Without reconnect the bot would go
/// permanently deaf + mute mid-call. This wraps [`run_moq_session`] in
/// a backoff loop; the only thing that stops it is the whole task
/// being aborted on call-end (see `ActiveCall`'s `Drop`).
#[allow(clippy::too_many_arguments)]
async fn run_moq_subscriber(
    cfg: Arc<SharedConfig>,
    sfu_url: url::Url,
    session_id: String,
    our_broadcast: String,
    channel: String,
    push_source: PushAudioSource,
    handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<()> {
    let mut attempt: u32 = 0;
    loop {
        let started = std::time::Instant::now();
        let result = run_moq_session(
            &cfg,
            &sfu_url,
            &session_id,
            &our_broadcast,
            &channel,
            // Fresh clone per attempt — same shared Speaker queue.
            push_source.clone(),
            &handle,
            &active,
        )
        .await;
        // A session that ran for a healthy while then dropped resets
        // the backoff — only a tight failure loop escalates.
        if started.elapsed() > Duration::from_secs(30) {
            attempt = 0;
        }
        match result {
            Ok(()) => {
                tracing::info!("MoQ subscription stream ended cleanly");
            }
            Err(e) => {
                tracing::warn!(error = ?e, "MoQ session error");
            }
        }
        attempt = attempt.saturating_add(1);
        let backoff = Duration::from_secs(2u64.pow(attempt.min(4))); // 2,4,8,16,16…
        tracing::info!(?backoff, attempt, "reconnecting MoQ session");
        tokio::time::sleep(backoff).await;
    }
}

/// One MoQ session: connect, publish the bot's broadcast, tap every
/// participant, until the transport drops. Tap tasks are owned by a
/// local `JoinSet` so when this function returns (for any reason)
/// they're all aborted — a reconnect starts every tap fresh rather
/// than leaving zombies spinning on a dead transport.
#[allow(clippy::too_many_arguments)]
async fn run_moq_session(
    cfg: &Arc<SharedConfig>,
    sfu_url: &url::Url,
    session_id: &str,
    our_broadcast: &str,
    channel: &str,
    push_source: PushAudioSource,
    handle: &Arc<ClientHandle>,
    active: &Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<()> {
    let my_nick = cfg.nick.clone();
    let session_prefix = format!("{session_id}/");
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init()?;

    // Publish the bot's own broadcast — an Opus stream fed by the
    // PushAudioSource (silence until the bot speaks).
    let broadcast = LocalBroadcast::new();
    broadcast
        .audio()
        .set(push_source, AudioCodec::Opus, [AudioPreset::Hq])
        .context("setting bot broadcast audio source")?;
    let pub_origin = moq_lite::Origin::produce();
    pub_origin.publish_broadcast(our_broadcast, broadcast.consume());

    let sub_origin = moq_lite::Origin::produce();
    let mut sub_consumer = sub_origin.consume();

    let session_handle = client
        .with_publish(pub_origin.consume())
        .with_consume(sub_origin)
        .connect(sfu_url.clone())
        .await
        .context("MoQ connect")?;

    // Keep the encoder alive for the session's lifetime.
    let _broadcast = broadcast;
    tracing::info!(%our_broadcast, "MoQ connected — publishing bot audio + watching participants");

    // Tap tasks live here — dropping the JoinSet on return aborts them.
    let mut taps: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let mut tapped: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            announced = sub_consumer.announced() => {
                match announced {
                    Some((path, Some(broadcast_consumer))) => {
                        let path_str = path.to_string();
                        if path_str == our_broadcast {
                            continue;
                        }
                        // Only tap broadcasts in *our* session — the SFU
                        // announces everything, including stale broadcasts
                        // from prior sessions (no live catalog → "no
                        // audio renditions").
                        if !path_str.starts_with(&session_prefix) {
                            tracing::debug!(%path_str, "skipping broadcast outside our session");
                            continue;
                        }
                        let last = path_str.split('/').last().unwrap_or("unknown");
                        let nick = last.split('~').next().unwrap_or(last).to_string();
                        // Skip the bot's own broadcast — that's our TTS
                        // audio; transcribing it would be a feedback loop.
                        if nick.eq_ignore_ascii_case(&my_nick) {
                            continue;
                        }
                        if !tapped.insert(path_str.clone()) {
                            continue;
                        }
                        tracing::info!(%nick, %path_str, "subscribing to participant");

                        let cfg = cfg.clone();
                        let channel = channel.to_string();
                        let handle = handle.clone();
                        let active = active.clone();
                        taps.spawn(async move {
                            if let Err(e) = tap_participant(
                                cfg,
                                path_str,
                                broadcast_consumer,
                                nick,
                                channel,
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
                        return Ok(()); // subscription stream closed
                    }
                }
            }
            res = session_handle.closed() => {
                anyhow::bail!("MoQ transport closed: {res:?}");
            }
        }
    }
}

// ── Voice-activity segmentation tuning ──────────────────────────────
// All in 16 kHz mono samples / amplitude units.

/// Peak amplitude above which a chunk counts as speech. Mic silence /
/// room noise sits well under this; conversational speech peaks far
/// above it. Deliberately low so we don't clip quiet talkers.
const VOICE_PEAK_THRESHOLD: f32 = 0.018;
/// A pause this long ends an utterance and triggers a flush. Long
/// enough to ride over the gaps between words, short enough that the
/// post still feels rapid. 0.6s @ 16 kHz.
const SILENCE_GAP_SAMPLES: usize = (16_000.0 * 0.6) as usize;
/// Hard cap on an utterance — flush even mid-speech so a monologue
/// doesn't accumulate unbounded latency. 22s @ 16 kHz.
const MAX_UTTERANCE_SAMPLES: usize = 16_000 * 22;
/// Don't bother transcribing an utterance with less than this much
/// actual voiced audio — it's a cough / click / room noise. 0.35s.
const MIN_VOICED_SAMPLES: usize = (16_000.0 * 0.35) as usize;

/// Known Whisper silence/noise hallucinations. Even with VAD, a short
/// burst of non-speech noise occasionally slips a window through;
/// these are the canonical phantom outputs across Whisper variants.
/// An exact (case/punctuation-insensitive) match is dropped.
fn is_hallucination(text: &str) -> bool {
    let t = text
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_lowercase();
    matches!(
        t.as_str(),
        "" | "you"
            | "thank you"
            | "thanks for watching"
            | "thank you for watching"
            | "bye"
            | "okay"
            | "so"
            | "the"
    )
}

/// True when an addressed utterance is asking the bot to post the
/// transcript buffered so far, rather than answer a question. The bot
/// no longer firehoses every utterance to the channel — people pull the
/// transcript on demand with "utopia: dump". Matched loosely
/// because it's typed *and* spoken (whisper punctuation varies).
pub(crate) fn is_transcript_request(question: &str) -> bool {
    let q = question
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_lowercase();
    q == "dump" || q.starts_with("dump ") || q.contains("transcript")
}

/// One per remote broadcast: subscribes to its audio and segments it
/// into utterances by voice activity — accumulate while the speaker is
/// talking, flush to STT on a natural pause. This kills both the
/// "Thank you." silence hallucinations (silent stretches never reach
/// STT) and the mid-sentence splits (we cut at pauses, not on a fixed
/// clock).
#[allow(clippy::too_many_arguments)]
async fn tap_participant(
    cfg: Arc<SharedConfig>,
    path_str: String,
    broadcast_consumer: moq_lite::BroadcastConsumer,
    nick: String,
    channel: String,
    handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> Result<()> {
    let stt = cfg.stt.clone();
    let remote = RemoteBroadcast::new(&path_str, broadcast_consumer)
        .await
        .context("RemoteBroadcast::new")?;
    let (backend, mut rx) = TapBackend::channel();
    // `audio_ready()` blocks on the catalog watcher until the broadcast
    // advertises an audio rendition, then subscribes. The plain
    // `audio()` is a one-shot catalog read — a participant who joined
    // before their mic catalog populated (or whose Opus track lands a
    // beat after the broadcast is announced) fails permanently with
    // "no audio renditions". Same race we hit on the video path.
    let _audio_track = match remote.audio_ready(&backend).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(%nick, error = ?e, "audio subscribe failed");
            return Ok(());
        }
    };
    tracing::info!(%nick, "audio track live — transcribing");

    // Utterance accumulator + VAD state.
    let mut buf: Vec<f32> = Vec::new();
    let mut voiced_samples: usize = 0;
    let mut trailing_silence: usize = 0;
    let mut frames_seen: u64 = 0;

    while let Some(frame) = rx.recv().await {
        frames_seen += 1;
        let pcm = to_whisper_pcm(&frame.samples, frame.format);
        if pcm.is_empty() {
            continue;
        }
        let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let voiced = peak >= VOICE_PEAK_THRESHOLD;

        if voiced {
            buf.extend_from_slice(&pcm);
            voiced_samples += pcm.len();
            trailing_silence = 0;
        } else if !buf.is_empty() {
            // Mid-utterance silence: keep it in the buffer (the pause is
            // part of natural speech and helps STT) and count it toward
            // the end-of-utterance gap.
            buf.extend_from_slice(&pcm);
            trailing_silence += pcm.len();
        }
        // else: pre-speech silence — drop it, never accumulate.

        if frames_seen == 1 || frames_seen.is_multiple_of(250) {
            tracing::info!(
                %nick, frames_seen, buffered = buf.len(), voiced_samples, peak,
                in_rate = frame.format.sample_rate,
                in_channels = frame.format.channel_count,
                "audio tap heartbeat"
            );
        }

        let pause_flush = trailing_silence >= SILENCE_GAP_SAMPLES && !buf.is_empty();
        let cap_flush = buf.len() >= MAX_UTTERANCE_SAMPLES;
        if !pause_flush && !cap_flush {
            continue;
        }

        let chunk = std::mem::take(&mut buf);
        let chunk_voiced = voiced_samples;
        voiced_samples = 0;
        trailing_silence = 0;

        // Skip utterances that are basically noise — too little actual
        // speech to be worth a round-trip (and a prime hallucination
        // source).
        if chunk_voiced < MIN_VOICED_SAMPLES {
            tracing::debug!(%nick, chunk_voiced, "skipping low-voice utterance");
            continue;
        }

        let stt = stt.clone();
        let nick = nick.clone();
        let channel = channel.clone();
        let handle = handle.clone();
        let active = active.clone();
        let cfg = cfg.clone();
        // `SttEngine::transcribe` is async — Groq is an HTTP round-trip,
        // local whisper does its own spawn_blocking internally. One task
        // per utterance so a slow STT call doesn't stall the tap loop.
        tokio::spawn(async move {
            match stt.transcribe(&chunk).await {
                Ok(text) => {
                    if text.is_empty() || is_hallucination(&text) {
                        tracing::info!(%nick, %text, "dropped empty/hallucinated utterance");
                        return;
                    }
                    tracing::info!(%nick, %text, "transcribed utterance");

                    // Voice-addressed Q&A: if the utterance starts with
                    // the bot's name ("utopia, summarize…"), treat
                    // it as a spoken question — answer + speak back —
                    // instead of just logging it as a transcript line.
                    // In a voice call people address the bot by talking,
                    // not typing.
                    if let Some(question) = qa::extract_addressed(&text, &cfg.nick) {
                        if is_transcript_request(question) {
                            dump_transcript(&handle, &channel, &active).await;
                            return;
                        }
                        let _ = handle
                            .privmsg(&channel, &format!("[transcript] {nick} asked: {text}"))
                            .await;
                        let (transcript, speaker) = {
                            let guard = active.lock().await;
                            match guard.as_ref() {
                                Some(call) => {
                                    (call.transcript.join("\n"), Some(call.speaker.clone()))
                                }
                                None => (String::new(), None),
                            }
                        };
                        answer_and_speak(
                            cfg,
                            handle,
                            channel,
                            nick,
                            question.to_string(),
                            transcript,
                            speaker,
                        )
                        .await;
                        return;
                    }

                    // Buffer the line — the bot no longer firehoses every
                    // utterance to the channel. A `dump` request posts
                    // what's accumulated.
                    let log_line = format!("{nick}: {text}");
                    let mut guard = active.lock().await;
                    if let Some(call) = guard.as_mut() {
                        call.transcript.push(log_line);
                    }
                }
                Err(e) => {
                    tracing::warn!(%nick, error = ?e, "STT failed");
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
/// path, `https`/`http` scheme.
///
/// The scheme matters for transport selection. moq-native races a QUIC
/// (WebTransport) connection against a WebSocket fallback and keeps the
/// first to succeed. Its QUIC backend only accepts `https`/`moqt`/`moql`
/// — a `wss` URL is rejected outright, so the bot would silently drop to
/// the WebSocket fallback. WebSocket runs over TCP, whose head-of-line
/// blocking turns any packet loss into bursty delivery, which the
/// receiver hears as "bad-radio" static on the bot's audio. Emitting
/// `https` puts QUIC (the proper low-latency media transport) back in
/// the race; the WebSocket fallback accepts `https`/`http` too, so this
/// costs nothing if QUIC is unavailable.
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
    // Normalize to `https`/`http` so moq-native can attempt QUIC (see
    // the doc comment above).
    match u.scheme() {
        "https" | "wss" => {
            u.set_scheme("https").ok();
        }
        "http" | "ws" => {
            u.set_scheme("http").ok();
        }
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

/// Derive the REST API base (`https://host[:port]`) from the IRC
/// server URL. `wss://host/irc` → `https://host`; `host:port` →
/// `http://host:port`.
pub(crate) fn api_base_from_server(server: &str) -> Result<String> {
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
        format!("ws://{trimmed}")
    };
    let u: url::Url = normalized
        .parse()
        .with_context(|| format!("parsing server URL for REST API: {trimmed:?}"))?;
    let scheme = match u.scheme() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        other => anyhow::bail!("unsupported scheme for REST API: {other:?}"),
    };
    let host = u.host_str().context("server URL has no host")?;
    Ok(match u.port() {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Query the REST API for an active AV session in `channel`. Returns
/// its session id if one is running, `None` otherwise (incl. on any
/// network/parse error — we then fall back to starting a fresh call).
async fn discover_active_session(cfg: &SharedConfig, channel: &str) -> Option<String> {
    let base = api_base_from_server(&cfg.server).ok()?;
    let encoded: String = channel
        .bytes()
        .map(|b| {
            if b == b'#' {
                "%23".to_string()
            } else {
                (b as char).to_string()
            }
        })
        .collect();
    let url = format!("{base}/api/v1/channels/{encoded}/sessions");
    let resp = cfg
        .http
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let active = json.get("active")?;
    if active.is_null() {
        return None;
    }
    let state = active.get("state").and_then(|s| s.as_str()).unwrap_or("");
    if state != "Active" {
        return None;
    }
    active
        .get("id")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
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

/// Post the transcript accumulated since the last dump to `channel`.
/// Called on an explicit `dump` request — this replaces the old
/// one-PRIVMSG-per-utterance firehose. Advances `dumped_upto` so a
/// repeated dump only shows what's new since the previous one.
async fn dump_transcript(
    handle: &ClientHandle,
    channel: &str,
    active: &Arc<AsyncMutex<Option<ActiveCall>>>,
) {
    // Snapshot the new lines while holding the lock; post outside it.
    let snapshot: Option<(Vec<String>, usize)> = {
        let mut guard = active.lock().await;
        guard.as_mut().map(|call| {
            let total = call.transcript.len();
            let from = call.dumped_upto.min(total);
            let new = call.transcript[from..].to_vec();
            call.dumped_upto = total;
            (new, total)
        })
    };
    match snapshot {
        None => {
            let _ = handle
                .privmsg(channel, "[transcript] no active call right now.")
                .await;
        }
        Some((new, total)) if new.is_empty() => {
            let msg = if total == 0 {
                "[transcript] nothing transcribed yet."
            } else {
                "[transcript] nothing new since the last dump."
            };
            let _ = handle.privmsg(channel, msg).await;
        }
        Some((new, _)) => {
            let _ = handle
                .privmsg(
                    channel,
                    &format!("[transcript] {} line(s) since the last dump:", new.len()),
                )
                .await;
            for line in &new {
                let _ = handle.privmsg(channel, &format!("  {line}")).await;
                // Brief pacing so we don't flood-trip the server.
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        }
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
    fn sfu_wss_irc_to_https_avmoq() {
        // `wss` IRC URL → `https` SFU URL so moq-native attempts QUIC
        // rather than skipping straight to the WebSocket fallback.
        let u = sfu_url_from_server("wss://irc.freeq.at/irc").unwrap();
        assert_eq!(u.as_str(), "https://irc.freeq.at/av/moq");
    }

    #[test]
    fn sfu_https_stays_https() {
        let u = sfu_url_from_server("https://irc.freeq.at").unwrap();
        assert_eq!(u.as_str(), "https://irc.freeq.at/av/moq");
    }

    #[test]
    fn sfu_http_stays_http() {
        let u = sfu_url_from_server("http://localhost").unwrap();
        assert_eq!(u.as_str(), "http://localhost/av/moq");
    }

    #[test]
    fn sfu_raw_host_port_to_http() {
        let u = sfu_url_from_server("localhost:6667").unwrap();
        assert_eq!(u.as_str(), "http://localhost:6667/av/moq");
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
        assert_eq!(u.as_str(), "https://irc.freeq.at/av/moq");
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
    fn classify_acts_on_self_started_session() {
        // The bot must act on a session IT started (--start-session-in):
        // the server attributes that `av-state=started` to the bot's
        // own nick. Self-recursion is not a risk — the subsequent
        // av-join surfaces as `av-state=joined`, which is Noop.
        let t = tags(&[
            ("+freeq.at/av-state", "started"),
            ("+freeq.at/av-id", "s1"),
            ("+freeq.at/av-actor", "TBot"),
        ]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::Start {
                channel: "#room".into(),
                session_id: "s1".into(),
            },
            "bot-initiated session start must be acted on, not self-skipped"
        );
    }

    #[test]
    fn classify_self_actor_joined_is_noop() {
        // The bot's own av-join → server broadcasts av-state=joined
        // attributed to the bot. Must be Noop, never a re-trigger.
        let t = tags(&[
            ("+freeq.at/av-state", "joined"),
            ("+freeq.at/av-id", "s1"),
            ("+freeq.at/av-actor", "tbot"),
        ]);
        assert_eq!(
            classify_av_event("#room", &t, &["#room".into()], "tbot"),
            AvAction::Noop,
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

    // ---------- is_transcript_request ----------

    #[test]
    fn transcript_request_matches_dump_phrasings() {
        for q in [
            "dump",
            "dump.",
            "Dump!",
            "dump it",
            "dump the transcript",
            "dump everything",
            "transcript",
            "transcript please",
            "show me the transcript",
            "post the transcript",
            "what's the transcript so far",
        ] {
            assert!(is_transcript_request(q), "should match: {q:?}");
        }
    }

    #[test]
    fn transcript_request_rejects_real_questions() {
        for q in [
            "",
            "what time is it",
            "summarize the action items",
            "who said that",
            "how are you",
            "dumpling recipe", // 'dump' must be a whole word, not a prefix
        ] {
            assert!(!is_transcript_request(q), "should not match: {q:?}");
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
