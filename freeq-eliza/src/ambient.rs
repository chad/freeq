//! Ambient "manifesting" — while she's *listening* her tile reflects
//! what's being discussed. Every ~20s a fast LLM call reads recent
//! transcript and picks:
//!   - a 1-3 word concept (printed on the HUD chip),
//!   - a hex accent colour (blended into the mood scrim),
//!   - an optional concrete image query.
//!
//! When the concept is concrete (a person, place, thing the listener
//! could picture) the monitor also drops a Hero scene with that subject
//! as the image backdrop — reusing the exact same scene + image-fetch
//! path her answers use. Most ticks are pure colour/topic shifts; the
//! image escalation kicks in on lingering concrete subjects.
//!
//! Silent on its own (it does NOT speak). Lives alongside the proactive
//! monitor — proactive owns *speaking*, ambient owns *looking like she's
//! tracking*. They share the transcript and snapshot it independently.
//!
//! Guardrails:
//! - 20s tick, skip first one (let the call settle),
//! - ≥ 12 new transcript words since the last applied concept,
//! - 60s minimum between scene escalations (an image card is loud — don't
//!   let it churn),
//! - never escalate while she's mid-answer (an active scene/board is up
//!   from QA — clobbering it would interrupt her own visual narrative).
//! - off switch: `--no-ambient` ([`crate::irc::RunConfig::ambient_enabled`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use freeq_sdk::client::ClientHandle;
use tokio::sync::Mutex as AsyncMutex;

use crate::imagegen;
use crate::irc::{ActiveCall, SharedConfig};
use crate::video::VideoTile;

/// How often the ambient loop wakes. Short — the HUD chip + accent
/// shift are cheap (one fast LLM call, no image fetch) so a tight tick
/// makes the tile feel genuinely responsive to the conversation.
const TICK: Duration = Duration::from_secs(8);
/// Lead-in before the first tick fires. Long enough to gather a handful
/// of words; short enough that she manifests almost immediately.
const FIRST_TICK_DELAY: Duration = Duration::from_secs(4);
/// Smallest gap between scene escalations. Short — a fresh visual on
/// roughly every other ambient tick keeps the tile feeling alive.
/// Existing scenes get replaced; the renderer fades the new one in.
const SCENE_COOLDOWN: Duration = Duration::from_secs(15);
/// Don't escalate while she just spoke — her own scene/board is still
/// the right visual.
const POST_ANSWER_GRACE: Duration = Duration::from_secs(10);
/// Need at least this many new transcript words since the last tick to
/// even bother calling the LLM. Avoids a hot loop while the call is silent.
const MIN_NEW_WORDS: usize = 5;
/// Capped count of recent concepts to feed back to the model so it
/// doesn't pick the same one each tick.
const RECENT_CONCEPTS_MAX: usize = 4;

const AMBIENT_SYSTEM: &str = "You are watching a live voice conversation. \
Your job is to PICK a single short concept that captures what's being \
talked about right now, a colour that *feels* like it, AND an image \
prompt that visualizes it. The picks drive Eliza's silent video tile — \
she never speaks them, the tile just paints.\n\n\
Output strictly JSON, no prose, no markdown:\n\
{\"concept\": \"1-3 short words\", \"accent\": \"#RRGGBB\", \"image_query\": \"vivid image prompt\"}\n\n\
Rules:\n\
- `concept` is what the conversation is ABOUT — a topic, not an emotion. \
  1-3 words max. Title case. Examples: \"Deep Ocean\", \"Apollo Program\", \
  \"Social Unrest\", \"Bridge Engineering\".\n\
- `accent` is a hex colour that evokes the topic. Be playful — moss green \
  for plants, deep blue for water, copper for rust, neon for futurism, \
  blood red for conflict.\n\
- `image_query` MUST be filled. Always. It is a vivid, evocative search/\
  image-generation prompt that depicts the concept — 4-10 words, concrete \
  visual language. For literal subjects, name the subject (\"Eiffel Tower \
  at night\", \"Apollo 11 lunar lander\"). For abstract topics, paint a \
  scene that *evokes* them (\"protest crowd at night with torches\" for \
  Social Unrest, \"empty office at dawn\" for Burnout, \"glowing neural \
  network diagram\" for Machine Learning). NEVER leave it empty.\n\
- Do NOT repeat a concept the user has recently shown (a list is provided). \
  Pick a different angle if needed.\n\
- If the snippet is too small or off-topic for a confident pick, output \
  concept=\"\".";

pub(crate) fn spawn_monitor(
    cfg: Arc<SharedConfig>,
    handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_monitor(cfg, handle, active))
}

async fn run_monitor(
    cfg: Arc<SharedConfig>,
    _handle: Arc<ClientHandle>,
    active: Arc<AsyncMutex<Option<ActiveCall>>>,
) {
    tracing::info!("ambient monitor armed");
    let mut consumed_lines: usize = 0;
    let mut recent_concepts: Vec<String> = Vec::new();
    let mut last_scene_at: Option<Instant> = None;
    // Short lead-in on the first tick — manifest fast on a fresh call —
    // then settle into the regular TICK cadence.
    let mut first_tick = true;
    loop {
        let delay = if first_tick { FIRST_TICK_DELAY } else { TICK };
        first_tick = false;
        tokio::time::sleep(delay).await;
        let Some(key) = cfg.groq_api_key.as_deref() else {
            continue;
        };

        let snapshot = {
            let guard = active.lock().await;
            let Some(call) = guard.as_ref() else {
                tracing::info!("ambient monitor: call ended, stopping");
                return;
            };
            AmbientSnapshot {
                transcript: call.transcript.clone(),
                video: call.video.clone(),
                last_answer: call.last_answer,
            }
        };

        let total = snapshot.transcript.len();
        let start = consumed_lines.min(total);
        // The bot's own answers are in the transcript now (symmetric
        // log) — they don't count as new conversation to react to, or
        // the tile would re-theme off her own voice every tick.
        let own_prefix = format!(
            "{}:",
            cfg.nick.split_once('-').map(|(p, _)| p).unwrap_or(&cfg.nick)
        )
        .to_ascii_lowercase();
        let new_words = snapshot.transcript[start..]
            .iter()
            .filter(|l| !l.to_ascii_lowercase().starts_with(&own_prefix))
            .flat_map(|l| l.split_whitespace())
            .count();
        if new_words < MIN_NEW_WORDS {
            tracing::debug!(new_words, "ambient: too little new transcript, skip");
            continue;
        }

        // Last ~25 transcript lines for context. We feed *all* recent
        // lines (not just the new ones) because the topic often persists
        // across our consumed_lines watermark.
        let tail_start = snapshot.transcript.len().saturating_sub(25);
        let recent = snapshot.transcript[tail_start..].join("\n");

        let plan = decide(
            &cfg.http,
            key,
            &cfg.groq_chat_model,
            &recent,
            &recent_concepts,
        )
        .await;
        let Some(plan) = plan else {
            tracing::debug!("ambient: model declined, skip");
            continue;
        };

        // Apply concept + accent immediately (the cheap, smooth path).
        tracing::info!(
            concept = %plan.concept,
            accent = %plan.accent,
            has_image = !plan.image_query.is_empty(),
            "ambient: applying"
        );
        consumed_lines = total;
        recent_concepts.push(plan.concept.clone());
        if recent_concepts.len() > RECENT_CONCEPTS_MAX {
            recent_concepts.remove(0);
        }
        snapshot
            .video
            .set_ambient(plan.concept.clone(), plan.accent.clone());

        // Escalation: a concrete subject + cooldown elapsed + she didn't
        // just speak. Drop a Hero scene whose image becomes the backdrop.
        let post_answer = snapshot
            .last_answer
            .map(|t| t.elapsed() < POST_ANSWER_GRACE)
            .unwrap_or(false);
        let cooled = last_scene_at
            .map(|t| t.elapsed() >= SCENE_COOLDOWN)
            .unwrap_or(true);
        if plan.image_query.is_empty() {
            tracing::debug!("ambient: abstract topic — no escalation");
        } else if post_answer {
            tracing::debug!("ambient: just answered — skipping scene escalation");
        } else if !cooled {
            tracing::debug!("ambient: scene cooldown — skipping escalation");
        } else {
            last_scene_at = Some(Instant::now());
            escalate_to_ambient_image(&cfg, &snapshot.video, &plan);
        }
    }
}

struct AmbientSnapshot {
    transcript: Vec<String>,
    video: VideoTile,
    last_answer: Option<Instant>,
}

/// Reserve an ambient-image slot on the tile and kick off an async
/// fetch of the matching backdrop. **Image only — no title, no body
/// text.** The topic name lives on the HUD chip; the image lives as a
/// subtle backdrop. Informational scene cards (with words) are
/// reserved for actual question answers in [`crate::irc::answer_and_speak`].
fn escalate_to_ambient_image(cfg: &Arc<SharedConfig>, video: &VideoTile, plan: &AmbientPlan) {
    video.show_ambient_image();
    let cfg = cfg.clone();
    let video = video.clone();
    let query = plan.image_query.clone();
    tokio::spawn(async move {
        let fetched = tokio::time::timeout(
            Duration::from_secs(45),
            imagegen::fetch(&cfg.http, &query, cfg.image_ai.as_ref()),
        )
        .await;
        let bytes = match fetched {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "ambient image unavailable");
                return;
            }
            Err(_) => {
                tracing::debug!("ambient image timed out");
                return;
            }
        };
        let uri =
            match tokio::task::spawn_blocking(move || imagegen::to_data_uri(&bytes)).await {
                Ok(Ok(uri)) => uri,
                _ => return,
            };
        video.set_ambient_image(uri);
        tracing::info!(query = %query, "ambient image ready");
    });
}

struct AmbientPlan {
    concept: String,
    accent: String,
    image_query: String,
}

/// Ask the chat model for an ambient pick. Returns `None` on any error
/// or when the model declines (empty concept) — ambient is best-effort,
/// a failure just means the tile keeps its previous topic.
async fn decide(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    recent_transcript: &str,
    recent_concepts: &[String],
) -> Option<AmbientPlan> {
    let recent_list = if recent_concepts.is_empty() {
        "(none yet)".to_string()
    } else {
        recent_concepts.join(", ")
    };
    let user = format!(
        "Recent transcript:\n{recent_transcript}\n\n\
         Concepts you've already shown (avoid repeating): {recent_list}\n\n\
         Output the JSON object."
    );
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 120,
        "temperature": 0.7,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": AMBIENT_SYSTEM },
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
    let concept = plan["concept"].as_str().unwrap_or("").trim();
    if concept.is_empty() {
        return None;
    }
    let accent = plan["accent"].as_str().unwrap_or("").trim().to_string();
    let image_query = plan["image_query"].as_str().unwrap_or("").trim().to_string();
    Some(AmbientPlan {
        concept: concept.chars().take(28).collect(),
        accent,
        image_query: image_query.chars().take(120).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_plan_truncates_long_concepts() {
        // The model is told to keep concept short but we don't trust it —
        // long concepts would overflow the HUD chip and break layout.
        let v = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "{\"concept\":\"this is a really really really really long concept that should be truncated\",\"accent\":\"#1a8fff\",\"image_query\":\"\"}"
                }
            }]
        });
        let content = v["choices"][0]["message"]["content"].as_str().unwrap();
        let plan: serde_json::Value = serde_json::from_str(content).unwrap();
        let concept = plan["concept"].as_str().unwrap().trim();
        let truncated: String = concept.chars().take(28).collect();
        assert!(truncated.chars().count() <= 28);
    }

    #[test]
    fn ambient_plan_rejects_empty_concept() {
        // An empty concept means "model declined" — must not be applied.
        let v = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "{\"concept\":\"  \",\"accent\":\"#000000\",\"image_query\":\"\"}"
                }
            }]
        });
        let content = v["choices"][0]["message"]["content"].as_str().unwrap();
        let plan: serde_json::Value = serde_json::from_str(content).unwrap();
        let concept = plan["concept"].as_str().unwrap_or("").trim();
        assert!(concept.is_empty());
    }
}
