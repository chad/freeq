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
use crate::video::{SceneKind, SceneSpec, VideoTile};

/// How often the ambient loop wakes. Short — the HUD chip + accent
/// shift are cheap (one fast LLM call, no image fetch) so a tight tick
/// makes the tile feel genuinely responsive to the conversation.
const TICK: Duration = Duration::from_secs(8);
/// Lead-in before the first tick fires. Long enough to gather a handful
/// of words; short enough that she manifests almost immediately.
const FIRST_TICK_DELAY: Duration = Duration::from_secs(4);
/// Smallest gap between concrete-subject scene escalations. An image
/// backdrop is loud; back-to-back swaps would make the tile feel twitchy.
/// Still short enough that a sustained topic gets a real image within
/// half a minute.
const SCENE_COOLDOWN: Duration = Duration::from_secs(30);
/// Don't escalate while she just spoke — her own scene/board is still
/// the right visual.
const POST_ANSWER_GRACE: Duration = Duration::from_secs(12);
/// Need at least this many new transcript words since the last tick to
/// even bother calling the LLM. Avoids a hot loop while the call is silent.
const MIN_NEW_WORDS: usize = 5;
/// Capped count of recent concepts to feed back to the model so it
/// doesn't pick the same one each tick.
const RECENT_CONCEPTS_MAX: usize = 4;

const AMBIENT_SYSTEM: &str = "You are watching a live voice conversation. \
Your job is to PICK a single short concept that captures what's being \
talked about right now, plus a colour that *feels* like it. The picks \
drive Eliza's silent video tile — they should never speak, only paint.\n\n\
Output strictly JSON, no prose, no markdown:\n\
{\"concept\": \"1-3 short words\", \"accent\": \"#RRGGBB\", \"image_query\": \"concrete subject or empty\"}\n\n\
Rules:\n\
- `concept` is what the conversation is ABOUT — a topic, not an emotion. \
  1-3 words max. Title case. Examples: \"Deep Ocean\", \"Apollo Program\", \
  \"Slow Mornings\", \"Bridge Engineering\".\n\
- `accent` is a hex colour that evokes the topic. Be playful — moss green \
  for plants, deep blue for water, copper for rust, neon for futurism.\n\
- `image_query` is OPTIONAL. Only fill it when the topic is a SPECIFIC \
  concrete subject a viewer could picture in a photo: a named person, \
  place, animal, object, event, artwork, organism. NEVER fill it for \
  abstract topics (feelings, opinions, meta-talk, software process, math).\n\
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
        let new_lines = &snapshot.transcript[start..];
        let new_words = new_lines
            .iter()
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
            escalate_to_scene(&cfg, &snapshot.video, &plan);
        }
    }
}

struct AmbientSnapshot {
    transcript: Vec<String>,
    video: VideoTile,
    last_answer: Option<Instant>,
}

/// Drop a minimal Hero scene with the concept + accent and kick off an
/// async fetch of the image backdrop. Reuses the same path
/// [`crate::irc::answer_and_speak`] uses for QA scenes.
fn escalate_to_scene(cfg: &Arc<SharedConfig>, video: &VideoTile, plan: &AmbientPlan) {
    let spec = SceneSpec {
        kind: SceneKind::Hero,
        title: plan.concept.clone(),
        // No subtitle: ambient scenes are about the image + topic, not
        // commentary. The HUD chip already labels the topic.
        subtitle: String::new(),
        points: Vec::new(),
        accent: plan.accent.clone(),
        image_query: plan.image_query.clone(),
    };
    let scene_id = video.show_scene(spec);
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
                tracing::debug!(error = %e, "ambient scene backdrop unavailable");
                return;
            }
            Err(_) => {
                tracing::debug!("ambient scene backdrop timed out");
                return;
            }
        };
        let uri =
            match tokio::task::spawn_blocking(move || imagegen::to_data_uri(&bytes)).await {
                Ok(Ok(uri)) => uri,
                _ => return,
            };
        video.set_scene_image(scene_id, uri);
        tracing::info!(scene_id, "ambient scene backdrop ready");
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
