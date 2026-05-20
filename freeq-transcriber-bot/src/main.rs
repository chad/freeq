//! freeq-transcriber-bot: sample agent that joins freeq AV sessions and
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
//!   ANTHROPIC_API_KEY=sk-... cargo run --release --bin freeq-transcriber-bot -- \
//!     --server wss://irc.freeq.at/irc \
//!     --channel '#avtest' \
//!     --model-path ./models/ggml-small.en.bin
//!
//! Identity files live at `~/.freeq/bots/transcriber/`. First run creates
//! them; subsequent runs reuse the same DID.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use freeq_transcriber_bot::{identity, irc, stt};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "freeq-transcriber-bot",
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

    /// Bot identity name. Files live at `~/.freeq/bots/<name>/`.
    #[arg(long, default_value = "transcriber")]
    name: String,

    /// IRC nick. Defaults to the identity name.
    #[arg(long)]
    nick: Option<String>,

    /// Path to a ggml whisper.cpp model. Recommend `ggml-small.en.bin`
    /// for a balance of latency/accuracy on a modern laptop CPU.
    #[arg(long, default_value = "./models/ggml-small.en.bin")]
    model_path: PathBuf,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("freeq_transcriber_bot=info,freeq_sdk=info,info")),
        )
        .init();

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let cli = Cli::parse();
    let nick = cli.nick.clone().unwrap_or_else(|| cli.name.clone());

    // Load or create the bot's did:key identity.
    let ident = identity::load_or_create(&cli.name).context("loading bot identity")?;
    tracing::info!(did = %ident.did, "bot identity ready");

    // Lazy-init whisper. We don't want a missing model file to fail us
    // mid-call; surface it at startup.
    let stt = Arc::new(
        stt::Whisper::load(&cli.model_path)
            .with_context(|| format!("loading whisper model at {}", cli.model_path.display()))?,
    );
    tracing::info!(model = %cli.model_path.display(), "whisper model loaded");

    // Anthropic key is optional — `--no-summary` or a missing key both
    // result in transcript-only mode.
    let anthropic_key = if cli.no_summary {
        None
    } else {
        std::env::var("ANTHROPIC_API_KEY").ok()
    };
    if anthropic_key.is_none() {
        tracing::info!("ANTHROPIC_API_KEY not set or --no-summary; end-of-call summary disabled");
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
    })
    .await
}
