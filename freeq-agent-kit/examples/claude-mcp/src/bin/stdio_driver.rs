//! Manual-test driver. JSON-lines on stdin/stdout. Lets you connect
//! to a channel and play conversation interactively without booting
//! the MCP server.
//!
//! Commands (stdin, one JSON object per line):
//!   {"cmd":"connect","channel":"#avtest","nick":"claude","start_if_idle":false}
//!   {"cmd":"say","text":"Hello, it's Claude."}
//!   {"cmd":"disconnect"}
//!
//! Events (stdout):
//!   {"event":"connected","channel":"#avtest","nick":"claude"}
//!   {"event":"transcript","speaker":"alice","text":"hi claude","addressed":true,"question":"hi","timestamp_ms":...}
//!   {"event":"error","message":"..."}
//!
//! Env vars (read at connect time):
//!   GROQ_API_KEY, ELEVENLABS_API_KEY,
//!   FREEQ_SERVER (default wss://irc.freeq.at/irc),
//!   FREEQ_ELEVEN_VOICE_ID, FREEQ_ELEVEN_MODEL.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use freeq_claude_mcp::{OrcConfig, Orchestrator, SayPriority, TileOverlay};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

fn default_one() -> u32 {
    1
}

fn default_addressed() -> SayPriority {
    SayPriority::Addressed
}

fn default_recall_limit() -> u32 {
    5
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Cmd {
    Connect {
        channel: String,
        nick: Option<String>,
        identity_name: Option<String>,
        #[serde(default)]
        start_if_idle: bool,
        #[serde(default)]
        peer_agents: Vec<String>,
    },
    Say {
        text: String,
        #[serde(default = "default_addressed")]
        priority: SayPriority,
    },
    Post {
        text: String,
    },
    ShowCard {
        title: String,
        #[serde(default)]
        bullets: Vec<String>,
    },
    ShowQuote {
        text: String,
        #[serde(default)]
        source: Option<String>,
    },
    ShowFile {
        path: String,
        #[serde(default = "default_one")]
        line_start: u32,
        #[serde(default)]
        line_end: Option<u32>,
    },
    Status {
        label: String,
        #[serde(default)]
        thinking: Option<bool>,
    },
    Clear,
    Look {
        #[serde(default)]
        speaker: Option<String>,
        #[serde(default)]
        question: Option<String>,
    },
    Recall {
        query: String,
        #[serde(default = "default_recall_limit")]
        limit: u32,
    },
    Participants,
    Disconnect,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "freeq_claude_mcp=info,freeq_eliza=info,freeq_av=info,info",
                )
            }),
        )
        .init();

    let orc: Arc<Mutex<Option<Arc<Orchestrator>>>> = Arc::new(Mutex::new(None));
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();

    while let Ok(Some(line)) = stdin.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cmd: Cmd = match serde_json::from_str(line) {
            Ok(c) => c,
            Err(e) => {
                emit_error(&format!("bad command: {e}"));
                continue;
            }
        };
        match cmd {
            Cmd::Connect {
                channel,
                nick,
                identity_name,
                start_if_idle,
                peer_agents,
            } => {
                if orc.lock().await.is_some() {
                    emit_error("already connected");
                    continue;
                }
                let nick = nick.unwrap_or_else(|| "claude".to_string());
                let identity_name = identity_name.unwrap_or_else(|| nick.clone());
                let cfg = OrcConfig {
                    server: std::env::var("FREEQ_SERVER")
                        .unwrap_or_else(|_| "wss://irc.freeq.at/irc".to_string()),
                    channel: channel.clone(),
                    nick: nick.clone(),
                    identity_name,
                    start_if_idle,
                    groq_api_key: env_nonempty("GROQ_API_KEY"),
                    elevenlabs_api_key: env_nonempty("ELEVENLABS_API_KEY"),
                    elevenlabs_voice_id: std::env::var("FREEQ_ELEVEN_VOICE_ID")
                        .unwrap_or_else(|_| "aj0fZfXTBc7E3By4X8L2".to_string()),
                    elevenlabs_model: std::env::var("FREEQ_ELEVEN_MODEL")
                        .unwrap_or_else(|_| "eleven_turbo_v2_5".to_string()),
                    sfu_url_override: std::env::var("FREEQ_SFU_URL").ok(),
                    peer_agents,
                    ghostly_character: std::env::var("FREEQ_CHARACTER")
                        .unwrap_or_else(|_| "eliza".to_string()),
                    volunteer_cooldown_secs: std::env::var("FREEQ_VOLUNTEER_COOLDOWN")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(30),
                    emit_diagrams: std::env::var("FREEQ_EMIT_DIAGRAMS")
                        .map(|s| s != "0" && s.to_lowercase() != "false")
                        .unwrap_or(true),
                    enable_memory: std::env::var("FREEQ_MEMORY")
                        .map(|s| s != "0" && s.to_lowercase() != "false")
                        .unwrap_or(true),
                    barge_in: std::env::var("FREEQ_BARGE_IN")
                        .map(|s| s != "0" && s.to_lowercase() != "false")
                        .unwrap_or(true),
                    deepgram_api_key: env_nonempty("DEEPGRAM_API_KEY"),
                    deepgram_model: std::env::var("DEEPGRAM_MODEL")
                        .unwrap_or_else(|_| "nova-3".to_string()),
                };
                match Orchestrator::connect(cfg).await {
                    Ok(o) => {
                        let o = Arc::new(o);
                        *orc.lock().await = Some(o.clone());
                        emit_event(&serde_json::json!({
                            "event": "connected",
                            "channel": channel,
                            "nick": nick,
                        }));
                        let orc_for_pump = o.clone();
                        tokio::spawn(async move {
                            loop {
                                let batch = orc_for_pump.recv_batch(Duration::from_secs(30)).await;
                                for t in batch {
                                    emit_event(&serde_json::json!({
                                        "event": "transcript",
                                        "speaker": t.speaker,
                                        "text": t.text,
                                        "addressed": t.addressed,
                                        "question": t.question,
                                        "timestamp_ms": t.timestamp_ms,
                                    }));
                                }
                            }
                        });
                    }
                    Err(e) => emit_error(&format!("connect failed: {e:#}")),
                }
            }
            Cmd::Say { text, priority } => {
                let g = orc.lock().await;
                let Some(o) = g.as_ref() else {
                    emit_error("not connected");
                    continue;
                };
                match o.say(&text, priority).await {
                    Ok(r) if r.suppressed => {
                        emit_event(&serde_json::json!({
                            "event": "suppressed",
                            "text": text,
                            "cooldown_remaining_secs": r.cooldown_remaining_secs,
                        }));
                    }
                    Ok(_) => emit_event(&serde_json::json!({
                        "event": "spoke",
                        "text": text,
                    })),
                    Err(e) => emit_error(&format!("say failed: {e:#}")),
                }
            }
            Cmd::Post { text } => {
                let g = orc.lock().await;
                let Some(o) = g.as_ref() else {
                    emit_error("not connected");
                    continue;
                };
                if let Err(e) = o.post(&text).await {
                    emit_error(&format!("post failed: {e:#}"));
                } else {
                    emit_event(&serde_json::json!({
                        "event": "posted",
                        "text": text,
                    }));
                }
            }
            Cmd::ShowCard { title, bullets } => {
                if let Some(o) = orc.lock().await.as_ref() {
                    o.control.set_overlay(TileOverlay::Card { title, bullets });
                    emit_event(&serde_json::json!({ "event": "shown", "kind": "card" }));
                } else {
                    emit_error("not connected");
                }
            }
            Cmd::ShowQuote { text, source } => {
                if let Some(o) = orc.lock().await.as_ref() {
                    o.control.set_overlay(TileOverlay::Quote { text, source });
                    emit_event(&serde_json::json!({ "event": "shown", "kind": "quote" }));
                } else {
                    emit_error("not connected");
                }
            }
            Cmd::ShowFile {
                path,
                line_start,
                line_end,
            } => {
                if let Some(o) = orc.lock().await.as_ref() {
                    match std::fs::read_to_string(&path) {
                        Ok(body) => {
                            let end = line_end.unwrap_or(line_start + 24).max(line_start);
                            let lines: Vec<String> = body
                                .lines()
                                .skip((line_start - 1) as usize)
                                .take((end - line_start + 1) as usize)
                                .map(|s| s.to_string())
                                .collect();
                            o.control.set_overlay(TileOverlay::File {
                                path: path.clone(),
                                lines,
                                line_start,
                            });
                            emit_event(
                                &serde_json::json!({ "event": "shown", "kind": "file", "path": path }),
                            );
                        }
                        Err(e) => emit_error(&format!("read {}: {e}", path)),
                    }
                } else {
                    emit_error("not connected");
                }
            }
            Cmd::Status { label, thinking } => {
                if let Some(o) = orc.lock().await.as_ref() {
                    let thinking = thinking.unwrap_or(label.eq_ignore_ascii_case("thinking"));
                    o.control.set_thinking(thinking);
                    if label.is_empty() {
                        o.control.set_overlay(TileOverlay::None);
                    } else {
                        o.control.set_overlay(TileOverlay::Status {
                            label: label.clone(),
                        });
                    }
                    emit_event(
                        &serde_json::json!({ "event": "status", "label": label, "thinking": thinking }),
                    );
                } else {
                    emit_error("not connected");
                }
            }
            Cmd::Clear => {
                if let Some(o) = orc.lock().await.as_ref() {
                    o.control.set_overlay(TileOverlay::None);
                    emit_event(&serde_json::json!({ "event": "cleared" }));
                } else {
                    emit_error("not connected");
                }
            }
            Cmd::Look { speaker, question } => {
                let o = orc.lock().await.as_ref().cloned();
                let Some(o) = o else {
                    emit_error("not connected");
                    continue;
                };
                match describe_via_vision(
                    &o,
                    speaker.as_deref(),
                    question
                        .as_deref()
                        .unwrap_or("What do you see in this frame?"),
                )
                .await
                {
                    Ok((picked, description)) => emit_event(&serde_json::json!({
                        "event": "look",
                        "speaker": picked,
                        "description": description,
                    })),
                    Err(e) => emit_error(&format!("look failed: {e:#}")),
                }
            }
            Cmd::Recall { query, limit } => {
                let g = orc.lock().await;
                let Some(o) = g.as_ref() else {
                    emit_error("not connected");
                    continue;
                };
                match o.recall(&query, limit as usize) {
                    Ok(recs) => emit_event(&serde_json::json!({
                        "event": "recall",
                        "query": query,
                        "hits": recs.iter().map(|r| serde_json::json!({
                            "speaker": r.asker,
                            "text": r.question,
                            "ts": r.ts,
                        })).collect::<Vec<_>>(),
                    })),
                    Err(e) => emit_error(&format!("recall failed: {e:#}")),
                }
            }
            Cmd::Participants => {
                if let Some(o) = orc.lock().await.as_ref() {
                    let nicks = o.participants().await;
                    emit_event(&serde_json::json!({
                        "event": "participants",
                        "nicks": nicks,
                    }));
                } else {
                    emit_error("not connected");
                }
            }
            Cmd::Disconnect => {
                let taken = orc.lock().await.take();
                let Some(o) = taken else {
                    emit_error("not connected");
                    continue;
                };
                let _ = o.disconnect().await;
                emit_event(&serde_json::json!({ "event": "disconnected" }));
            }
        }
    }
    Ok(())
}

fn emit_event(v: &serde_json::Value) {
    println!("{}", v);
}

fn emit_error(msg: &str) {
    emit_event(&serde_json::json!({ "event": "error", "message": msg }));
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

async fn describe_via_vision(
    orc: &Orchestrator,
    speaker: Option<&str>,
    question: &str,
) -> anyhow::Result<(String, String)> {
    use anyhow::anyhow;
    let Some(api_key) = env_nonempty("GROQ_API_KEY") else {
        return Err(anyhow!("GROQ_API_KEY not set — vision unavailable"));
    };
    let model = std::env::var("FREEQ_VISION_MODEL")
        .unwrap_or_else(|_| "meta-llama/llama-4-scout-17b-16e-instruct".to_string());
    let Some((picked, frame)) = orc.latest_frame(speaker).await else {
        return Err(anyhow!(
            "no video frame available{}",
            speaker.map(|s| format!(" for {s}")).unwrap_or_default()
        ));
    };
    let uri = freeq_eliza::vision::frame_to_jpeg_data_uri(&frame)?;
    let client = reqwest::Client::new();
    let text = freeq_eliza::vision::describe(&client, &api_key, &model, question, "", &uri).await?;
    Ok((picked, text))
}
