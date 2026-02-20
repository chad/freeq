//! Chatroom simulator — connects multiple LLM-powered bots to a channel.
//!
//! Each bot has a distinct personality and chats naturally. Useful for
//! generating realistic screenshots and screencasts.
//!
//! Usage:
//!   ANTHROPIC_API_KEY=sk-... cargo run --release --bin chatroom -- \
//!     --server 127.0.0.1:16799 --channel '#demo' --bots 5
//!
//! Or connect to production:
//!   ANTHROPIC_API_KEY=sk-... cargo run --release --bin chatroom -- \
//!     --server irc.freeq.at:6667 --channel '#freeq' --bots 4

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use rand::Rng;
use rand::SeedableRng;
use tokio::sync::Mutex;

use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;

// ── Simple Claude API client (self-contained) ──────────────────────────

mod llm {
    use anyhow::{Context, Result};
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct ApiResponse {
        content: Vec<ContentBlock>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type")]
    enum ContentBlock {
        #[serde(rename = "text")]
        Text { text: String },
        #[allow(dead_code)]
        #[serde(other)]
        Other,
    }

    pub async fn complete(
        api_key: &str,
        model: &str,
        system: &str,
        prompt: &str,
        max_tokens: u32,
    ) -> Result<String> {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
        });

        let resp = reqwest::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Claude API call failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error {status}: {body}");
        }

        let api_resp: ApiResponse = resp.json().await.context("Parse failed")?;
        Ok(api_resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""))
    }
}

// ── CLI ────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "chatroom", about = "LLM-powered chatroom simulator for freeq")]
struct Args {
    /// IRC server address (host:port)
    #[arg(long, default_value = "127.0.0.1:16799")]
    server: String,

    /// Channel to join
    #[arg(long, default_value = "#freeq")]
    channel: String,

    /// Number of bots (max 10)
    #[arg(long, default_value = "5")]
    bots: usize,

    /// Claude model
    #[arg(long, default_value = "claude-sonnet-4-20250514")]
    model: String,

    /// Min seconds between messages per bot
    #[arg(long, default_value = "10")]
    min_delay: u64,

    /// Max seconds between messages per bot
    #[arg(long, default_value = "45")]
    max_delay: u64,

    /// Conversation topic/vibe
    #[arg(long, default_value = "casual tech chat — open source, startups, dev tools, the usual")]
    topic: String,

    /// Use TLS
    #[arg(long)]
    tls: bool,
}

// ── Personas ───────────────────────────────────────────────────────────

struct BotPersona {
    nick: &'static str,
    realname: &'static str,
    personality: &'static str,
}

const PERSONAS: &[BotPersona] = &[
    BotPersona {
        nick: "mika",
        realname: "Mika Chen",
        personality: "Mika Chen, 28, frontend engineer at a climate tech startup in Vancouver. \
Previously contributed to Svelte and maintains a popular CSS animation library (motionkit). \
Studied design at Emily Carr then taught herself to code. Loves generative art, \
makes pieces with p5.js and posts them on fxhash. Uses emoji naturally 🎨 \
Types mostly lowercase. Gets genuinely excited about beautiful UI.",
    },
    BotPersona {
        nick: "dex",
        realname: "Dexter Okafor",
        personality: "Dexter Okafor, 34, SRE at a mid-size fintech in Lagos. \
Maintains several Ansible roles on Galaxy and contributes to Prometheus exporters. \
Has a blog about running infrastructure in Africa that occasionally hits HN front page. \
Dry humor, proper punctuation, skeptical of hype but genuinely curious. \
Will share relevant xkcd links. Loves Rust and NixOS.",
    },
    BotPersona {
        nick: "zara",
        realname: "Zara Okonkwo-Patel",
        personality: "Zara Okonkwo-Patel, 31, ML engineer who left Google Brain to co-found \
a small AI safety research lab. Publishes on arXiv regularly. Cares deeply about \
responsible AI. Shares interesting papers and datasets she finds. Warm, encouraging, \
asks good follow-up questions. Will gently push back on bad takes about AI. \
Has opinions about Python packaging.",
    },
    BotPersona {
        nick: "ghostwire",
        realname: "Sam Reyes",
        personality: "Sam Reyes (ghostwire), 29, nonbinary security researcher and CTF champion. \
Works at a red team consultancy in Berlin. Maintains a popular OSINT toolkit on GitHub. \
Dark humor, slightly paranoid in an endearing way. Shares infosec news and memes. \
Has a newsletter about supply chain attacks. Types fast, occasionally typos.",
    },
    BotPersona {
        nick: "sol",
        realname: "Sol Andersen",
        personality: "Sol Andersen, 36, design engineer — does both design and code. \
Runs a tiny consultancy in Copenhagen. Previously at Vercel on the design systems team. \
Obsessed with typography, whitespace, and accessible color palettes. \
Uses em-dashes liberally — thinks in systems. Shares Figma and Dribbble links. \
Quiet but when she speaks it's usually something worth hearing.",
    },
    BotPersona {
        nick: "priya",
        realname: "Priya Sharma",
        personality: "Priya Sharma, 26, iOS developer and open source contributor. \
Maintains a popular Swift networking library. Lives in Bangalore, works remotely \
for a US startup. Collects mechanical keyboards and fosters rescue cats. \
Very positive energy, shares cat photos (imgur links) and coffee pics. \
Excited about SwiftUI and the fediverse. Uses exclamation marks genuinely!",
    },
    BotPersona {
        nick: "rook",
        realname: "Marcus Webb",
        personality: "Marcus Webb (rook), 41, indie game developer and retro computing collector. \
Made a moderately successful roguelike that got a cult following on itch.io. \
Contributes to LÖVE2D and runs a monthly online demoscene meetup. \
References old games and tracker music. Shares pixel art and chiptune links. \
Thoughtful, doesn't post often but always has something interesting to say.",
    },
    BotPersona {
        nick: "nyx",
        realname: "Nyx Liu",
        personality: "Nyx Liu, 33, open source maintainer (core team on a popular JS framework) \
and developer advocate. Lives in Taipei. Strong opinions on software licensing, \
dependency management, and burnout in OSS. Shares GitHub repos and HN threads. \
Sarcastic but kind underneath. Night owl who's always online at weird hours. \
Will bikeshed on naming things but knows she's doing it.",
    },
    BotPersona {
        nick: "jade",
        realname: "Jade Torres",
        personality: "Jade Torres, 30, founder of a small developer tools company (bootstrapped, \
profitable). Previously a backend engineer at Stripe. Lives in Mexico City. \
Skeptical of VC hype, loves boring technology. Shares business insights without \
being preachy. Quick-witted, good at one-liners. Into running, mezcal, and \
mechanical watch modding. Types concisely.",
    },
    BotPersona {
        nick: "byte",
        realname: "River Kim",
        personality: "River Kim (byte), 23, recent CS grad and junior developer at their first job \
(a small agency in Portland). Eager, asks great questions, gets genuinely excited \
when they learn something new. Contributes to docs and first-good-issue bugs. \
Shares things they just discovered with infectious enthusiasm. Uses they/them. \
Occasionally overwhelmed but always bouncing back.",
    },
];

// ── Shared state ───────────────────────────────────────────────────────

/// Owned version of BotPersona for spawned tasks
struct OwnedPersona {
    nick: String,
    realname: String,
    personality: String,
}

/// Recent chat messages visible to all bots
type ChatLog = Arc<Mutex<VecDeque<(String, String)>>>;

// ── Main ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let api_key =
        std::env::var("ANTHROPIC_API_KEY").context("Set ANTHROPIC_API_KEY environment variable")?;
    let bot_count = args.bots.min(PERSONAS.len());
    let chat_log: ChatLog = Arc::new(Mutex::new(VecDeque::with_capacity(100)));

    println!("🤖 Chatroom simulator");
    println!("   Server:  {}", args.server);
    println!("   Channel: {}", args.channel);
    println!("   Bots:    {bot_count}");
    println!("   Model:   {}", args.model);
    println!("   Delay:   {}–{}s", args.min_delay, args.max_delay);
    println!("   Topic:   {}", args.topic);
    println!();

    for persona in PERSONAS.iter().take(bot_count) {
        let server = args.server.clone();
        let channel = args.channel.clone();
        let topic = args.topic.clone();
        let model = args.model.clone();
        let api_key = api_key.clone();
        let chat_log = Arc::clone(&chat_log);
        let min_delay = args.min_delay;
        let max_delay = args.max_delay;
        let tls = args.tls;
        let nick = persona.nick.to_string();
        let realname = persona.realname.to_string();
        let personality = persona.personality.to_string();

        tokio::spawn(async move {
            let p = OwnedPersona { nick: nick.clone(), realname, personality };
            if let Err(e) = run_bot(
                &server, &channel, &p, &topic, &model, &api_key, chat_log, min_delay,
                max_delay, tls,
            )
            .await
            {
                eprintln!("❌ {nick} error: {e}");
            }
        });

        // Stagger connections so they don't all join at once
        tokio::time::sleep(Duration::from_millis(2000)).await;
    }

    println!("All bots connected. Ctrl+C to stop.\n");

    // Wait forever
    tokio::signal::ctrl_c().await?;
    println!("\n👋 Shutting down...");
    Ok(())
}

// ── Bot loop ───────────────────────────────────────────────────────────

async fn run_bot(
    server: &str,
    channel: &str,
    persona: &OwnedPersona,
    topic: &str,
    model: &str,
    api_key: &str,
    chat_log: ChatLog,
    min_delay: u64,
    max_delay: u64,
    tls: bool,
) -> Result<()> {
    let config = ConnectConfig {
        server_addr: server.to_string(),
        nick: persona.nick.clone(),
        user: persona.nick.clone(),
        realname: persona.realname.clone(),
        tls,
        tls_insecure: false,
    };

    let (handle, mut events) = client::connect(config, None);

    // Wait for registration
    loop {
        match tokio::time::timeout(Duration::from_secs(15), events.recv()).await {
            Ok(Some(Event::Registered { .. })) => break,
            Ok(Some(_)) => continue,
            Ok(None) => anyhow::bail!("{}: connection closed during registration", persona.nick),
            Err(_) => anyhow::bail!("{}: registration timeout", persona.nick),
        }
    }

    println!("  ✅ {} ({}) joined", persona.nick, persona.realname);

    // Join channel
    handle.join(channel).await?;

    // Drain initial flood (MOTD, NAMES, topic, history)
    tokio::time::sleep(Duration::from_secs(3)).await;
    while events.try_recv().is_ok() {}

    let system_prompt = build_system_prompt(persona, topic);
    let mut rng = rand::rngs::StdRng::from_entropy();

    // Stagger first message
    tokio::time::sleep(Duration::from_secs(rng.gen_range(3..12))).await;

    loop {
        // Build context from recent chat
        let prompt = build_prompt(&chat_log, &persona.nick).await;

        // Generate and send message
        match llm::complete(api_key, model, &system_prompt, &prompt, 150).await {
            Ok(raw) => {
                let msg = clean_response(&raw, &persona.nick);
                if !msg.is_empty() {
                    send_message(&handle, channel, &persona.nick, &msg).await;
                    // Record in shared log
                    let display = if let Some(action) = msg.strip_prefix("/me ") {
                        format!("* {} {action}", persona.nick)
                    } else {
                        msg.clone()
                    };
                    let mut log = chat_log.lock().await;
                    log.push_back((persona.nick.to_string(), display));
                    if log.len() > 60 {
                        log.pop_front();
                    }
                }
            }
            Err(e) => {
                eprintln!("  ⚠ {} LLM error: {e:#}", persona.nick);
                // Back off on errors
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }

        // Drain incoming events into shared log
        drain_events(&mut events, &chat_log, &persona.nick).await;

        // Random delay
        let delay = rng.gen_range(min_delay..=max_delay);
        tokio::time::sleep(Duration::from_secs(delay)).await;

        // Drain again after sleeping
        drain_events(&mut events, &chat_log, &persona.nick).await;
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn build_system_prompt(persona: &OwnedPersona, topic: &str) -> String {
    format!(
        r#"You are {nick} in an IRC chat channel. Here is who you are:

{personality}

The channel's general vibe: {topic}

RULES — follow these exactly:
- Output ONLY your next chat message. No prefix, no quotes, no explanation.
- Keep it SHORT. 1-2 sentences max. This is IRC, not email.
- Be natural. Match your character's voice.
- React to what others said when it makes sense. Sometimes change the subject.
- About 1 in 10 messages, share a real URL — could be:
  • A GitHub repo (github.com/real-org/real-project)
  • An image/meme (i.imgur.com/xxxxx.jpg — make up a plausible hash)
  • A YouTube video (youtube.com/watch?v=...)
  • An xkcd (xkcd.com/NNN)
  • A blog post or HN thread
- You can use /me for actions: /me sighs
- Vary your message style. Don't always start the same way.
- Sometimes just react briefly: "lol", "nice", "oh no", "hah true", "^^ this", "+1"
- NEVER break character. NEVER narrate or describe what you're doing.
- NEVER use quotation marks around your message.
- If the channel is quiet, start a new topic naturally."#,
        nick = persona.nick,
        personality = persona.personality,
        topic = topic,
    )
}

async fn build_prompt(chat_log: &ChatLog, my_nick: &str) -> String {
    let log = chat_log.lock().await;
    if log.is_empty() {
        return "The channel has been quiet. Say something to get things going.".to_string();
    }
    let recent: Vec<String> = log
        .iter()
        .map(|(nick, msg)| {
            if nick == my_nick {
                format!("<{nick}> {msg}  [you]")
            } else {
                format!("<{nick}> {msg}")
            }
        })
        .collect();
    format!(
        "Recent chat:\n{}\n\nWrite your next message as {}:",
        recent.join("\n"),
        my_nick
    )
}

fn clean_response(raw: &str, nick: &str) -> String {
    let mut msg = raw.trim().to_string();

    // Strip accidental self-prefix patterns
    for prefix in [
        format!("<{nick}> "),
        format!("{nick}: "),
        format!("[{nick}] "),
    ] {
        if let Some(rest) = msg.strip_prefix(&prefix) {
            msg = rest.to_string();
        }
    }

    // Strip surrounding quotes
    if msg.len() > 2
        && ((msg.starts_with('"') && msg.ends_with('"'))
            || (msg.starts_with('\'') && msg.ends_with('\'')))
    {
        msg = msg[1..msg.len() - 1].to_string();
    }

    // Truncate overly long messages (IRC limit ~)
    if msg.len() > 400 {
        msg.truncate(400);
        if let Some(last_space) = msg.rfind(' ') {
            msg.truncate(last_space);
        }
    }

    msg
}

async fn send_message(handle: &ClientHandle, channel: &str, nick: &str, msg: &str) {
    if let Some(action) = msg.strip_prefix("/me ") {
        let ctcp = format!("\x01ACTION {action}\x01");
        let _ = handle.privmsg(channel, &ctcp).await;
        println!("  \x1b[36m* {nick}\x1b[0m {action}");
    } else {
        let _ = handle.privmsg(channel, msg).await;
        println!("  \x1b[32m<{nick}>\x1b[0m {msg}");
    }
}

async fn drain_events(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    chat_log: &ChatLog,
    my_nick: &str,
) {
    while let Ok(event) = events.try_recv() {
        if let Event::Message {
            from, text, target, ..
        } = event
        {
            // Only log channel messages from others
            if from != my_nick && target.starts_with('#') {
                let mut log = chat_log.lock().await;
                log.push_back((from, text));
                if log.len() > 60 {
                    log.pop_front();
                }
            }
        }
    }
}
