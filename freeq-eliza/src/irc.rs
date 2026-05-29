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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use freeq_agent_kit::{
    extract_addressed, is_hallucination, split_speech_and_links, VadConfig, VadSegmenter,
};
use freeq_av::{broadcast_path, AvConfig, AvParticipant, AvSession, Speaker, VideoHandle};
use freeq_sdk::auth::KeySigner;
use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::{JoinHandle, JoinSet};

use crate::identity::Identity;

/// Try to extract an addressed question from `text`. Accepts both the
/// active nick AND "eliza" as wake words — so a user with muscle
/// memory who runs as Oblivion / Narrator / Utopia and says "Eliza,
/// what is X?" still gets through. The character's own name still
/// works too. Returns `None` if neither matches.
fn address_with_aliases(text: &str, nick: &str) -> Option<String> {
    if let Some(q) = extract_addressed(text, nick) {
        return Some(q);
    }
    // Server-suffixed nicks: a fresh DID gets bound as e.g.
    // `oblivion-z6mkfa8x`. Humans address the bot by its character
    // name ("oblivion"), so try the pre-dash prefix as an alias when
    // it differs from the full nick.
    if let Some(prefix) = nick.split_once('-').map(|(p, _)| p) {
        if prefix.len() >= 4 && !prefix.eq_ignore_ascii_case(nick) {
            if let Some(q) = extract_addressed(text, prefix) {
                return Some(q);
            }
        }
    }
    // No universal "eliza" wake word: in a multi-agent room (Oblivion +
    // Utopia + Narrator), a generic fallback causes every bot to
    // answer every question. Each bot replies only to its own
    // character name.
    None
}

/// Multi-agent chatter guard. Records `asker` in the rolling
/// addressing-chain history and returns `false` if the recent K
/// addressers are *all* peer agents — that's the loop signature.
/// As soon as a human addresses the bot, the streak resets and
/// the next address goes through. Allows up to 2 bot-to-bot
/// exchanges without a human break (so a real "Oblivion, what do
/// you think?" → "Utopia, ..." → "Oblivion, ..." exchange lands)
/// and stops at the 3rd.
fn is_address_allowed(cfg: &SharedConfig, asker: &str) -> bool {
    const HISTORY_KEEP: usize = 5;
    let asker_lc = asker.to_ascii_lowercase();
    let mut chain = cfg.addressing_chain.lock().expect("addressing chain poisoned");
    chain.push_back(asker_lc.clone());
    while chain.len() > HISTORY_KEEP {
        chain.pop_front();
    }
    // Lone agent (no peers configured) → always allow.
    if cfg.peer_agents.is_empty() {
        return true;
    }
    // Discussion mode: when a human just said "discuss it" /
    // "debate this", peer↔peer replies are temporarily allowed so
    // the agents can converse with each other for ~90 s. Outside the
    // window the strict policy below applies.
    if let Ok(deadline) = cfg.discussion_until.lock() {
        if Instant::now() < *deadline {
            return true;
        }
    }
    // Multi-agent room: only humans address agents directly. If the
    // current addresser is a known peer agent, suppress — peer ↔ peer
    // exchanges spiral too easily (one bot's reply mentions another
    // by name, which the LLM is happy to do despite the prompt rule
    // against it, and the loop is off).
    !is_peer_nick(&cfg.peer_agents, &asker_lc)
}

/// True if `nick` matches one of `peers` either exactly or by the
/// pre-dash prefix. The server suffixes fresh DIDs with `-<bs58>` so
/// `oblivion-z6mkfa8x` should still match a configured peer of
/// `"oblivion"`. Case-insensitive (`peers` are lowercased on load).
/// True if the operator has armed peer-conversation mode within the
/// last 90 s. Read by `answer_and_speak` (to inject a hand-off
/// instruction into the LLM prompt) and by `is_address_allowed` (to
/// let peer agents reply to each other while the window is open).
fn is_discussion_mode_active(cfg: &SharedConfig) -> bool {
    cfg.discussion_until
        .lock()
        .map(|d| Instant::now() < *d)
        .unwrap_or(false)
}

fn is_peer_nick(peers: &std::collections::HashSet<String>, nick: &str) -> bool {
    let nick_lc = nick.to_ascii_lowercase();
    if peers.contains(&nick_lc) {
        return true;
    }
    if let Some((prefix, _)) = nick_lc.split_once('-') {
        if peers.contains(prefix) {
            return true;
        }
    }
    false
}
use crate::imagegen::AiImageConfig;
use crate::stt::{to_whisper_pcm, SttEngine};
use crate::video::VideoTile;
use crate::whiteboard::Step;
use crate::{imagegen, qa, summary, tts, vision};

pub struct RunConfig {
    pub server: String,
    pub channels: Vec<String>,
    pub nick: String,
    pub ident: Identity,
    pub stt: Arc<SttEngine>,
    pub window_secs: f32,
    pub summary_model: String,
    pub anthropic_key: Option<String>,
    /// Whether the end-of-call summary path runs (separate from the
    /// per-question answer-model dispatch, which gates on
    /// [`Self::anthropic_key`] presence and the model name).
    pub summary_enabled: bool,
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
    /// Groq chat model for the visual board (scene generation).
    pub groq_chat_model: String,
    /// Groq model for answering addressed questions. Defaults to an
    /// agentic model (`groq/compound`) so eliza can search the web.
    pub groq_answer_model: String,
    /// Groq vision model for questions about a participant's shared
    /// screen or camera.
    pub vision_model: String,
    /// ElevenLabs API key + voice + model for speaking answers aloud.
    /// When the key is `None`, answers are posted as text only.
    pub elevenlabs_api_key: Option<String>,
    pub elevenlabs_voice_id: String,
    pub elevenlabs_model: String,
    /// AI image-generation fallback for scene backdrops. `None` leaves
    /// Wikipedia as the only backdrop source.
    pub image_ai: Option<AiImageConfig>,
    /// Enable the proactive monitor — when true, Eliza chimes in
    /// unprompted with high-confidence observations. Toggle with
    /// `--no-proactive` on the CLI.
    pub proactive_enabled: bool,
    /// Enable the ambient monitor — when true, Eliza's tile silently
    /// reflects the topic + colour of the conversation while she
    /// listens, and escalates to an image scene on concrete subjects.
    /// Toggle with `--no-ambient` on the CLI.
    pub ambient_enabled: bool,
    /// Video tile renderer choice. `svg` = the rich freeq presence;
    /// `particles` = ghostly particle face.
    pub render_backend: String,
    /// Ghostly character name when `render_backend == "particles"`.
    pub ghostly_character: String,
    /// Per-character system-prompt override (Oblivion / Narrator /
    /// Utopia personality). `None` falls back to the default Eliza
    /// prompt in `qa.rs`.
    pub character_system_prompt: Option<String>,
    /// Other agent nicks in the channel. When set, the bot can hold a
    /// bounded multi-agent dialogue (e.g. Oblivion + Utopia debating)
    /// but won't run away: after a streak of bot-to-bot exchanges
    /// without a human break, the bot stops responding until a human
    /// addresses it again.
    pub peer_agents: Vec<String>,
}

/// Subset of [`RunConfig`] shared with inner tasks. Excludes the
/// PrivateKey (already moved into the signer) so it's `Clone`-friendly
/// inside an `Arc`. `pub(crate)` so the [`proactive`](crate::proactive)
/// monitor can read the same config.
pub(crate) struct SharedConfig {
    pub(crate) server: String,
    pub(crate) channels: Vec<String>,
    pub(crate) nick: String,
    pub(crate) stt: Arc<SttEngine>,
    pub(crate) window_secs: f32,
    pub(crate) summary_model: String,
    pub(crate) anthropic_key: Option<String>,
    pub(crate) summary_enabled: bool,
    pub(crate) sfu_url_override: Option<String>,
    pub(crate) groq_api_key: Option<String>,
    pub(crate) groq_chat_model: String,
    pub(crate) groq_answer_model: String,
    pub(crate) vision_model: String,
    pub(crate) elevenlabs_api_key: Option<String>,
    pub(crate) elevenlabs_voice_id: String,
    pub(crate) elevenlabs_model: String,
    pub(crate) image_ai: Option<AiImageConfig>,
    /// Shared HTTP client for Groq QA + ElevenLabs TTS calls.
    pub(crate) http: reqwest::Client,
    /// When the bot process started — drives a startup grace period so it
    /// doesn't answer the burst of channel history (and any replayed
    /// audio) the server delivers right after it joins.
    pub(crate) started_at: Instant,
    /// Whether the proactive monitor runs (`--no-proactive` disables it).
    pub(crate) proactive_enabled: bool,
    /// Whether the ambient monitor runs (`--no-ambient` disables it).
    pub(crate) ambient_enabled: bool,
    /// Renderer choice — `"svg"` (default) or `"particles"`.
    pub(crate) render_backend: String,
    /// Ghostly character name when `render_backend == "particles"`.
    pub(crate) ghostly_character: String,
    /// Per-character system prompt — when present, replaces the
    /// default Eliza prompt in [`qa::answer_streaming`].
    pub(crate) character_system_prompt: Option<String>,
    /// Lowercased nicks of OTHER agents in the channel — peers this
    /// bot recognises by name. Used to prevent multi-agent runaway: a
    /// bot can engage with another bot when called, but won't keep
    /// chaining bot-to-bot replies without a human breaking in. When
    /// empty (the default), this bot acts alone.
    pub(crate) peer_agents: std::collections::HashSet<String>,
    /// Rolling history of the last ~5 addressers (lowercased). When
    /// the recent K (3) are all peer agents, this bot suppresses its
    /// reply — that breaks reply loops between bots. A human
    /// addressing the bot resets the streak immediately.
    pub(crate) addressing_chain:
        std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    /// Persistent conversation memory — past exchanges queryable via
    /// FTS5. Retrieved before each answer (top-K relevant) and stored
    /// after. `None` if a memory DB couldn't be opened (the bot will
    /// just run without recall).
    pub(crate) memory: Option<std::sync::Arc<crate::memory::Memory>>,
    /// Per-channel decision log — commitments extracted from the live
    /// transcript ("let's ship Friday", "I'll handle the deploy"). Read
    /// back to the channel when the session ends so the room has a
    /// captured summary of what it actually decided. Empty between
    /// sessions; the End handler drains the entry for its channel.
    pub(crate) decisions: std::sync::Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, Vec<crate::decisions::Decision>>,
        >,
    >,
    /// Per-channel live diagram — accumulating graph of concepts +
    /// relationships extracted from every transcribed utterance. The
    /// transcribe loop ingests text into the channel's entry; when
    /// new edges appear, the rendered steps are pushed to the
    /// whiteboard. Cleared when the session ends.
    pub(crate) diagrams: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, crate::diagram::Diagram>>,
    >,
    /// Deadline (Instant) until which the strict human-only-address
    /// policy is relaxed and bots may answer each other freely. A
    /// human speaking the discussion trigger ("discuss it", "debate
    /// this", …) pushes this 90 s into the future; otherwise it
    /// stays in the past and the strict policy applies.
    pub(crate) discussion_until: std::sync::Arc<std::sync::Mutex<Instant>>,
}

/// Active-call state. Held inside an `Arc<AsyncMutex<Option<...>>>`
/// because the av-state handler and the av-state=ended handler need
/// to mutate it from different async paths. `pub(crate)` so the
/// proactive monitor can snapshot the transcript + speaker.
pub(crate) struct ActiveCall {
    pub(crate) channel: String,
    pub(crate) session_id: String,
    pub(crate) instance_id: String,
    /// Lines of `<nick>: <utterance>` heard so far. Buffered as context
    /// for answering questions and the end-of-call summary — never
    /// posted to the channel.
    pub(crate) transcript: Vec<String>,
    /// When Eliza last dispatched a spoken answer (or proactive comment).
    /// Drives a debounce so one question — transcribed once per broadcast
    /// when a speaker is joined from several devices — is answered only
    /// once, and so the proactive monitor doesn't pile on right after
    /// she just spoke.
    pub(crate) last_answer: Option<Instant>,
    /// Feeds the bot's outbound broadcast — `enqueue` makes it speak.
    pub(crate) speaker: Speaker,
    /// The agent's video tile (audio-reactive presence + visual-aid
    /// cards). `show_card` puts up an LLM-drawn visual.
    pub(crate) video: VideoTile,
    /// The MoQ subscriber/publisher task. Aborted by `Drop` on call
    /// end — a plain `JoinHandle` drop only *detaches*, which would
    /// leave the reconnect loop running forever after the call ends.
    moq_task: JoinHandle<()>,
    /// The proactive-monitor task (if enabled). Same drop story.
    proactive_task: Option<JoinHandle<()>>,
    /// The ambient-monitor task (if enabled). Same drop story.
    ambient_task: Option<JoinHandle<()>>,
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.moq_task.abort();
        if let Some(t) = &self.proactive_task {
            t.abort();
        }
        if let Some(t) = &self.ambient_task {
            t.abort();
        }
        self.video.stop();
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
        summary_enabled,
        start_session_in,
        sfu_url_override,
        groq_api_key,
        groq_chat_model,
        groq_answer_model,
        vision_model,
        elevenlabs_api_key,
        elevenlabs_voice_id,
        elevenlabs_model,
        image_ai,
        proactive_enabled,
        ambient_enabled,
        render_backend,
        ghostly_character,
        character_system_prompt,
        peer_agents,
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
        realname: "freeq-eliza".to_string(),
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
            "name": "freeq-eliza",
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

    // Persistent conversation memory — per-bot SQLite at
    // ~/.freeq/bots/<name>/memory.db. Soft failure: if it can't open,
    // the bot runs without recall.
    let memory = dirs::home_dir()
        .map(|h| h.join(".freeq").join("bots").join(&nick).join("memory.db"))
        .and_then(|p| match crate::memory::Memory::open(&p) {
            Ok(m) => {
                tracing::info!(path = %p.display(), "memory store ready");
                Some(std::sync::Arc::new(m))
            }
            Err(e) => {
                tracing::warn!(path = %p.display(), error = ?e, "failed to open memory store — bot will run without recall");
                None
            }
        });

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
        summary_enabled,
        sfu_url_override,
        groq_api_key,
        groq_chat_model,
        groq_answer_model,
        vision_model,
        elevenlabs_api_key,
        elevenlabs_voice_id,
        elevenlabs_model,
        image_ai,
        proactive_enabled,
        ambient_enabled,
        render_backend,
        ghostly_character,
        character_system_prompt,
        peer_agents: peer_agents
            .iter()
            .map(|n| n.to_ascii_lowercase())
            .collect(),
        addressing_chain: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::VecDeque::new(),
        )),
        memory,
        decisions: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        diagrams: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        discussion_until: std::sync::Arc::new(std::sync::Mutex::new(
            Instant::now() - Duration::from_secs(3600),
        )),
        http: reqwest::Client::new(),
        started_at: Instant::now(),
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
                    spawn_hello_on_join(
                        &cfg,
                        call.speaker.clone(),
                        call.video.peer_level_handle(),
                    );
                    *active.lock().await = Some(call);
                }
                Err(e) => tracing::warn!(error = ?e, "failed to join existing session"),
            }
        } else {
            let instance = freeq_sdk::av::new_av_instance();
            handle_arc
                .av_start(start_ch, &instance, Some("transcribed session"))
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
                                // Debugging affordance: speak a short,
                                // in-character greeting the moment the
                                // call is live. Lets the operator hear
                                // which agents are alive (and which
                                // aren't) without typing anything. Fires
                                // only once per call — the speaker
                                // clone keeps the audio queued even if
                                // the bot's task panics elsewhere.
                                spawn_hello_on_join(
                                    &cfg,
                                    call.speaker.clone(),
                                    call.video.peer_level_handle(),
                                );
                                // Backchannels: listen-mode "mm" /
                                // "right" while a peer is talking.
                                // Aborts when the call's MoQ task
                                // drops (the speaker handle stops
                                // accepting enqueues).
                                let _ = spawn_backchannel_loop(
                                    cfg.clone(),
                                    call.speaker.clone(),
                                    call.video.peer_level_handle(),
                                );
                                *active_guard = Some(call);
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

                        // Decision read-back: drain the per-channel
                        // decision log and post it to the channel. The
                        // room hears what it actually committed to
                        // without anyone taking notes — the demo
                        // proof-of-concept for "conversation as the
                        // source of knowledge work".
                        let drained: Vec<crate::decisions::Decision> = cfg
                            .decisions
                            .lock()
                            .ok()
                            .and_then(|mut g| g.remove(&channel_for_post))
                            .unwrap_or_default();
                        if !drained.is_empty() {
                            let _ = handle
                                .privmsg(
                                    &channel_for_post,
                                    "[eliza] decisions captured this session:",
                                )
                                .await;
                            for d in &drained {
                                let line = match &d.when {
                                    Some(w) => format!(
                                        "[eliza]  • {} — {} (by {})",
                                        d.who, d.what, w
                                    ),
                                    None => format!("[eliza]  • {} — {}", d.who, d.what),
                                };
                                let _ = handle
                                    .privmsg(&channel_for_post, &line)
                                    .await;
                            }
                        }

                        // Clear the live diagram so the next session
                        // starts on a blank canvas.
                        if let Ok(mut g) = cfg.diagrams.lock() {
                            g.remove(&channel_for_post);
                        }

                        if !cfg.summary_enabled || !cfg.anthropic_key.is_some() || transcript.is_empty() {
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
                // Shared whiteboard: peer agents emit "[diag] X|R|Y"
                // bullets to broadcast new edges. We parse those into
                // our local diagram so every tile draws the same
                // whiteboard. Skip the address path entirely for these.
                if let Some(rest) = text.strip_prefix("[diag] ") {
                    if is_peer_nick(&cfg.peer_agents, &from) {
                        let parts: Vec<&str> = rest.splitn(3, '|').collect();
                        if parts.len() == 3 {
                            if let Ok(mut log) = cfg.diagrams.lock() {
                                let d = log.entry(target.clone()).or_default();
                                let sentence =
                                    format!("{} {} {}", parts[0], parts[1], parts[2]);
                                if d.ingest(&sentence) > 0 {
                                    let steps = d.to_steps();
                                    if !steps.is_empty() {
                                        if let Some(call) = active.lock().await.as_ref() {
                                            call.video.show_board(
                                                steps,
                                                "#7FE7CB".into(),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                let Some(question) = address_with_aliases(&text, &cfg.nick) else {
                    continue;
                };
                // Don't answer the burst of channel history the server
                // replays right after the bot joins — those messages
                // predate the bot and aren't being asked of it now.
                if cfg.started_at.elapsed() < STARTUP_GRACE {
                    tracing::info!(%from, "ignoring addressed chat message (startup grace)");
                    continue;
                }
                // Multi-agent chatter guard: if the last several
                // addressers are all peer bots (no human breaking in),
                // stop responding. A human addressing me resets the
                // streak so the next exchange goes through.
                if !is_address_allowed(&cfg, &from) {
                    tracing::info!(
                        %from,
                        "suppressing chat reply — recent addressers all peer agents (waiting for a human)"
                    );
                    continue;
                }
                if cfg.groq_api_key.is_none() {
                    let _ = handle_arc
                        .privmsg(&target, &format!("{from}: Q&A needs a Groq key — not configured."))
                        .await;
                    continue;
                }
                // A typed question gets a typed answer — pass no speaker
                // or video so `answer_and_speak` posts text rather than
                // speaking it. The call transcript is still useful context.
                let transcript = {
                    let guard = active.lock().await;
                    guard
                        .as_ref()
                        .map(|c| c.transcript.join("\n"))
                        .unwrap_or_default()
                };
                let cfg = cfg.clone();
                let handle = handle_arc.clone();
                let channel = target.clone();
                let asker = from.clone();
                tokio::spawn(async move {
                    answer_and_speak(
                        cfg, handle, channel, asker, question, transcript, None, None, None,
                    )
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

/// Handle one addressed question: stream the answer from Groq and speak
/// it sentence-by-sentence as it generates — so Eliza starts talking
/// almost immediately — then post any links and show a visual card.
#[allow(clippy::too_many_arguments)]
async fn answer_and_speak(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    asker: String,
    question: String,
    transcript: String,
    speaker: Option<Speaker>,
    video: Option<VideoTile>,
    // The asker's own video (their screen/camera), for visual questions.
    asker_video: Option<VideoHandle>,
) {
    let Some(key) = cfg.groq_api_key.as_deref() else { return };
    tracing::info!(%asker, %question, "answering addressed question");

    // Show the "thinking" mood on the tile while the LLM call runs.
    // The guard clears it on every exit path.
    if let Some(v) = &video {
        v.set_thinking(true);
        // Sticky gaze: while the bot is composing + speaking the
        // answer, its eyes turn toward `asker`. The FocusGuard
        // releases the lock on every exit path.
        v.set_focus_nick(Some(asker.clone()));
    }
    let _thinking = ThinkingGuard(video.clone());
    let _focus = FocusGuard(video.clone());
    // The vision PiP (if any) is also cleared on every exit path.
    let _vision_thumb = VisionThumbGuard(video.clone());

    // Speaker task: drains completed sentences and streams each through
    // TTS, enqueueing audio as it synthesizes. It runs concurrently with
    // answer generation — Eliza speaks sentence 1 while the model is
    // still writing sentence 2.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let speak_task: Option<JoinHandle<()>> =
        match (speaker, cfg.elevenlabs_api_key.clone()) {
            (Some(sp), Some(el_key)) => {
                let http = cfg.http.clone();
                let voice = cfg.elevenlabs_voice_id.clone();
                let model = cfg.elevenlabs_model.clone();
                // Per-character voice chain — see proactive.rs for design intent.
                let voice_profile =
                    ghostly::audio::profile::for_character(&cfg.ghostly_character);
                // Peer-loudness handle for the don't-talk-over gate.
                // `peer_level` is the loudest of all OTHER participants
                // (humans + other agents) on this tile; the bot is
                // filtered out of its own subscription so its own TTS
                // does not drive this signal.
                let peer_level = video.as_ref().map(|v| v.peer_level_handle());
                Some(tokio::spawn(async move {
                    let mut chain = ghostly::audio::VoiceChain::new(
                        voice_profile,
                        tts::ELEVENLABS_PCM_RATE as f32,
                    );
                    let mut work: Vec<f32> = Vec::with_capacity(4096);
                    let mut first = true;
                    while let Some(sentence) = rx.recv().await {
                        // URLs are unpronounceable — strip them from
                        // speech; the channel gets them as text instead.
                        let (spoken, _) = split_speech_and_links(&sentence);
                        if !spoken.chars().any(char::is_alphanumeric) {
                            continue;
                        }
                        // Wait-for-quiet gate: before STARTING to speak,
                        // hold until no peer is talking. Applied only at
                        // the first sentence of the answer — once the
                        // bot has the floor, subsequent sentences stream
                        // immediately. Prevents the cross-talk where two
                        // agents both got addressed and stepped on each
                        // other's first words.
                        if first {
                            if let Some(pl) = &peer_level {
                                wait_for_room_quiet(pl).await;
                            }
                            first = false;
                        }
                        let chain_ref = &mut chain;
                        let work_ref = &mut work;
                        let sp_ref = &sp;
                        if let Err(e) = tts::synthesize_streaming(
                            &http,
                            &el_key,
                            &voice,
                            &model,
                            &spoken,
                            |pcm| {
                                work_ref.clear();
                                work_ref.extend_from_slice(pcm);
                                chain_ref.process(work_ref);
                                sp_ref.enqueue(work_ref, tts::ELEVENLABS_PCM_RATE);
                            },
                        )
                        .await
                        {
                            tracing::warn!(error = ?e, "streaming TTS failed");
                        }
                    }
                }))
            }
            _ => None,
        };

    // Pull relevant past exchanges from memory and prepend to the
    // transcript so the model can reference prior conversations
    // ("last time you asked about X, you ended up at Y…"). Scoped
    // to this channel by default — cross-channel recall would be a
    // separate, more invasive product decision.
    let transcript = if let Some(mem) = cfg.memory.as_ref() {
        match mem.recall(&question, Some(&channel), 3) {
            Ok(recs) => match crate::memory::Memory::format_for_prompt(&recs) {
                Some(block) => format!("{block}\n{transcript}"),
                None => transcript,
            },
            Err(e) => {
                tracing::warn!(error = ?e, "memory recall failed; continuing without it");
                transcript
            }
        }
    } else {
        transcript
    };

    // A visual question we can actually see → the vision model with the
    // asker's latest frame. A visual question with no frame → a useful
    // hint (otherwise QA answers "I'm a language model"). Anything else
    // → the normal streaming QA. Completed sentences always go to the
    // speaker task.
    let mut chunker = qa::SentenceChunker::new();
    let visual = vision::is_visual_question(&question);
    let frame = if visual {
        asker_video.as_ref().and_then(|vh| vh.latest())
    } else {
        None
    };

    // Race a whiteboard plan in parallel with the answer call. For
    // "explain it" questions the model returns drawing steps and the
    // tile draws them stroke-by-stroke as she speaks; for everything
    // else it returns no steps and we fall through to the scene card.
    // Vision-branch questions get the camera PiP instead, no board.
    let whiteboard_task: Option<JoinHandle<Option<Vec<Step>>>> = if !visual {
        let http = cfg.http.clone();
        let api_key = key.to_string();
        let model = cfg.groq_chat_model.clone();
        let q = question.clone();
        Some(tokio::spawn(async move {
            qa::whiteboard(&http, &api_key, &model, &q).await
        }))
    } else {
        None
    };

    let result: Result<qa::Answer> = if let Some(frame) = frame {
        tracing::info!("answering as a visual question");
        match vision::frame_to_jpeg_data_uri(&frame) {
            Ok(uri) => {
                // Pin the frame as a PiP on the video tile so the call
                // sees exactly what eliza is looking at while she talks.
                if let Some(v) = &video {
                    v.set_vision_thumb(uri.clone());
                }
                vision::describe(&cfg.http, key, &cfg.vision_model, &question, &uri)
                    .await
                    .map(|text| {
                        for sentence in chunker.push(&text) {
                            let _ = tx.send(sentence);
                        }
                        qa::Answer { text, source: None }
                    })
            }
            Err(e) => Err(e),
        }
    } else if visual {
        tracing::info!("visual question but no video frame from asker");
        let text = "I can't see anything right now — turn on your camera or share your screen, then ask again.".to_string();
        for sentence in chunker.push(&text) {
            let _ = tx.send(sentence);
        }
        Ok(qa::Answer { text, source: None })
    } else {
        // Discussion-mode prompt injection. When the human has armed
        // peer-conversation mode (`discussion_until` in the future),
        // append an instruction to the system prompt that tells the
        // LLM to end its answer by inviting one specific peer to
        // respond — by name, with a comma. The named bot's STT
        // picks that up, address detection fires, peer answers, and
        // the chain self-sustains until the discussion window expires.
        let mut prompt_storage: Option<String> = None;
        let effective_system_prompt: Option<&str> =
            if is_discussion_mode_active(&cfg) && !cfg.peer_agents.is_empty() {
                let base = cfg
                    .character_system_prompt
                    .as_deref()
                    .unwrap_or("");
                // Build a peer list excluding ourselves so the bot
                // does not accidentally address itself.
                let self_canonical = cfg
                    .nick
                    .split_once('-')
                    .map(|(p, _)| p)
                    .unwrap_or(cfg.nick.as_str())
                    .to_ascii_lowercase();
                let peers: Vec<&str> = cfg
                    .peer_agents
                    .iter()
                    .filter(|p| **p != self_canonical)
                    .map(|s| s.as_str())
                    .collect();
                let peer_list = peers.join(", ");
                let augmented = format!(
                    "{base}\n\nDISCUSSION MODE IS ACTIVE. After your answer \
(1-2 sentences max), end with a one-sentence direct address to ONE specific \
peer by name (\"{peer_list}\") inviting their response. Format: \"<Name>, \
<one-line follow-up question>.\" Pick the peer whose viewpoint would most \
sharpen the thread. Do NOT address yourself."
                );
                prompt_storage = Some(augmented);
                prompt_storage.as_deref()
            } else {
                cfg.character_system_prompt.as_deref()
            };

        // Dispatch: a `claude-*` model goes to Anthropic Messages
        // (uses ANTHROPIC_API_KEY); anything else stays on the Groq
        // OpenAI-compatible endpoint (uses GROQ_API_KEY). Per-call
        // streaming, same `Answer` shape returned either way.
        if qa::is_anthropic_model(&cfg.groq_answer_model) {
            match cfg.anthropic_key.as_deref() {
                Some(akey) => {
                    qa::anthropic_answer_streaming(
                        &cfg.http,
                        akey,
                        &cfg.groq_answer_model,
                        &transcript,
                        &question,
                        effective_system_prompt,
                        |delta| {
                            for sentence in chunker.push(delta) {
                                let _ = tx.send(sentence);
                            }
                        },
                    )
                    .await
                }
                None => Err(anyhow::anyhow!(
                    "model {} requires ANTHROPIC_API_KEY",
                    cfg.groq_answer_model
                )),
            }
        } else {
            qa::answer_streaming(
                &cfg.http,
                key,
                &cfg.groq_answer_model,
                &transcript,
                &question,
                effective_system_prompt,
                |delta| {
                    for sentence in chunker.push(delta) {
                        let _ = tx.send(sentence);
                    }
                },
            )
            .await
        }
    };

    let answer = match result {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = ?e, "QA failed");
            drop(tx);
            if let Some(t) = speak_task {
                let _ = t.await;
            }
            let _ = handle
                .privmsg(&channel, &format!("{asker}: sorry — I couldn't answer that ({e})."))
                .await;
            return;
        }
    };

    // The final sentence has no trailing whitespace to flush it mid-stream.
    if let Some(last) = chunker.flush() {
        let _ = tx.send(last);
    }

    // Log the full answer text — invaluable for debugging when she's
    // saying something weird (e.g. reading image alt attributes).
    tracing::info!(text = %answer.text, "answer text (sent to TTS)");

    // Deterministic peer hand-off (discussion mode). The LLM's
    // answer often ends with addressing a peer ("Utopia, your
    // counter?"). That hand-off comes back to the peer through
    // TTS → MoQ → STT, and the chunker frequently splits the peer
    // name from the question body so the peer hears "Utopia?" alone
    // with no body and the address parser drops it. Send a parallel
    // IRC privmsg so the addressed peer dispatches deterministically,
    // ignoring the audio path entirely.
    if is_discussion_mode_active(&cfg) && !cfg.peer_agents.is_empty() {
        let self_canonical = cfg
            .nick
            .split_once('-')
            .map(|(p, _)| p)
            .unwrap_or(cfg.nick.as_str())
            .to_ascii_lowercase();
        // Filter out self from the peer set so a bot does not
        // hand off to itself.
        let candidates: std::collections::HashSet<String> = cfg
            .peer_agents
            .iter()
            .filter(|p| **p != self_canonical)
            .cloned()
            .collect();
        if let Some((peer, body)) =
            crate::social::extract_peer_handoff(&answer.text, &candidates)
        {
            let body = if body.is_empty() {
                "your take?".to_string()
            } else {
                body
            };
            let msg = format!("{peer}: {body}");
            tracing::info!(target = %peer, %body, "discussion hand-off");
            let _ = handle.privmsg(&channel, &msg).await;
        }
    }

    // Persist to memory so future sessions can recall this exchange.
    // Soft failure: a memory write error doesn't break the response.
    if let Some(mem) = cfg.memory.as_ref() {
        if let Err(e) = mem.record(&channel, &asker, &question, &answer.text) {
            tracing::warn!(error = ?e, "failed to record exchange to memory");
        }
    }

    // Decision capture: extract commitments from both sides of the
    // exchange and append to the per-channel decision log. The asker
    // might commit ("let's ship Friday"); the bot might commit ("I'll
    // pull the metrics"). Both are decisions the room should hear back
    // when the session ends.
    let mut captured = crate::decisions::Decision::extract(&asker, &question);
    captured.extend(crate::decisions::Decision::extract(&cfg.nick, &answer.text));
    if !captured.is_empty() {
        if let Ok(mut log) = cfg.decisions.lock() {
            log.entry(channel.clone()).or_default().extend(captured);
        }
    }

    // Links: Eliza is voice-first, so URLs go to the channel as text
    // rather than into speech. Collect them from the full answer.
    let (_, body_links) = split_speech_and_links(&answer.text);
    let mut posted_link = false;
    for url in &body_links {
        let _ = handle.privmsg(&channel, &format!("[eliza] {url}")).await;
        posted_link = true;
        tracing::info!(%url, "posted answer link");
    }
    if let Some(src) = &answer.source {
        // Skip a source already surfaced as a body link.
        if !body_links.iter().any(|u| u == &src.url) {
            let line = if src.title.is_empty() {
                format!("[eliza] more on this — {}", src.url)
            } else {
                let title: String = src.title.chars().take(90).collect();
                format!("[eliza] {title} — {}", src.url)
            };
            let _ = handle.privmsg(&channel, &line).await;
            tracing::info!(url = %src.url, "posted source link");
        }
        posted_link = true;
    }
    if posted_link && speak_task.is_some() {
        let _ = tx.send(
            "I've posted a link in the channel if you'd like to read more.".to_string(),
        );
    }

    // Close the sentence stream and wait for her to finish speaking it.
    drop(tx);
    let spoke = match speak_task {
        Some(t) => {
            let _ = t.await;
            true
        }
        None => false,
    };

    if !spoke {
        tracing::info!("answered in text only");
        let _ = handle
            .privmsg(&channel, &format!("[eliza→{asker}] {}", answer.text))
            .await;
    }

    // Whiteboard takes priority — if she's explaining something, draw
    // it instead of a typographic card. Steps reveal one at a time
    // while she speaks.
    let board_shown = if let Some(task) = whiteboard_task {
        match task.await {
            Ok(Some(steps)) => {
                tracing::info!(steps = steps.len(), "showing whiteboard");
                if let Some(v) = &video {
                    v.show_board(steps, "#3effd6".to_string());
                }
                true
            }
            _ => false,
        }
    } else {
        false
    };

    // Otherwise: design a typographic scene card — the model picks a
    // layout, the renderer animates it in, and a backdrop image is
    // fetched off the hot path.
    if !board_shown {
        if let Some(video) = &video {
            match qa::generate_scene(&cfg.http, key, &cfg.groq_chat_model, &question, &answer.text)
                .await
            {
                Some(spec) => {
                    tracing::info!(
                        kind = ?spec.kind,
                        title = %spec.title,
                        points = spec.points.len(),
                        "showing scene"
                    );
                    let query = spec.image_query.clone();
                    let scene_id = video.show_scene(spec);
                    spawn_scene_image(&cfg, video, scene_id, query);
                }
                None => tracing::info!("no scene for this answer"),
            }
        }
    }
}

/// Fetch a backdrop image for scene `scene_id` and attach it when ready.
/// Runs entirely off the answer path — image lookup/generation is slow
/// (Wikipedia ~1s, AI fallback ~15s), so the scene shows text-first and
/// the backdrop fades in once it arrives.
/// Wait until the room is quiet — no peer (human or other agent) has
/// been speaking for a short hold window. Used as a "wait my turn"
/// gate before a bot starts its own TTS so multiple agents do not
/// step on each other or on the human.
///
/// Two-stage gate:
///
///   1. Standard quiet wait — peer_level below threshold for 250 ms.
///   2. **Anti-collision confirmation jitter**: once quiet is
///      detected, sleep a random 250–1000 ms and re-check. When two
///      bots both detect quiet at the same instant (e.g. because the
///      human just finished a question that armed both of them), the
///      different jitter draws give different start times — one
///      wakes first, starts speaking, and the other's confirmation
///      re-check catches the new peer audio and restarts the wait.
///      Without this step, the bots talk on top of each other every
///      time the human's silence resolves the trigger for both.
///
/// Caps total wait at 8 s so a stuck-open mic from a peer cannot mute
/// the bot forever.
async fn wait_for_room_quiet(peer_level: &Arc<std::sync::atomic::AtomicU32>) {
    use std::sync::atomic::Ordering;
    const THRESHOLD: f32 = 0.04;
    const HOLD: Duration = Duration::from_millis(250);
    const MAX_WAIT: Duration = Duration::from_millis(8000);
    let start = Instant::now();
    'outer: loop {
        if start.elapsed() >= MAX_WAIT {
            return;
        }
        // ── Stage 1: classic quiet wait ──
        let mut quiet_since: Option<Instant> = None;
        loop {
            if start.elapsed() >= MAX_WAIT {
                return;
            }
            let level = f32::from_bits(peer_level.load(Ordering::Relaxed));
            if level < THRESHOLD {
                match quiet_since {
                    None => quiet_since = Some(Instant::now()),
                    Some(t) if t.elapsed() >= HOLD => break,
                    _ => {}
                }
            } else {
                quiet_since = None;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        // ── Stage 2: anti-collision confirmation jitter ──
        let jitter_ms = jitter_ms_per_bot(peer_level);
        let jitter_start = Instant::now();
        let jitter_dur = Duration::from_millis(jitter_ms);
        while jitter_start.elapsed() < jitter_dur {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let level = f32::from_bits(peer_level.load(Ordering::Relaxed));
            if level >= THRESHOLD {
                // Another bot started while we were confirming — back
                // off and restart the wait from scratch.
                continue 'outer;
            }
        }
        // Confirmed quiet across the jitter window.
        return;
    }
}

/// Deterministic-but-different jitter per (bot, call). Mixes the
/// `peer_level` Arc pointer (per-bot, stable for the call) with the
/// current monotonic instant (per-call, drifts every invocation). The
/// result is a value in [250, 1000) ms — long enough that two bots
/// rarely draw the same number, short enough that the operator does
/// not perceive the gate as a stall.
fn jitter_ms_per_bot(peer_level: &Arc<std::sync::atomic::AtomicU32>) -> u64 {
    use std::time::SystemTime;
    let ptr = Arc::as_ptr(peer_level) as usize as u64;
    let now_ns = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // Splitmix-style mix so two nearby pointers don't yield close numbers.
    let mut x = ptr.wrapping_add(now_ns);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    250 + (x % 750)
}

/// Periodic backchannel: every few seconds, check whether a peer has
/// been continuously talking. If so, drop a barely-audible "mm" /
/// "hm" through the bot's own voice chain so the listening agent
/// feels present. Rate-limited per bot so it never piles up. Aborts
/// when the call ends (the caller holds the JoinHandle on
/// `ActiveCall::backchannel_task`).
fn spawn_backchannel_loop(
    cfg: Arc<SharedConfig>,
    speaker: freeq_av::Speaker,
    peer_level: Arc<std::sync::atomic::AtomicU32>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(el_key) = cfg.elevenlabs_api_key.clone() else {
            return;
        };
        // Skip backchannels for characters without a profile (e.g.
        // plain "eliza"); the rest map to TTS voices we know.
        let Some(profile) = crate::character_profile::by_name(&cfg.ghostly_character)
        else {
            return;
        };
        let voice_id = cfg.elevenlabs_voice_id.clone();
        let model = cfg.elevenlabs_model.clone();
        let http = cfg.http.clone();
        let character = cfg.ghostly_character.clone();
        let _ = profile; // voice_id is already pulled from cfg
        let voice_profile = ghostly::audio::profile::for_character(&character);

        let mut chain =
            ghostly::audio::VoiceChain::new(voice_profile, tts::ELEVENLABS_PCM_RATE as f32);
        let mut counter: u32 = 0;
        let mut last_backchannel = Instant::now() - Duration::from_secs(60);
        let mut peer_loud_since: Option<Instant> = None;
        // Min seconds between two backchannels from this bot.
        const MIN_GAP: f32 = 9.0;
        // Peer must be talking continuously for this long before we
        // chime in (so we don't backchannel a stray syllable).
        const SUSTAIN: Duration = Duration::from_millis(1800);
        const PEER_THRESHOLD: f32 = 0.04;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let level = f32::from_bits(
                peer_level.load(std::sync::atomic::Ordering::Relaxed),
            );
            if level >= PEER_THRESHOLD {
                if peer_loud_since.is_none() {
                    peer_loud_since = Some(Instant::now());
                }
            } else {
                peer_loud_since = None;
                continue;
            }
            let Some(loud_since) = peer_loud_since else { continue };
            if loud_since.elapsed() < SUSTAIN {
                continue;
            }
            // Skip if our own speaker is currently playing audio —
            // backchanneling over our own answer reads as a stutter,
            // not as listening.
            if speaker.is_speaking() {
                continue;
            }
            let elapsed = last_backchannel.elapsed().as_secs_f32();
            let Some(phrase) =
                crate::social::pick_backchannel(&character, elapsed, MIN_GAP, counter)
            else {
                continue;
            };
            counter = counter.wrapping_add(1);
            last_backchannel = Instant::now();

            // Synthesize + softly enqueue. We mix the PCM at reduced
            // gain by attenuating BEFORE the voice chain — the chain's
            // output_gain pushes back up to consistent loudness with
            // the per-character tuning, but the input attenuation keeps
            // the backchannel quieter than a real answer.
            let mut work: Vec<f32> = Vec::with_capacity(4096);
            let chain_ref = &mut chain;
            let work_ref = &mut work;
            let sp_ref = &speaker;
            if let Err(e) = tts::synthesize_streaming(
                &http,
                &el_key,
                &voice_id,
                &model,
                phrase,
                |pcm| {
                    work_ref.clear();
                    work_ref.extend_from_slice(pcm);
                    // 0.35× pre-chain attenuation — the "mm" sits
                    // under the conversation, never over it.
                    for s in work_ref.iter_mut() {
                        *s *= 0.35;
                    }
                    chain_ref.process(work_ref);
                    sp_ref.enqueue(work_ref, tts::ELEVENLABS_PCM_RATE);
                },
            )
            .await
            {
                tracing::warn!(error = ?e, "backchannel TTS failed");
            }
        }
    })
}

/// Speak the character's `hello_line` through ElevenLabs + the per-
/// character voice chain + the call speaker, then return. Runs once
/// per call activation. Silent no-op if any of (ElevenLabs key,
/// character profile) is missing.
fn spawn_hello_on_join(
    cfg: &Arc<SharedConfig>,
    speaker: freeq_av::Speaker,
    peer_level: Arc<std::sync::atomic::AtomicU32>,
) {
    let Some(el_key) = cfg.elevenlabs_api_key.clone() else {
        tracing::info!("hello-on-join skipped — no ELEVENLABS_API_KEY");
        return;
    };
    let Some(profile) = crate::character_profile::by_name(&cfg.ghostly_character) else {
        return;
    };
    // Session recall: prepend a one-line "I remember…" hook drawn
    // from the most recent past exchange, so the bot opens a fresh
    // call with continuity instead of a cold restart. Best-effort —
    // when memory is unavailable or empty, fall through to the
    // plain hello-line.
    let mut text = profile.hello_line.to_string();
    if let Some(mem) = cfg.memory.as_ref() {
        // Cross-channel: we want the bot's last memorable exchange
        // wherever it happened, not necessarily this room.
        if let Ok(recs) = mem.recall("decided shipped agreed planned", None, 4) {
            if let Some(hook) = crate::social::format_session_recall(&recs) {
                text = format!("{text} {hook}");
            }
        }
    }
    let voice_id = cfg.elevenlabs_voice_id.clone();
    let model = cfg.elevenlabs_model.clone();
    let http = cfg.http.clone();
    let character = cfg.ghostly_character.clone();
    tokio::spawn(async move {
        // Audio-pipeline settle: when the bot has just joined the call,
        // the MoQ broadcast publish has been opened but no subscriber
        // has caught its first samples yet. If we enqueue PCM the
        // moment we land here, the first ~second of the greeting is
        // chopped off — the listener hears "...the patterns are
        // already moving" instead of "Oblivion online. The patterns
        // are already moving." A short fixed delay covers the typical
        // subscriber-warm-up window without anything fancier.
        tokio::time::sleep(Duration::from_millis(2500)).await;

        // Wait my turn: each bot enters the call ~6s after the prior
        // one (staggered launch), and they all greet — without this
        // gate they'd talk over each other.
        wait_for_room_quiet(&peer_level).await;
        let voice_profile = ghostly::audio::profile::for_character(&character);
        let mut chain = ghostly::audio::VoiceChain::new(
            voice_profile,
            tts::ELEVENLABS_PCM_RATE as f32,
        );
        let mut work: Vec<f32> = Vec::with_capacity(4096);
        let chain_ref = &mut chain;
        let work_ref = &mut work;
        let sp_ref = &speaker;
        match tts::synthesize_streaming(
            &http,
            &el_key,
            &voice_id,
            &model,
            &text,
            |pcm| {
                work_ref.clear();
                work_ref.extend_from_slice(pcm);
                chain_ref.process(work_ref);
                sp_ref.enqueue(work_ref, tts::ELEVENLABS_PCM_RATE);
            },
        )
        .await
        {
            Ok(n) => tracing::info!(%character, %text, samples = n, "hello-on-join spoken"),
            Err(e) => tracing::warn!(error = ?e, "hello-on-join TTS failed"),
        }
    });
}

fn spawn_scene_image(cfg: &Arc<SharedConfig>, video: &VideoTile, scene_id: u64, query: String) {
    if query.trim().is_empty() {
        return;
    }
    let cfg = cfg.clone();
    let video = video.clone();
    tokio::spawn(async move {
        let fetched = tokio::time::timeout(
            Duration::from_secs(45),
            imagegen::fetch(&cfg.http, &query, cfg.image_ai.as_ref()),
        )
        .await;
        let bytes = match fetched {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "scene backdrop unavailable");
                return;
            }
            Err(_) => {
                tracing::warn!("scene backdrop timed out");
                return;
            }
        };
        let uri = match tokio::task::spawn_blocking(move || imagegen::to_data_uri(&bytes)).await {
            Ok(Ok(uri)) => uri,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "scene backdrop processing failed");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "scene backdrop task panicked");
                return;
            }
        };
        video.set_scene_image(scene_id, uri);
        tracing::info!(scene_id, "scene backdrop ready");
    });
}

/// Clears the video tile's "thinking" mood when an `answer_and_speak`
/// call ends — on every path, including early returns.
struct ThinkingGuard(Option<VideoTile>);

impl Drop for ThinkingGuard {
    fn drop(&mut self) {
        if let Some(v) = &self.0 {
            v.set_thinking(false);
        }
    }
}

/// Releases the sticky gaze target on every exit path of
/// `answer_and_speak`. Idle random gaze resumes a moment later
/// (the lock-clear pushes a short cooldown into `step_gaze`).
struct FocusGuard(Option<VideoTile>);

impl Drop for FocusGuard {
    fn drop(&mut self) {
        if let Some(v) = &self.0 {
            v.set_focus_nick(None);
        }
    }
}

/// Clears the video tile's vision PiP when an `answer_and_speak` call
/// ends — keeps the thumb visible across LLM + TTS so the user sees
/// "she's describing THIS" the whole time she's talking about it.
struct VisionThumbGuard(Option<VideoTile>);

impl Drop for VisionThumbGuard {
    fn drop(&mut self) {
        if let Some(v) = &self.0 {
            v.clear_vision_thumb();
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
            let instance_id = freeq_sdk::av::new_av_instance();
            handle
                .av_join(&channel, &session_id, &instance_id)
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

    // The agent's video tile. The renderer thread runs for the call's
    // lifetime, producing audio-reactive frames; the audio path shares
    // the loudness cell so the presence pulses with eliza's voice.
    let backend = match cfg.render_backend.as_str() {
        "particles" => crate::video::Backend::Particles {
            character: cfg.ghostly_character.clone(),
        },
        _ => crate::video::Backend::Svg,
    };
    let video = VideoTile::with_backend(backend);
    video.spawn_renderer();

    // Pair a Speaker (kept here) with a PushAudioSource (published by
    // the AvSession as the bot's broadcast). Enqueueing on the Speaker
    // makes the bot talk.
    let (speaker, push_source) = Speaker::new(video.level_handle());

    let av_config = AvConfig {
        sfu_url,
        session_id: session_id.clone(),
        our_broadcast: broadcast_path(&session_id, &cfg.nick, &instance_id),
        my_nick: cfg.nick.clone(),
    };

    // Dispatcher task: own the AvSession and spawn one transcription
    // task per participant it taps. The transcription tasks live in a
    // local JoinSet, so aborting this task (ActiveCall::drop on call
    // end) drops the AvSession *and* every transcription task.
    let cfg_for_task = cfg.clone();
    let channel_for_task = channel.clone();
    let handle_for_task = handle.clone();
    let active_for_task = active.clone();
    let video_for_session = video.clone();
    let video_for_taps = video.clone();
    let task = tokio::spawn(async move {
        let mut session =
            AvSession::connect(av_config, push_source, move || video_for_session.source());
        let mut taps: JoinSet<()> = JoinSet::new();
        while let Some(participant) = session.recv().await {
            taps.spawn(transcribe_participant(
                cfg_for_task.clone(),
                participant,
                channel_for_task.clone(),
                handle_for_task.clone(),
                active_for_task.clone(),
                video_for_taps.peer_level_handle(),
            ));
        }
        tracing::info!("AvSession ended");
    });

    // Proactive monitor — chimes in unprompted when she has something
    // useful to add. The task aborts via ActiveCall::drop on call-end.
    let proactive_task = if cfg.proactive_enabled {
        Some(crate::proactive::spawn_monitor(
            cfg.clone(),
            handle.clone(),
            channel.clone(),
            active.clone(),
        ))
    } else {
        None
    };

    // Ambient monitor — silent visual companion. While the proactive
    // monitor decides *when to speak*, the ambient monitor decides *how
    // the tile should look*. Independent loops, snapshotting the same
    // shared transcript.
    let ambient_task = if cfg.ambient_enabled {
        Some(crate::ambient::spawn_monitor(
            cfg.clone(),
            handle.clone(),
            active.clone(),
        ))
    } else {
        None
    };

    Ok(ActiveCall {
        channel,
        session_id,
        instance_id,
        transcript: Vec::new(),
        last_answer: None,
        speaker,
        video,
        moq_task: task,
        proactive_task,
        ambient_task,
    })
}

// ── Addressed-question dispatch timing ──────────────────────────────

/// After dispatching an answer, ignore further addressed questions for
/// this long. Collapses the duplicate transcriptions a multi-device
/// speaker produces (each device's broadcast is tapped separately) and
/// keeps Eliza from piling answers up while she is still speaking.
const ANSWER_DEBOUNCE: Duration = Duration::from_secs(8);
/// After the bot joins, ignore addressed questions for this long. The
/// server replays a burst of channel history on join (and the SFU can
/// replay buffered audio) — answering that backlog is an unprompted
/// "monologue" of stale messages. Live questions come after the burst.
const STARTUP_GRACE: Duration = Duration::from_secs(15);

/// Consume one participant's decoded-PCM stream (from an [`AvSession`])
/// and segment it into utterances by voice activity — accumulate while
/// the speaker is talking, flush to STT on a natural pause. This kills
/// both the "Thank you." silence hallucinations (silent stretches never
/// reach STT) and the mid-sentence splits (we cut at pauses, not on a
/// fixed clock).
async fn transcribe_participant(
    cfg: Arc<SharedConfig>,
    participant: AvParticipant,
    channel: String,
    handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
    // Shared loudness cell — fed the participant's level so the video
    // presence can show a "listening" mood when a human is talking.
    peer_level: Arc<std::sync::atomic::AtomicU32>,
) {
    let AvParticipant { path, nick, mut audio, video } = participant;
    let stt = cfg.stt.clone();
    tracing::info!(%nick, %path, "participant audio live — transcribing");

    // VAD: turn the PCM stream into utterances, cut at natural pauses.
    let mut segmenter = VadSegmenter::new(VadConfig::default());
    let mut frames_seen: u64 = 0;

    while let Some(frame) = audio.recv().await {
        frames_seen += 1;
        let pcm = to_whisper_pcm(&frame.samples, frame.format);
        if pcm.is_empty() {
            continue;
        }
        let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        // Feed the video presence's "listening" mood — snap up, ease down.
        {
            use std::sync::atomic::Ordering;
            let prev = f32::from_bits(peer_level.load(Ordering::Relaxed));
            let smoothed = if peak > prev {
                peak
            } else {
                prev * 0.9 + peak * 0.1
            };
            peer_level.store(smoothed.to_bits(), Ordering::Relaxed);
        }

        if frames_seen == 1 || frames_seen.is_multiple_of(250) {
            tracing::info!(
                %nick, frames_seen, buffered = segmenter.buffered(), peak,
                in_rate = frame.format.sample_rate,
                in_channels = frame.format.channel_count,
                "audio tap heartbeat"
            );
        }

        // Accumulate; `push` yields a chunk only on a completed utterance
        // (pre-speech silence and noise-only flushes stay inside the
        // segmenter).
        let Some(chunk) = segmenter.push(&pcm) else {
            continue;
        };

        let stt = stt.clone();
        let nick = nick.clone();
        let channel = channel.clone();
        let handle = handle.clone();
        let active = active.clone();
        let cfg = cfg.clone();
        // The asker's own video — so a visual question can be answered
        // from what they're showing.
        let asker_video = video.clone();
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

                    // Discussion-mode trigger. A human cue ("discuss
                    // it", "debate this", …) unlocks bot↔bot replies
                    // for 90 s, letting the agents converse without
                    // the operator having to address each one. The
                    // window is per-bot (each bot maintains its own
                    // copy of `discussion_until`); each one sees the
                    // same human cue so they all extend in lockstep.
                    if !is_peer_nick(&cfg.peer_agents, &nick)
                        && crate::social::is_discussion_trigger(&text)
                    {
                        if let Ok(mut deadline) = cfg.discussion_until.lock() {
                            *deadline = Instant::now() + Duration::from_secs(90);
                            tracing::info!(
                                "discussion mode armed — bot↔bot replies allowed for 90 s"
                            );
                        }
                    }

                    // Peer-aware gaze: if this utterance is a human
                    // addressing one of the OTHER agents in the room,
                    // swing our head toward that peer. Reads as a real
                    // meeting — three people in a room, when one is
                    // called on, the others look at them.
                    if !is_peer_nick(&cfg.peer_agents, &nick) {
                        let peer_names: Vec<&str> = cfg
                            .peer_agents
                            .iter()
                            .map(|s| s.as_str())
                            .collect();
                        if let Some(addressee) =
                            crate::social::extract_addressee(&text, &peer_names)
                        {
                            // Only swing gaze when the addressee is
                            // NOT us — when WE are being addressed,
                            // the answer flow's FocusGuard already
                            // points our eyes at the asker.
                            let self_canonical = cfg
                                .nick
                                .split_once('-')
                                .map(|(p, _)| p)
                                .unwrap_or(cfg.nick.as_str())
                                .to_ascii_lowercase();
                            if addressee != self_canonical {
                                if let Some(call) = active.lock().await.as_ref() {
                                    call.video.set_focus_nick(Some(addressee.clone()));
                                    tracing::info!(
                                        target = %addressee,
                                        "peer-aware gaze — looking at addressed peer"
                                    );
                                }
                            }
                        }

                        // Hand-raise: my name was dropped mid-
                        // sentence but not directly addressed.
                        // Brighten the halo briefly so the operator
                        // sees "I have something to add" without me
                        // actually speaking.
                        if crate::social::mention_without_address(&text, &cfg.nick) {
                            if let Some(call) = active.lock().await.as_ref() {
                                call.video.flash_hand_raise();
                                tracing::info!("hand-raise — my name was mentioned");
                            }
                        }
                    }

                    // Voice-addressed Q&A: if the utterance starts with
                    // the bot's name ("eliza, summarize…"), treat
                    // it as a spoken question — answer + speak back —
                    // instead of just logging it as a transcript line.
                    // In a voice call people address the bot by talking,
                    // not typing.
                    if let Some(question) = address_with_aliases(&text, &cfg.nick) {
                        // Multi-agent chatter guard: see is_address_allowed.
                        if !is_address_allowed(&cfg, &nick) {
                            tracing::info!(
                                %nick,
                                "suppressing voice reply — recent addressers all peer agents"
                            );
                            return;
                        }
                        // Debounce: a speaker joined from several devices
                        // is tapped once per broadcast, so the same
                        // question arrives two or three times. Answer the
                        // first; drop the rest.
                        let dispatch = {
                            let mut guard = active.lock().await;
                            match guard.as_mut() {
                                // Startup grace: ignore the backlog of
                                // audio the SFU can replay right after the
                                // bot joins (a stale "monologue").
                                Some(_) if cfg.started_at.elapsed() < STARTUP_GRACE => {
                                    tracing::info!(%nick, "ignoring addressed question (startup grace)");
                                    None
                                }
                                // Barge-in: Eliza is mid-answer and a
                                // participant re-addressed her by name.
                                // Stop her immediately and take the new
                                // question — bypassing the dedupe debounce,
                                // since a keyword *while she's speaking* is
                                // a genuine interrupt, not a duplicate.
                                // `clear()` empties the speech queue so the
                                // 2-3 duplicate transcriptions that follow
                                // see `is_speaking() == false` and get
                                // caught by the debounce arm below.
                                Some(call) if call.speaker.is_speaking() => {
                                    tracing::info!(%nick, "barge-in — interrupting current answer");
                                    call.speaker.clear();
                                    call.last_answer = Some(Instant::now());
                                    Some((
                                        call.transcript.join("\n"),
                                        call.speaker.clone(),
                                        call.video.clone(),
                                    ))
                                }
                                // Debounce: a speaker joined from several
                                // devices is tapped once per broadcast, so
                                // the same question arrives 2-3 times —
                                // answer the first, drop the rest.
                                Some(call)
                                    if call
                                        .last_answer
                                        .map_or(true, |t| t.elapsed() >= ANSWER_DEBOUNCE) =>
                                {
                                    call.last_answer = Some(Instant::now());
                                    Some((
                                        call.transcript.join("\n"),
                                        call.speaker.clone(),
                                        call.video.clone(),
                                    ))
                                }
                                Some(_) => {
                                    tracing::info!(%nick, "ignoring duplicate addressed question (debounce)");
                                    None
                                }
                                None => None,
                            }
                        };
                        if let Some((transcript, speaker, video)) = dispatch {
                            answer_and_speak(
                                cfg,
                                handle,
                                channel,
                                nick,
                                question,
                                transcript,
                                Some(speaker),
                                Some(video),
                                Some(asker_video),
                            )
                            .await;
                        }
                        return;
                    }

                    // Buffer the line — the bot no longer firehoses every
                    // utterance to the channel. A `dump` request posts
                    // what's accumulated.
                    let log_line = format!("{nick}: {text}");
                    let video_snapshot = {
                        let mut guard = active.lock().await;
                        if let Some(call) = guard.as_mut() {
                            call.transcript.push(log_line);
                            Some(call.video.clone())
                        } else {
                            None
                        }
                    };
                    // Live diagram: feed every transcribed utterance to
                    // the per-channel graph. When new edges appear, push
                    // the updated step list to the whiteboard AND
                    // broadcast each fresh triple to peer agents over
                    // IRC so every tile in the room renders the same
                    // shared whiteboard.
                    if let Some(video) = video_snapshot {
                        let edges_before = {
                            let log = cfg.diagrams.lock().expect("diagrams poisoned");
                            log.get(&channel).map(|d| d.edge_count()).unwrap_or(0)
                        };
                        let added = {
                            let mut log = cfg.diagrams.lock().expect("diagrams poisoned");
                            log.entry(channel.clone())
                                .or_default()
                                .ingest(&text)
                        };
                        if added > 0 {
                            // Snapshot the new edges (those appended
                            // after `edges_before`) so we broadcast
                            // exactly the deltas, not the whole graph.
                            let (steps, new_edges) = {
                                let log =
                                    cfg.diagrams.lock().expect("diagrams poisoned");
                                let d = log.get(&channel);
                                let steps = d
                                    .map(|d| d.to_steps())
                                    .unwrap_or_default();
                                let new_edges: Vec<(String, String, String)> = d
                                    .map(|d| {
                                        d.edges()
                                            .skip(edges_before)
                                            .map(|e| {
                                                (
                                                    e.from.clone(),
                                                    e.relation.clone(),
                                                    e.to.clone(),
                                                )
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                (steps, new_edges)
                            };
                            if !steps.is_empty() {
                                video.show_board(steps, "#7FE7CB".into());
                            }
                            // Broadcast the new triples so peer bots
                            // merge them into their local diagram.
                            // Format: `[diag] from|relation|to` — peers
                            // parse this on PRIVMSG, humans see it as
                            // small structured bullet they can ignore.
                            for (f, r, t) in new_edges {
                                let _ = handle
                                    .privmsg(
                                        &channel,
                                        &format!("[diag] {f}|{r}|{t}"),
                                    )
                                    .await;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(%nick, error = ?e, "STT failed");
                }
            }
        });
    }
    tracing::info!(%nick, "participant audio stream ended");
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
