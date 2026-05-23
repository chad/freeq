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
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.moq_task.abort();
        if let Some(t) = &self.proactive_task {
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
        groq_answer_model,
        vision_model,
        elevenlabs_api_key,
        elevenlabs_voice_id,
        elevenlabs_model,
        image_ai,
        proactive_enabled,
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
                let Some(question) = extract_addressed(&text, &cfg.nick) else {
                    continue;
                };
                // Don't answer the burst of channel history the server
                // replays right after the bot joins — those messages
                // predate the bot and aren't being asked of it now.
                if cfg.started_at.elapsed() < STARTUP_GRACE {
                    tracing::info!(%from, "ignoring addressed chat message (startup grace)");
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
    }
    let _thinking = ThinkingGuard(video.clone());
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
                Some(tokio::spawn(async move {
                    while let Some(sentence) = rx.recv().await {
                        // URLs are unpronounceable — strip them from
                        // speech; the channel gets them as text instead.
                        let (spoken, _) = split_speech_and_links(&sentence);
                        if !spoken.chars().any(char::is_alphanumeric) {
                            continue;
                        }
                        if let Err(e) = tts::synthesize_streaming(
                            &http,
                            &el_key,
                            &voice,
                            &model,
                            &spoken,
                            |pcm| sp.enqueue(pcm, tts::ELEVENLABS_PCM_RATE),
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
        qa::answer_streaming(
            &cfg.http,
            key,
            &cfg.groq_answer_model,
            &transcript,
            &question,
            |delta| {
                for sentence in chunker.push(delta) {
                    let _ = tx.send(sentence);
                }
            },
        )
        .await
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
    let video = VideoTile::new();
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

                    // Voice-addressed Q&A: if the utterance starts with
                    // the bot's name ("eliza, summarize…"), treat
                    // it as a spoken question — answer + speak back —
                    // instead of just logging it as a transcript line.
                    // In a voice call people address the bot by talking,
                    // not typing.
                    if let Some(question) = extract_addressed(&text, &cfg.nick) {
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
