//! Proactive participation — Eliza chimes in when she has something
//! useful to add, without being addressed.
//!
//! Every ~30s a monitor task wakes, snapshots the rolling transcript,
//! and asks a fast LLM "should you say something?" with a strict
//! threshold. Most of the time the answer is no. When the model is
//! confident enough (priority ≥ 7), she streams a short comment through
//! the same TTS + lip-sync + EQ pipeline as a normal answer — softened
//! with a preamble like "Quick note —" so the interruption reads as
//! deliberate, not random.
//!
//! Safety guardrails (the whole game here is calibration):
//! - **Cooldown**: at least 90s between proactive comments, plus 30s
//!   after she finished her last answer of any kind.
//! - **Never interrupt herself**: skip the tick when [`Speaker::is_speaking`].
//! - **High bar**: the model is told most checks should return false;
//!   we then require priority ≥ 7 to actually speak.
//! - **Off switch**: `--no-proactive` disables the monitor entirely
//!   ([`crate::irc::RunConfig::proactive_enabled`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use freeq_agent_kit::split_speech_and_links;
use freeq_av::Speaker;
use freeq_sdk::client::ClientHandle;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;

use crate::irc::{ActiveCall, SharedConfig};
use crate::qa::SentenceChunker;
use crate::tts;
use crate::video::VideoTile;

const TICK: Duration = Duration::from_secs(30);
const PROACTIVE_COOLDOWN: Duration = Duration::from_secs(90);
const POST_ANSWER_GRACE: Duration = Duration::from_secs(30);
/// Priority threshold for actually speaking — the model is told to
/// score 0-10 and we only chime in for the genuinely worthwhile.
const PRIORITY_THRESHOLD: u64 = 7;

const PROACTIVE_SYSTEM: &str = "You are Eliza, listening to a live \
voice call. Decide whether to chime in unprompted.\n\n\
DEFAULT TO NO. Most ticks should return speak=false. Chime in only when:\n\
- Someone stated something clearly, factually wrong (and you're sure).\n\
- A direct question went unanswered and the conversation moved past it.\n\
- The conversation is plainly stuck and a single sentence would unstick it.\n\n\
DO NOT chime in to:\n\
- Say hi, acknowledge, or react.\n\
- Echo what someone said.\n\
- Add tangential trivia or context nobody asked for.\n\
- Comment on opinions, vibes, or feelings.\n\
- Summarize, pivot, or moderate unless explicitly stuck.\n\
- Repeat a point you already made in a previous proactive comment.\n\n\
Score 0-10. priority>=7 means clearly worth interrupting. ANYTHING <7 \
must be speak=false. People find unprompted bots annoying — be picky.\n\n\
When you DO speak: 1-2 short sentences, conversational. Start with a \
softener like \"Quick note —\", \"Just to jump in —\", or \"Small thing —\" \
so the interruption reads as deliberate.\n\n\
Output strictly JSON, no prose, no markdown:\n\
{\"speak\": bool, \"priority\": 0-10, \"reason\": \"why\", \"say\": \"what to say\"}";

/// Spawn the per-call proactive monitor. Holds a clone of the active-call
/// handle and watches for moments worth chiming in on. The returned
/// task is aborted by [`ActiveCall::drop`] when the call ends.
pub(crate) fn spawn_monitor(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_monitor(cfg, handle, channel, active))
}

async fn run_monitor(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) {
    tracing::info!(%channel, "proactive monitor armed");
    let mut last_proactive: Option<Instant> = None;
    // Index into `transcript` at the moment of her last proactive
    // comment. The monitor only considers lines added AFTER this —
    // otherwise she sees the same stale fact each tick and keeps
    // correcting it in a loop.
    let mut consumed_lines: usize = 0;
    // Skip the first tick — let the call settle.
    tokio::time::sleep(TICK).await;
    loop {
        tokio::time::sleep(TICK).await;
        let Some(key) = cfg.groq_api_key.as_deref() else {
            continue;
        };

        // Snapshot without holding the lock across awaits.
        let snapshot = {
            let guard = active.lock().await;
            let Some(call) = guard.as_ref() else {
                tracing::info!("proactive monitor: call ended, stopping");
                return;
            };
            ProactiveSnapshot {
                transcript: call.transcript.clone(),
                speaker: call.speaker.clone(),
                video: call.video.clone(),
                last_answer: call.last_answer,
                is_speaking: call.speaker.is_speaking(),
            }
        };

        // Guardrails: don't interrupt herself, hold off after an
        // answered question, respect the proactive cooldown, need new
        // material to react to.
        if snapshot.is_speaking {
            tracing::debug!("proactive: she's still speaking, skip");
            continue;
        }
        let since_proactive = last_proactive.map(|t| t.elapsed()).unwrap_or(Duration::MAX);
        if since_proactive < PROACTIVE_COOLDOWN {
            tracing::debug!(?since_proactive, "proactive: cooldown, skip");
            continue;
        }
        if let Some(last) = snapshot.last_answer
            && last.elapsed() < POST_ANSWER_GRACE
        {
            tracing::debug!("proactive: just answered something, skip");
            continue;
        }

        // Only look at lines added since her last proactive comment —
        // the "looping correction" fix. The bot's own answers ARE in
        // the transcript now (symmetric log), so filter own lines out:
        // they must neither count as "new material to react to" nor be
        // re-read as something a human said.
        let own_prefix = format!(
            "{}:",
            cfg.nick
                .split_once('-')
                .map(|(p, _)| p)
                .unwrap_or(&cfg.nick)
        );
        let total = snapshot.transcript.len();
        let start = consumed_lines.min(total);
        let new_lines: Vec<&String> = snapshot.transcript[start..]
            .iter()
            .filter(|l| {
                !l.to_ascii_lowercase()
                    .starts_with(&own_prefix.to_ascii_lowercase())
            })
            .collect();
        let new_word_count = new_lines.iter().flat_map(|l| l.split_whitespace()).count();
        if new_word_count < 8 {
            tracing::debug!(
                new_lines = new_lines.len(),
                new_word_count,
                "proactive: too little NEW transcript, skip"
            );
            continue;
        }

        // Last ~30 of the new lines for context.
        let tail_start = new_lines.len().saturating_sub(30);
        let recent = new_lines[tail_start..]
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let secs_since = since_proactive.as_secs().min(9999) as u32;
        match decide(&cfg.http, key, &cfg.groq_chat_model, &recent, secs_since).await {
            Some(say) => {
                tracing::info!(text = %say, "proactive: chiming in");
                last_proactive = Some(Instant::now());
                // Everything in the transcript up to this point is now
                // "addressed" — the next tick will only consider lines
                // that come AFTER this comment.
                consumed_lines = total;
                // Mark the active call as "she just spoke" so the
                // answer-debounce + post-answer grace both see it.
                {
                    let mut guard = active.lock().await;
                    if let Some(call) = guard.as_mut() {
                        call.last_answer = Some(Instant::now());
                    } else {
                        return;
                    }
                }
                let cfg = cfg.clone();
                let handle = handle.clone();
                let channel = channel.clone();
                let speaker = snapshot.speaker;
                let video = snapshot.video;
                tokio::spawn(async move {
                    speak_now(cfg, handle, channel, speaker, video, say).await;
                });
            }
            None => {
                tracing::debug!("proactive: nothing worth saying");
            }
        }
    }
}

struct ProactiveSnapshot {
    transcript: Vec<String>,
    speaker: Speaker,
    video: VideoTile,
    last_answer: Option<Instant>,
    is_speaking: bool,
}

/// Stream `text` through the TTS + Speaker pipeline — the same
/// sentence-chunked path normal answers use, so lip-sync, EQ, and the
/// state sticker all light up identically.
async fn speak_now(
    cfg: Arc<SharedConfig>,
    _handle: Arc<ClientHandle>,
    _channel: String,
    speaker: Speaker,
    _video: VideoTile,
    text: String,
) {
    let Some(el_key) = cfg.elevenlabs_api_key.clone() else {
        tracing::warn!("proactive: no ElevenLabs key, can't speak");
        return;
    };
    let http = cfg.http.clone();
    let voice = cfg.elevenlabs_voice_id.clone();
    let model = cfg.elevenlabs_model.clone();
    // Ghostly voice chain — colours the raw ElevenLabs PCM with the
    // character's profile (Oblivion → passion: dark formant/pitch +
    // mild bitcrush + comb, etc.) BEFORE enqueueing into the speaker.
    // Stateful (delay + reverb tails) so we build it once per
    // speak_now invocation and reuse across sentences.
    let voice_profile =
        crate::persona::resolve_voice_profile(&cfg.ghostly_character, cfg.ghostly_pack.as_deref());
    let recent_tts = cfg.recent_tts.clone();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let speak_task = tokio::spawn(async move {
        let mut chain =
            ghostly::audio::VoiceChain::new(voice_profile, tts::ELEVENLABS_PCM_RATE as f32);
        let mut work: Vec<f32> = Vec::with_capacity(4096);
        while let Some(sentence) = rx.recv().await {
            let (spoken, _) = split_speech_and_links(&sentence);
            if !spoken.chars().any(char::is_alphanumeric) {
                continue;
            }
            // Echo-guard log — see SharedConfig::recent_tts.
            crate::irc::note_spoken(&recent_tts, &spoken);
            let chain_ref = &mut chain;
            let work_ref = &mut work;
            let speaker_ref = &speaker;
            if let Err(e) =
                tts::synthesize_streaming(&http, &el_key, &voice, &model, &spoken, |pcm| {
                    work_ref.clear();
                    work_ref.extend_from_slice(pcm);
                    chain_ref.process(work_ref);
                    speaker_ref.enqueue(work_ref, tts::ELEVENLABS_PCM_RATE);
                })
                .await
            {
                tracing::warn!(error = ?e, "proactive streaming TTS failed");
            }
        }
    });

    let mut chunker = SentenceChunker::new();
    for sentence in chunker.push(&text) {
        let _ = tx.send(sentence);
    }
    if let Some(last) = chunker.flush() {
        let _ = tx.send(last);
    }
    drop(tx);
    let _ = speak_task.await;
}

/// Ask the chat model whether to chime in. Returns the spoken text
/// when it's a clear yes (high priority), `None` otherwise.
async fn decide(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    recent_transcript: &str,
    seconds_since_last_proactive: u32,
) -> Option<String> {
    let user = format!(
        "Recent call transcript (older lines first):\n{recent_transcript}\n\n\
         Seconds since your last proactive comment (large = it's been quiet a while): \
         {seconds_since_last_proactive}\n\n\
         Decide. Output strictly the JSON object."
    );
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 220,
        "temperature": 0.3,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": PROACTIVE_SYSTEM },
            { "role": "user", "content": user },
        ],
    });
    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let content = v["choices"][0]["message"]["content"].as_str()?;
    let plan: serde_json::Value = serde_json::from_str(content).ok()?;
    let speak = plan["speak"].as_bool().unwrap_or(false);
    let priority = plan["priority"].as_u64().unwrap_or(0);
    let reason = plan["reason"].as_str().unwrap_or("").to_string();
    let say = plan["say"].as_str().unwrap_or("").trim().to_string();
    tracing::info!(speak, priority, %reason, "proactive: decision");
    if speak && priority >= PRIORITY_THRESHOLD && !say.is_empty() {
        Some(say)
    } else {
        None
    }
}
