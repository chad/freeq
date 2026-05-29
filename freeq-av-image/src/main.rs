//! `freeq-av-image` — publish a still image as an agent's AV video tile.
//!
//! This is the trivial cousin of `ghostly`: where ghostly renders a 12K
//! particle face into the H.264 tile, this just decodes one image and emits
//! it as the video. Everything else — the H.264 encode and the MoQ-over-QUIC
//! transport — is `freeq-av`'s `AvSession`, exactly as `freeq-eliza` uses it.
//!
//! The IRC-side signaling (av-join etc.) is done by the agent (e.g. the Ruby
//! SDK's `Freeq::Av::ImageVideoBackend`); this process only publishes media to
//! the session's broadcast path.
//!
//!   freeq-av-image --sfu https://irc.freeq.at:8080/av/moq \
//!     --session <SID> --nick pixbot --instance 0a1b2c3d --image cat.png

use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use freeq_av::{broadcast_path, AvConfig, AvSession, Speaker};
use iroh_live::media::format::{PixelFormat, VideoFormat, VideoFrame};
use iroh_live::media::traits::VideoSource;

/// 720p tile — matches freeq-eliza, reads well after H.264 + P360.
const TILE_W: u32 = 1280;
const TILE_H: u32 = 720;
const FPS: f64 = 15.0;

#[derive(Parser)]
#[command(about = "Publish a still image as a freeq AV video tile")]
struct Args {
    /// MoQ SFU URL, e.g. https://irc.freeq.at:8080/av/moq
    #[arg(long)]
    sfu: String,
    /// freeq AV session id (from the av-state broadcast / REST discovery).
    #[arg(long)]
    session: String,
    /// Our nick in the call.
    #[arg(long)]
    nick: String,
    /// Our per-device instance id (8 hex chars), matching the av-join.
    #[arg(long)]
    instance: String,
    /// Path to the image to display (PNG or JPEG).
    #[arg(long)]
    image: String,
}

/// A [`VideoSource`] that emits one fixed RGBA frame at a steady cadence, so
/// the encoder produces a continuous static stream (late joiners still get
/// keyframes). The frame is shared `Bytes` — cloning per emit is a refcount.
struct StaticImage {
    data: bytes::Bytes,
    last: Option<Instant>,
    count: u64,
}

impl VideoSource for StaticImage {
    fn name(&self) -> &str {
        "image"
    }

    fn format(&self) -> VideoFormat {
        VideoFormat {
            pixel_format: PixelFormat::Rgba,
            dimensions: [TILE_W, TILE_H],
        }
    }

    fn pop_frame(&mut self) -> anyhow::Result<Option<VideoFrame>> {
        let now = Instant::now();
        let due = self
            .last
            .is_none_or(|t| now.duration_since(t).as_secs_f64() >= 1.0 / FPS);
        if !due {
            return Ok(None);
        }
        self.last = Some(now);
        self.count += 1;
        // First frame at info — confirms a subscriber pulled video and the
        // encoder started (if this never logs, nobody subscribed to our video
        // track). Thereafter periodic at debug to avoid log spam.
        if self.count == 1 {
            tracing::info!("video subscriber present — emitting frames");
        } else if self.count % 150 == 0 {
            tracing::debug!(frames = self.count, "emitting video frames");
        }
        Ok(Some(VideoFrame::new_rgba(
            self.data.clone(),
            TILE_W,
            TILE_H,
            Duration::ZERO,
        )))
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Decode `path` and contain-fit it (preserving aspect) onto a black
/// TILE_W×TILE_H RGBA canvas. Returns the raw RGBA buffer as `Bytes`.
fn load_frame(path: &str) -> anyhow::Result<bytes::Bytes> {
    let src = image::open(path).with_context(|| format!("open image {path}"))?.to_rgba8();
    let (sw, sh) = src.dimensions();
    let scale = (TILE_W as f32 / sw as f32).min(TILE_H as f32 / sh as f32);
    let nw = ((sw as f32 * scale).round() as u32).max(1);
    let nh = ((sh as f32 * scale).round() as u32).max(1);
    let scaled = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Lanczos3);

    let mut canvas = image::RgbaImage::from_pixel(TILE_W, TILE_H, image::Rgba([0, 0, 0, 255]));
    image::imageops::overlay(
        &mut canvas,
        &scaled,
        ((TILE_W - nw) / 2) as i64,
        ((TILE_H - nh) / 2) as i64,
    );
    Ok(bytes::Bytes::from(canvas.into_raw()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let frame = load_frame(&args.image)?;

    let our_broadcast = broadcast_path(&args.session, &args.nick, &args.instance);
    let config = AvConfig {
        sfu_url: args.sfu.parse().context("parse --sfu url")?,
        session_id: args.session.clone(),
        our_broadcast: our_broadcast.clone(),
        my_nick: args.nick.clone(),
    };

    // Silent audio — we publish video only; the queue stays empty so the
    // PushAudioSource serves continuous silence (keeps subscribers attached).
    let (_speaker, audio) = Speaker::new(Arc::new(AtomicU32::new(0)));

    let mut session = AvSession::connect(config, audio, move || StaticImage {
        data: frame.clone(),
        last: None,
        count: 0,
    });
    tracing::info!(broadcast = %our_broadcast, image = %args.image, "publishing image tile");

    // Keep the session (and thus the publish task) alive. recv() yields each
    // tapped participant and only returns None once the session task ends.
    while let Some(p) = session.recv().await {
        tracing::info!(nick = %p.nick, "participant present");
    }
    Ok(())
}
