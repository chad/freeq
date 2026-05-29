//! freeq-eliza: sample agent that joins freeq AV sessions and
//! transcribes the audio.
//!
//! Lifecycle:
//! 1. Load (or auto-create) a did:key identity at
//!    `~/.freeq/bots/<name>/key.ed25519`.
//! 2. Connect to a freeq IRC server with SASL ATPROTO-CHALLENGE using
//!    that key.
//! 3. Join the configured channels and watch for
//!    `+freeq.at/av-state=started` TAGMSGs.
//! 4. On a session start, send `av-join`, open a MoQ subscriber via the
//!    SFU, and subscribe to every participant broadcast.
//! 5. Tap decoded PCM out of each remote audio track via a custom
//!    `AudioStreamFactory`, run whisper-rs over rolling 10s windows.
//! 6. Post each utterance as a PRIVMSG into the channel:
//!    `[transcript] <nick>: <text>`.
//! 7. On `av-state=ended` for our session, send the rolling transcript
//!    to the Anthropic API for a summary + action items and post that
//!    back to the channel.
//!
//! Run as a one-shot for development:
//!   ANTHROPIC_API_KEY=sk-... cargo run --release --bin freeq-eliza -- \
//!     --server wss://irc.freeq.at/irc \
//!     --channel '#avtest' \
//!     --model-path ./models/ggml-small.en.bin
//!
//! Identity files live at `~/.freeq/bots/eliza/`. First run creates
//! them; subsequent runs reuse the same DID.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use freeq_eliza::{character_profile, identity, imagegen, irc, stt};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "freeq-eliza",
    about = "Joins freeq AV sessions, transcribes audio, posts the transcript + summary back to the channel."
)]
struct Cli {
    /// IRC server URL. `wss://` / `https://` → WebSocket; `host:port` →
    /// raw TCP.
    #[arg(long, default_value = "wss://irc.freeq.at/irc")]
    server: String,

    /// Channels to live in. The bot only transcribes calls that start
    /// in channels it's a member of.
    #[arg(long, default_values_t = vec!["#avtest".to_string()])]
    channel: Vec<String>,

    /// Bot identity name. Files live at `~/.freeq/bots/<name>/`. When
    /// not given, defaults to the active character ("eliza" for the
    /// SVG backend; the `--ghostly-character` value for the particles
    /// backend) — so each character gets its own DID + nick rather
    /// than colliding on "eliza".
    #[arg(long)]
    name: Option<String>,

    /// IRC nick. Defaults to the identity name.
    #[arg(long)]
    nick: Option<String>,

    /// Path to a ggml whisper.cpp model — used only by the local STT
    /// backend (the `stt` cargo feature). Ignored when `GROQ_API_KEY`
    /// is set, which is the preferred path.
    #[arg(long, default_value = "./models/ggml-small.en.bin")]
    model_path: PathBuf,

    /// Groq transcription model. Used when `GROQ_API_KEY` is set in
    /// the environment. `whisper-large-v3-turbo` is fast + accurate.
    #[arg(long, default_value = "whisper-large-v3-turbo")]
    groq_model: String,

    /// Groq chat model for the visual board (scene generation).
    #[arg(long, default_value = "llama-3.3-70b-versatile")]
    groq_chat_model: String,

    /// Model for answering questions addressed to the bot. Default is
    /// Anthropic's `claude-opus-4-7` (slowest but highest quality;
    /// requires `ANTHROPIC_API_KEY`). Falls back to Groq when given a
    /// non-claude model — `groq/compound` is the agentic web-search
    /// option, `groq/compound-mini` lower latency, `llama-3.3-70b-versatile`
    /// no web. Flag name kept as `--groq-answer-model` for back-compat;
    /// `claude-*` routes to Anthropic Messages automatically.
    #[arg(long, default_value = "claude-opus-4-7")]
    groq_answer_model: String,

    /// Groq vision model for questions about a participant's shared
    /// screen or camera (e.g. "Eliza, what's on my screen?").
    #[arg(long, default_value = "meta-llama/llama-4-scout-17b-16e-instruct")]
    vision_model: String,

    /// ElevenLabs voice + model for speaking answers aloud. Reads
    /// `ELEVENLABS_API_KEY` from the environment.
    #[arg(long, default_value = "aj0fZfXTBc7E3By4X8L2")]
    elevenlabs_voice: String,
    #[arg(long, default_value = "eleven_turbo_v2_5")]
    elevenlabs_model: String,

    /// AI image-generation provider for scene backdrops, used as a
    /// fallback when Wikipedia has no image: "openai" or "gemini". The
    /// API key is read from the environment (OPENAI_API_KEY, or
    /// GEMINI_API_KEY / GOOGLE_API_KEY). With no key, backdrops come
    /// from Wikipedia only.
    #[arg(long, default_value = "openai")]
    image_provider: String,

    /// Image model for the AI backdrop fallback.
    #[arg(long, default_value = "gpt-image-1-mini")]
    image_model: String,

    /// Skip the end-of-call summary even if `ANTHROPIC_API_KEY` is set.
    #[arg(long)]
    no_summary: bool,

    /// Anthropic model used for the end-of-call summary. Reads
    /// `ANTHROPIC_API_KEY` from the environment.
    #[arg(long, default_value = "claude-sonnet-4-5")]
    summary_model: String,

    /// Window in seconds of audio to accumulate before running whisper.
    /// Shorter = lower latency, more re-decode work. Default 10s.
    #[arg(long, default_value_t = 10.0)]
    window_secs: f32,

    /// Initiate a call: send `av-start` for this channel right after
    /// joining, instead of only waiting for someone else to start one.
    /// The channel must also be in `--channel`.
    #[arg(long)]
    start_session_in: Option<String>,

    /// Override the MoQ SFU URL. Default: derived from `--server` as
    /// `https://<host>/av/moq`. Point at the SFU's QUIC port to force
    /// the low-latency transport, e.g.
    /// `https://irc.freeq.at:4443/av/moq`.
    #[arg(long)]
    sfu_url: Option<String>,

    /// Disable the proactive monitor — Eliza only speaks when addressed.
    /// Useful when she's chatty and you want quiet.
    #[arg(long)]
    no_proactive: bool,

    /// Disable the ambient monitor — the tile reverts to a static HUD
    /// and skips topic/image manifesting while she listens. Cuts a small
    /// extra cost (one fast LLM call every 20s) when you don't want it.
    #[arg(long)]
    no_ambient: bool,

    /// Video tile renderer: `svg` (default — full freeq cyberpunk
    /// presence with EQ strip, scene cards, ambient HUD, vision PiP) or
    /// `particles` (ghostly particle face — face only, no overlays).
    #[arg(long, default_value = "svg")]
    render_backend: String,

    /// Ghostly character used when `--render-backend particles`. One of
    /// `eliza`, `narrator`, `utopia`, `oblivion`.
    #[arg(long, default_value = "eliza")]
    ghostly_character: String,

    /// Other agent nicks this bot recognises as peers — e.g.
    /// `--peer-agents oblivion,utopia` when running Eliza alongside
    /// the other two for a multi-agent demo. The bot will respond
    /// when one peer addresses it by name, but a streak of 3+
    /// peer-only addresses (no human break) triggers a chatter guard
    /// that suppresses further replies until a human speaks. Empty
    /// (the default) = lone agent, no special handling.
    #[arg(long, value_delimiter = ',', num_args = 0..)]
    peer_agents: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("freeq_eliza=info,freeq_sdk=info,info")),
        )
        .init();

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let cli = Cli::parse();
    // Identity defaults to the active character — `--render-backend
    // particles --ghostly-character oblivion` lands in
    // `~/.freeq/bots/oblivion/` with a fresh DID and bound nick
    // "oblivion", instead of sharing eliza's identity and getting
    // server-side rebound to her nick. Explicit `--name` always wins.
    let identity_name = cli.name.clone().unwrap_or_else(|| {
        if cli.render_backend == "particles" && !cli.ghostly_character.is_empty() {
            cli.ghostly_character.clone()
        } else {
            "eliza".to_string()
        }
    });
    // Nick defaults to the identity name, so a freshly-minted oblivion
    // identity advertises itself as `oblivion` on the channel.
    let nick = cli.nick.clone().unwrap_or_else(|| identity_name.clone());

    // Load or create the bot's did:key identity.
    let ident = identity::load_or_create(&identity_name).context("loading bot identity")?;
    tracing::info!(did = %ident.did, "bot identity ready");

    // Pick the STT backend. Priority: Groq (hosted, fast, no local
    // toolchain) when GROQ_API_KEY is set; else the local whisper.cpp
    // backend if the `stt` feature was compiled in; else a no-op.
    let stt = Arc::new(build_stt(&cli)?);
    tracing::info!(backend = %stt.label(), "STT backend ready");

    // Anthropic key is now used for TWO things: optional end-of-call
    // summary, AND (by default) the per-question answer model when
    // `--groq-answer-model` is a `claude-*` model. So we always try
    // to load it; `--no-summary` only suppresses the summary path,
    // not the answer-model route.
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if anthropic_key.is_none() {
        tracing::info!(
            "ANTHROPIC_API_KEY not set — claude-* answer models won't work; \
             end-of-call summary also disabled"
        );
    }
    // `--no-summary` only suppresses the end-of-call summary call;
    // it doesn't disable the answer-model route to Anthropic.
    let summary_enabled = !cli.no_summary;

    // Groq key powers STT (above) + question-answering. ElevenLabs key
    // powers TTS. Read both from the environment.
    let groq_api_key = std::env::var("GROQ_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    let elevenlabs_api_key = std::env::var("ELEVENLABS_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    if elevenlabs_api_key.is_none() {
        tracing::info!("ELEVENLABS_API_KEY not set — spoken replies disabled (text only)");
    }

    // Scene backdrops: Wikipedia is always available; an AI image model
    // is an optional fallback, enabled when its key is in the env.
    let image_provider = imagegen::ImageProvider::parse(&cli.image_provider);
    let image_ai = image_provider
        .key_vars()
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|k| !k.trim().is_empty()))
        .map(|key| imagegen::AiImageConfig {
            provider: image_provider,
            model: cli.image_model.clone(),
            key,
        });
    if image_ai.is_none() {
        tracing::info!(
            provider = ?image_provider,
            "no image API key in env — scene backdrops will use Wikipedia only"
        );
    }

    irc::run(irc::RunConfig {
        server: cli.server,
        channels: cli.channel,
        nick,
        ident,
        stt,
        window_secs: cli.window_secs,
        summary_model: cli.summary_model,
        anthropic_key,
        summary_enabled,
        start_session_in: cli.start_session_in,
        sfu_url_override: cli.sfu_url,
        groq_api_key,
        groq_chat_model: cli.groq_chat_model,
        groq_answer_model: cli.groq_answer_model,
        vision_model: cli.vision_model,
        elevenlabs_api_key,
        elevenlabs_model: cli.elevenlabs_model,
        image_ai,
        proactive_enabled: !cli.no_proactive,
        ambient_enabled: !cli.no_ambient,
        render_backend: cli.render_backend.clone(),
        ghostly_character: cli.ghostly_character.clone(),
        // Per-character voice + system-prompt overrides. When the
        // character matches an entry in `character_profile`, swap in
        // its ElevenLabs voice ID and personality. Without a match
        // (e.g. `--ghostly-character eliza`) we fall through to the
        // CLI's `--elevenlabs-voice` and the default Eliza prompt.
        elevenlabs_voice_id: character_profile::by_name(&cli.ghostly_character)
            .map(|p| p.voice_id.to_string())
            .unwrap_or(cli.elevenlabs_voice),
        character_system_prompt: character_profile::by_name(&cli.ghostly_character)
            .map(|p| p.system_prompt.to_string()),
        peer_agents: cli.peer_agents,
    })
    .await
}

/// Choose the STT backend. Groq wins when `GROQ_API_KEY` is present.
fn build_stt(cli: &Cli) -> Result<stt::SttEngine> {
    if let Ok(key) = std::env::var("GROQ_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(stt::SttEngine::groq(key, cli.groq_model.clone()));
        }
    }
    #[cfg(feature = "stt")]
    {
        return stt::SttEngine::local(&cli.model_path).with_context(|| {
            format!("loading local whisper model at {}", cli.model_path.display())
        });
    }
    #[cfg(not(feature = "stt"))]
    {
        tracing::warn!(
            "no GROQ_API_KEY and the `stt` feature is off — transcription is a no-op. \
             Set GROQ_API_KEY or rebuild with --features stt."
        );
        Ok(stt::SttEngine::noop())
    }
}
