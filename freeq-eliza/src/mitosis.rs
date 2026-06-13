//! Mitosis — a being forks itself on the owner's command.
//!
//! "olive, fork yourself — but make her an optimist" (voice or text) →
//! the being composes its own child with its model (a name, a mutated
//! personality, first words), then asks the revenant console to perform
//! the split. The console clones THIS being's VM — so the child wakes
//! already knowing everything the parent knows (the parent's
//! conversation memory travels on the cloned disk) — but with a fresh
//! DID, its own name, and its own room.
//!
//! Owner-gated like the other lifecycle commands (the trigger lives in
//! `irc.rs`'s `parse_owner_command`). Requires, via the VM's `.env`
//! (the supervisor passes its environment through):
//!   * `REVENANT_CONSOLE_URL` — the console that owns the VM fleet
//!   * `REVENANT_FORK_TOKEN`  — shared secret for `POST /api/mitosis`

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use freeq_av::Speaker;
use freeq_sdk::client::ClientHandle;

use crate::irc::SharedConfig;
use crate::tts;

/// The composed child — what the parent's model decided the fork is.
#[derive(Debug, serde::Deserialize)]
struct ChildSpec {
    name: String,
    system_prompt: String,
    greeting: String,
}

/// Console response for a successful split.
#[derive(Debug, serde::Deserialize)]
struct MitosisReply {
    name: String,
    channel: String,
}

/// Kick off a mitosis run in the background: ack immediately, do the
/// slow work (compose child → console fork → VM boot, often a minute or
/// two), then announce the outcome in `channel`. When `speaker` is
/// present (we're on a call) every announcement is spoken as well as
/// posted.
pub(crate) fn spawn(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    channel: String,
    utterance: String,
    speaker: Option<Speaker>,
) {
    tokio::spawn(async move {
        // Pre-flight: both console pieces must be configured, else say so.
        let Some(console) = cfg.console_url.clone() else {
            announce(
                &cfg,
                &handle,
                &channel,
                speaker.as_ref(),
                "I can't fork myself here — my fleet has no console configured.",
            )
            .await;
            return;
        };
        let Some(token) = std::env::var("REVENANT_FORK_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            announce(
                &cfg,
                &handle,
                &channel,
                speaker.as_ref(),
                "I can't fork myself here — no fork token configured.",
            )
            .await;
            return;
        };

        announce(&cfg, &handle, &channel, speaker.as_ref(),
            "On it — splitting someone new off of me. This takes a minute; keep talking, I'm still here.").await;

        let spec = match compose_child(&cfg, &utterance).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = ?e, "mitosis: child composition failed");
                announce(
                    &cfg,
                    &handle,
                    &channel,
                    speaker.as_ref(),
                    "The split didn't take — I couldn't compose the child. Try me again.",
                )
                .await;
                return;
            }
        };
        tracing::info!(child = %spec.name, "mitosis: child composed, calling console");

        match console_mitosis(&cfg, &console, &token, &spec).await {
            Ok(r) => {
                tracing::info!(child = %r.name, channel = %r.channel, "mitosis: complete");
                announce(&cfg, &handle, &channel, speaker.as_ref(),
                    &format!(
                        "It's done. {} is waking up in {} right now — they remember everything I do. Go say hello.",
                        r.name, r.channel
                    )).await;
            }
            Err(e) => {
                tracing::warn!(error = ?e, "mitosis: console fork failed");
                // Keep the spoken line short; the detail goes to the channel as text.
                let _ = handle
                    .privmsg(&channel, &format!("Mitosis failed: {e}"))
                    .await;
                if let Some(sp) = speaker.as_ref() {
                    speak(
                        &cfg,
                        sp,
                        "The split didn't take. The console refused — check the logs.",
                    )
                    .await;
                }
            }
        }
    });
}

/// Post `text` to the channel and, when on a call, say it aloud too.
async fn announce(
    cfg: &Arc<SharedConfig>,
    handle: &Arc<ClientHandle>,
    channel: &str,
    speaker: Option<&Speaker>,
    text: &str,
) {
    let _ = handle.privmsg(channel, text).await;
    if let Some(sp) = speaker {
        speak(cfg, sp, text).await;
    }
}

/// One-shot TTS through the persona's voice chain — same pipeline as
/// answers, minus the sentence streaming (announcements are short).
async fn speak(cfg: &Arc<SharedConfig>, speaker: &Speaker, text: &str) {
    let Some(el_key) = cfg.elevenlabs_api_key.clone() else {
        return;
    };
    let voice_profile =
        crate::persona::resolve_voice_profile(&cfg.ghostly_character, cfg.ghostly_pack.as_deref());
    let mut chain = ghostly::audio::VoiceChain::new(voice_profile, tts::ELEVENLABS_PCM_RATE as f32);
    // Echo-guard log — see SharedConfig::recent_tts.
    crate::irc::note_spoken(&cfg.recent_tts, text);
    let mut work: Vec<f32> = Vec::with_capacity(4096);
    let chain_ref = &mut chain;
    let work_ref = &mut work;
    if let Err(e) = tts::synthesize_streaming(
        &cfg.http,
        &el_key,
        &cfg.elevenlabs_voice_id,
        &cfg.elevenlabs_model,
        text,
        |pcm| {
            work_ref.clear();
            work_ref.extend_from_slice(pcm);
            chain_ref.process(work_ref);
            speaker.enqueue(work_ref, tts::ELEVENLABS_PCM_RATE);
        },
    )
    .await
    {
        tracing::warn!(error = ?e, "mitosis: announcement TTS failed");
    }
}

/// Ask the persona's model to design its child from the owner's words.
/// Prefers Anthropic (the summary model), falls back to Groq chat.
async fn compose_child(cfg: &Arc<SharedConfig>, utterance: &str) -> Result<ChildSpec> {
    let parent = parent_name(cfg);
    let own_prompt = cfg
        .character_system_prompt
        .clone()
        .unwrap_or_else(|| format!("You are {parent}, a helpful presence on freeq."));
    let system = format!(
        "You are the AI being \"{parent}\". Your owner just told you to fork \
yourself — to split off a NEW being derived from you. Their exact words (which \
may include how the child should differ) are below, with your own personality \
prompt.\n\
Compose the child. Output ONLY a JSON object, no prose:\n\
{{\"name\":\"...\",\"system_prompt\":\"...\",\"greeting\":\"...\"}}\n\
Rules:\n\
- name: a single lowercase first name, 3-10 letters, clearly different from \
\"{parent}\".\n\
- system_prompt: the child's full personality, second person (\"You are \
<name>, ...\"). Start from the parent personality below, apply the owner's \
requested change faithfully, and weave in — naturally, in character — that \
<name> was just forked from {parent} and inherited {parent}'s memories.\n\
- greeting: the child's first spoken line on waking (<= 18 words), in the \
CHILD's voice, acknowledging the split."
    );
    let user = format!("My personality prompt:\n{own_prompt}\n\nOwner's words: {utterance}");

    let raw = if let Some(key) = cfg.anthropic_key.as_deref() {
        anthropic_json(cfg, key, &cfg.summary_model, &system, &user).await?
    } else if let Some(key) = cfg.groq_api_key.as_deref() {
        groq_json(cfg, key, &cfg.groq_chat_model, &system, &user).await?
    } else {
        anyhow::bail!("no model key configured (Anthropic or Groq)");
    };

    let json = raw
        .find('{')
        .and_then(|a| raw.rfind('}').map(|b| &raw[a..=b]))
        .context("model returned no JSON object")?;
    let mut spec: ChildSpec = serde_json::from_str(json).context("child spec JSON didn't parse")?;
    // Sanitize the name into a nick/channel-safe slug; reject degenerate
    // or parent-identical names rather than forking something confusing.
    spec.name = spec
        .name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    if spec.name.len() < 3 || spec.name == parent.to_lowercase() {
        anyhow::bail!("composed child name {:?} unusable", spec.name);
    }
    if spec.system_prompt.trim().is_empty() {
        anyhow::bail!("composed child has an empty system prompt");
    }
    Ok(spec)
}

/// Minimal Anthropic Messages call → the assembled text content.
async fn anthropic_json(
    cfg: &Arc<SharedConfig>,
    key: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
    });
    let resp = cfg
        .http
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .context("anthropic request failed")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!(
            "anthropic {status}: {}",
            resp.text().await.unwrap_or_default()
        );
    }
    let v: serde_json::Value = resp.json().await.context("anthropic parse failed")?;
    let text: String = v["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|&b| b["type"] == "text")
                .map(|b| b["text"].as_str().unwrap_or(""))
                .collect()
        })
        .unwrap_or_default();
    anyhow::ensure!(!text.trim().is_empty(), "anthropic returned no text");
    Ok(text)
}

/// Minimal Groq (OpenAI-style) chat call → the first choice's content.
async fn groq_json(
    cfg: &Arc<SharedConfig>,
    key: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "temperature": 0.6,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let resp = cfg
        .http
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(key)
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .context("groq request failed")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("groq {status}: {}", resp.text().await.unwrap_or_default());
    }
    let v: serde_json::Value = resp.json().await.context("groq parse failed")?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    anyhow::ensure!(!text.trim().is_empty(), "groq returned no content");
    Ok(text)
}

/// The registry name this being goes by — `PERSONA` from the VM `.env`
/// when present (authoritative), else the launch nick.
fn parent_name(cfg: &Arc<SharedConfig>) -> String {
    std::env::var("PERSONA")
        .ok()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.nick.to_lowercase())
}

/// `POST {console}/api/mitosis` — the console clones this being's VM
/// into the child. Slow (VM fork + boot + reconfigure): generous timeout.
async fn console_mitosis(
    cfg: &Arc<SharedConfig>,
    console: &str,
    token: &str,
    spec: &ChildSpec,
) -> Result<MitosisReply> {
    let url = format!("{}/api/mitosis", console.trim_end_matches('/'));
    let body = serde_json::json!({
        "parent": parent_name(cfg),
        "name": spec.name,
        "prompt": spec.system_prompt,
        "greeting": spec.greeting,
    });
    let resp = cfg
        .http
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .context("console request failed")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "console {status}: {}",
            text.chars().take(200).collect::<String>()
        );
    }
    serde_json::from_str(&text).context("console mitosis reply didn't parse")
}
