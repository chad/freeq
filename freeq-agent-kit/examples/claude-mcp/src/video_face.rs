//! Ghostly particle face as the bot's video tile.
//!
//! A render thread ticks a `ghostly::FaceState` at 15 fps and drops the
//! latest RGBA frame into a shared cell. `pop_frame` takes from that
//! cell. The orchestrator pumps in audio level and visual state
//! through a `ParticleControl` handle (shared atomics), so the face
//! reacts to listening / speaking / thinking without crossing thread
//! boundaries on the hot path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use ghostly::{
    Character as GhoCharacter, Emotion, FaceState, RenderSettings, Renderer, apply_emotion,
    characters as gho_characters,
};
use iroh_live::media::format::{PixelFormat, VideoFormat, VideoFrame};
use iroh_live::media::traits::VideoSource;
use resvg::tiny_skia::Pixmap;
use resvg::usvg;

use crate::tile_overlay::{OverlayCell, TileOverlay, composite_overlay, new_overlay_cell};

const FPS: u64 = 15;
const PARTICLES: usize = 18_000;
const VIDEO_W: u32 = 640;
const VIDEO_H: u32 = 360;
const EMOTION_INTENSITY: f32 = 0.45;

/// Shared, thread-safe knobs the orchestrator uses to push state into
/// the renderer without owning the render thread. Cloning is cheap
/// (Arc bumps) and intentionally allowed — the orchestrator passes a
/// clone to each background task that drives a particular signal
/// (e.g. the per-participant audio tap pushes into `peer_level`).
#[derive(Clone)]
pub struct ParticleControl {
    /// f32 in [0,1] — the loudness of whoever is currently talking,
    /// excluding ourselves. Drives the "listening" halo.
    pub(crate) peer_level: Arc<AtomicU32>,
    /// f32 in [0,1] — the loudness of our own TTS output. Drives the
    /// face's audio reactivity + speech-onset flash.
    pub(crate) self_level: Arc<AtomicU32>,
    /// True while an LLM call / tool call is in flight. Drives the
    /// rotating "working" arc.
    pub(crate) thinking: Arc<AtomicBool>,
    /// Active visual overlay (scene card / file slice / status chip).
    /// Read on every frame; rewritten by the MCP `freeq_show*` tools.
    pub(crate) overlay: OverlayCell,
}

impl Default for ParticleControl {
    fn default() -> Self {
        Self {
            peer_level: Arc::new(AtomicU32::new(0)),
            self_level: Arc::new(AtomicU32::new(0)),
            thinking: Arc::new(AtomicBool::new(false)),
            overlay: new_overlay_cell(),
        }
    }
}

impl ParticleControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_peer_level(&self, level: f32) {
        self.peer_level
            .store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn set_self_level(&self, level: f32) {
        self.self_level
            .store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn set_thinking(&self, thinking: bool) {
        self.thinking.store(thinking, Ordering::Relaxed);
    }
    pub fn set_overlay(&self, overlay: TileOverlay) {
        if let Ok(mut g) = self.overlay.lock() {
            *g = overlay;
        }
    }
    pub fn clear_overlay(&self) {
        self.set_overlay(TileOverlay::None);
    }
    /// Push an overlay only if the tile is currently idle (None) or
    /// already showing a Graph. Used by the orchestrator to auto-update
    /// the whiteboard without stomping on a Card / File / Quote the
    /// model deliberately put up.
    pub fn set_overlay_if_idle_or_graph(&self, overlay: TileOverlay) -> bool {
        if let Ok(mut g) = self.overlay.lock() {
            let replaceable = matches!(
                *g,
                TileOverlay::None | TileOverlay::Graph { .. }
            );
            if replaceable {
                *g = overlay;
                return true;
            }
        }
        false
    }
}

pub struct ParticleVideoSource {
    latest: Arc<std::sync::Mutex<Option<VideoFrame>>>,
    running: Arc<AtomicBool>,
}

impl ParticleVideoSource {
    /// Spawn a fresh render thread and return a `VideoSource` that
    /// publishes its frames. The thread exits when `running` flips false
    /// (via `stop()` or Drop).
    pub fn spawn(character_name: String, control: ParticleControl) -> Self {
        let latest = Arc::new(std::sync::Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let latest_t = latest.clone();
        let running_t = running.clone();
        std::thread::spawn(move || {
            render_loop(character_name, control, latest_t, running_t);
        });
        Self { latest, running }
    }
}

impl Drop for ParticleVideoSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl VideoSource for ParticleVideoSource {
    fn name(&self) -> &str {
        "claude"
    }
    fn format(&self) -> VideoFormat {
        VideoFormat {
            pixel_format: PixelFormat::Rgba,
            dimensions: [VIDEO_W, VIDEO_H],
        }
    }
    fn pop_frame(&mut self) -> anyhow::Result<Option<VideoFrame>> {
        Ok(self.latest.lock().expect("video frame lock").take())
    }
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> anyhow::Result<()> {
        self.running.store(false, Ordering::Relaxed);
        Ok(())
    }
}

fn render_loop(
    character_name: String,
    control: ParticleControl,
    latest: Arc<std::sync::Mutex<Option<VideoFrame>>>,
    running: Arc<AtomicBool>,
) {
    let base = match gho_characters::by_name(&character_name) {
        Some(c) => c,
        None => {
            tracing::warn!(
                character = %character_name,
                "ghostly character unknown; falling back to 'eliza'"
            );
            gho_characters::by_name("eliza").expect("eliza always exists")
        }
    };
    tracing::info!(character = %base.name, particles = PARTICLES, "claude face renderer started");

    let settings = RenderSettings {
        width: VIDEO_W,
        height: VIDEO_H,
        ..RenderSettings::default()
    };
    let Some(mut renderer) = Renderer::new(settings) else {
        tracing::error!("face renderer: failed to allocate {VIDEO_W}x{VIDEO_H} pixmap");
        return;
    };

    const SCALE: f32 = 6.5;
    let mut state = FaceState::new(&base, PARTICLES, SCALE, 42);
    let mut current: GhoCharacter = apply_emotion(&base, Emotion::Calm, EMOTION_INTENSITY);
    let mut last_emotion = Emotion::Calm;

    // Scratch pixmap reused every frame for overlay rasterization.
    let mut overlay_scratch = match Pixmap::new(VIDEO_W, VIDEO_H) {
        Some(p) => p,
        None => {
            tracing::error!("face renderer: failed to allocate overlay scratch");
            return;
        }
    };
    let mut usvg_opt = usvg::Options::default();
    usvg_opt.fontdb_mut().load_system_fonts();

    let frame_dt = Duration::from_millis(1000 / FPS);
    let started = Instant::now();
    while running.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let t = started.elapsed().as_secs_f32();

        let peer = f32::from_bits(control.peer_level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let self_level =
            f32::from_bits(control.self_level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let thinking = control.thinking.load(Ordering::Relaxed);

        let emotion = if self_level > 0.03 {
            Emotion::Passion
        } else if thinking {
            Emotion::Curiosity
        } else if peer > 0.03 {
            Emotion::Curiosity
        } else {
            Emotion::Calm
        };
        if emotion != last_emotion {
            current = apply_emotion(&base, emotion, EMOTION_INTENSITY);
            last_emotion = emotion;
        }

        state.set_audio_level(self_level.max(peer));
        state.set_listening_level(peer);
        state.set_working(thinking);

        let dt_secs = frame_dt.as_secs_f32();
        state.step_gaze(t, dt_secs);
        state.step_blink(t, dt_secs);
        state.step_eye_saccade(t, dt_secs);
        state.step_audio_onset(t, dt_secs);
        state.set_brow(match emotion {
            Emotion::Curiosity => 0.6,
            Emotion::Awe => 0.85,
            Emotion::Passion => -0.35,
            Emotion::Concern => -0.5,
            Emotion::Triumph => 0.7,
            Emotion::Joy | Emotion::Warmth => 0.3,
            Emotion::Calm => 0.0,
        });
        if let Some(cfg) = current.render_config.embers {
            state.step_embers(&cfg, dt_secs, SCALE);
        }

        let face_pixmap = renderer.render(&current, &state, t);
        // Compose face + overlay onto a fresh buffer (renderer.render
        // returns a borrowed pixmap so we can't mutate it directly).
        let mut composed = face_pixmap.clone();
        let overlay_snapshot = control
            .overlay
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or(TileOverlay::None);
        if !matches!(overlay_snapshot, TileOverlay::None) {
            composite_overlay(
                &overlay_snapshot,
                &mut composed,
                &usvg_opt,
                &mut overlay_scratch,
            );
        }
        let data = Bytes::copy_from_slice(composed.data());
        let frame = VideoFrame::new_rgba(data, VIDEO_W, VIDEO_H, Duration::ZERO);
        if let Ok(mut g) = latest.lock() {
            *g = Some(frame);
        }

        if let Some(rest) = frame_dt.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    tracing::info!("claude face renderer stopped");
}
