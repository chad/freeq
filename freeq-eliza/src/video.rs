//! Eliza's video tile.
//!
//! The tile is a live, animated surface — never a black square. When
//! eliza has nothing to show it renders a **state-aware presence** (an
//! orb that visibly idles, listens, thinks, or speaks). When it answers
//! a question it renders a **designed scene card**: the model picks a
//! layout — hero, key points, a big stat, a quote, or a timeline — and
//! the renderer draws it with typographic hierarchy, depth and motion.
//!
//! Everything is drawn as SVG, re-rendered every frame (so it genuinely
//! animates), rasterized with resvg, and fed to the H.264 encoder. The
//! tile is a plain video stream, so every client just plays it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use iroh_live::media::format::{PixelFormat, VideoFormat, VideoFrame};
use iroh_live::media::traits::VideoSource;

use crate::whiteboard::Step;

/// Tile resolution. 720p — chunkier than the old 360p but with the
/// new bloom + ember count it reads enormously better, and CPU
/// rasterizing at 15 fps still leaves headroom.
pub const VIDEO_W: u32 = 1280;
pub const VIDEO_H: u32 = 720;
const FPS: u64 = 15;
/// Most points a scene shows (extras are dropped).
const MAX_POINTS: usize = 6;
/// How long a scene stays on the tile after it appears before the tile
/// returns to the presence orb. Short — the overlay primitives compose
/// on top so she still reads as "live" while a card is up, so cards
/// don't need to *takeover* the tile to do their job.
const SCENE_HOLD: Duration = Duration::from_secs(12);
/// Extra time a scene stays up once its backdrop image arrives — image
/// generation is slow, so a late image still gets airtime.
const IMAGE_HOLD: Duration = Duration::from_secs(8);

/// How long a whiteboard stays on the tile. A bit longer than scenes —
/// diagrams reward dwell time. Each step reveals over [`BOARD_STEP_MS`].
const BOARD_HOLD: Duration = Duration::from_secs(20);
/// Time between successive whiteboard step reveals.
const BOARD_STEP_MS: u32 = 900;
/// How long each step's fade-in animation runs.
const BOARD_REVEAL_MS: u32 = 320;
/// How long an ambient "topic" chip stays on the tile before reverting
/// to the default HUD readout. Long enough that one quiet tick doesn't
/// snap it away (the ambient loop ticks every ~8s); short enough that a
/// stale chip can't outlast the conversation it described.
const AMBIENT_HOLD: Duration = Duration::from_secs(25);
/// Accent used when the model gives no (or a malformed) colour.
const DEFAULT_ACCENT: &str = "#6cb0ff";
/// Font stack for all tile text.
const FONT: &str = "Helvetica, Arial, sans-serif";

/// Which layout a scene card uses. Chosen by the model per answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SceneKind {
    /// One big idea: headline + takeaway.
    Hero,
    /// Several distinct points as a numbered list.
    KeyPoints,
    /// A single number carries the answer.
    Stat,
    /// A striking statement or definition.
    Quote,
    /// An ordered sequence or process.
    Timeline,
}

impl SceneKind {
    /// Parse the model's `kind` string. Unknown values fall back to
    /// [`SceneKind::KeyPoints`] — the most general layout.
    pub fn from_tag(s: &str) -> SceneKind {
        match s.trim().to_lowercase().as_str() {
            "hero" => SceneKind::Hero,
            "stat" => SceneKind::Stat,
            "quote" => SceneKind::Quote,
            "timeline" => SceneKind::Timeline,
            _ => SceneKind::KeyPoints,
        }
    }
}

/// A scene card description — what the model produces for an answer and
/// what the renderer turns into a frame. Field meaning depends on
/// [`SceneKind`]; see the scene-generation prompt in `qa.rs`.
#[derive(Clone, Debug)]
pub struct SceneSpec {
    pub kind: SceneKind,
    pub title: String,
    pub subtitle: String,
    pub points: Vec<String>,
    /// Accent colour as `#RRGGBB` (validated by [`VideoTile::show_scene`]).
    pub accent: String,
    /// A short, concrete subject to illustrate the scene (e.g. "Apollo
    /// 11 Moon landing") — used as a Wikipedia image search and, on
    /// fallback, as the AI image-generation subject.
    pub image_query: String,
}

/// What eliza is doing — read off the audio, a "thinking" flag, and a
/// "vision thumb" set when she's analyzing a participant's frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mood {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Vision,
}

/// How many EQ-strip / history samples to keep.
const EQ_BARS: usize = 32;

/// A whiteboard currently on the tile. Steps reveal in order, one
/// every [`BOARD_STEP_MS`], with a [`BOARD_REVEAL_MS`] fade-in each.
struct Board {
    steps: Vec<Step>,
    shown_at: Instant,
    /// Accent colour for the board's strokes and labels.
    accent: String,
}

impl Board {
    fn is_visible(&self) -> bool {
        self.shown_at.elapsed() < BOARD_HOLD
    }
}

/// Ambient "manifesting" state — a short concept + accent colour the
/// ambient monitor refreshes every ~20s while she's listening. It does
/// NOT take the tile over — the HUD chip and a faint scrim blend pick
/// it up while mood, scene, and board continue to drive the main visual.
struct Ambient {
    concept: String,
    accent: String,
    set_at: Instant,
}

impl Ambient {
    fn is_visible(&self) -> bool {
        self.set_at.elapsed() < AMBIENT_HOLD
    }
}

/// Ambient image — a subtle, **text-less** backdrop that surfaces when
/// the ambient monitor recognises a concrete subject in the
/// conversation. Distinct from `Scene` (which has a title + body +
/// points for answer-driven informational cards). Ambient images are
/// pure visual cues: faded, no overlay text — the topic name lives in
/// the [`Ambient`] HUD chip instead.
struct AmbientImage {
    image: Option<(String, Instant)>,
    set_at: Instant,
}

/// How long an ambient image sticks around once its picture has
/// arrived. A little longer than the scene hold so a slow image-gen
/// still gets airtime.
const AMBIENT_IMAGE_HOLD: Duration = Duration::from_secs(25);

impl AmbientImage {
    fn is_visible(&self) -> bool {
        // Stays visible from the moment of `show_ambient_image` until
        // either the image arrives + IMAGE_HOLD, or it never arrives
        // and the placeholder slot times out.
        match &self.image {
            Some((_, at)) => at.elapsed() < AMBIENT_IMAGE_HOLD,
            None => self.set_at.elapsed() < Duration::from_secs(60),
        }
    }
}

/// A scene currently on the tile, plus when it appeared (drives the
/// reveal animation and the hold-then-revert-to-orb timer).
struct Scene {
    spec: SceneSpec,
    shown_at: Instant,
    /// Monotonic id so a late-arriving backdrop image attaches to the
    /// scene it was generated for, not a newer one.
    id: u64,
    /// Generated backdrop: the JPEG `data:` URI and when it arrived
    /// (drives the image fade-in and extends the scene hold).
    image: Option<(String, Instant)>,
}

impl Scene {
    /// Whether the scene should still be on the tile (vs the orb). A
    /// scene holds for [`SCENE_HOLD`]; once a backdrop image arrives it
    /// holds [`IMAGE_HOLD`] past that arrival so the image is seen.
    fn is_visible(&self) -> bool {
        self.shown_at.elapsed() < SCENE_HOLD
            || self
                .image
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() < IMAGE_HOLD)
    }
}

/// Which renderer powers the tile.
///
/// - `Svg` (default) is the full freeq cyberpunk presence — corner
///   brackets, EQ strip, state sticker, HUD chip, scene cards,
///   whiteboards, vision PiP, ambient topic. Owns every overlay the
///   rest of `freeq-eliza` orchestrates.
/// - `Particles { character }` is the ghostly particle-face renderer —
///   a 12K-particle procedural face from `~/src/ghostly`. Scene cards,
///   whiteboards, and the ambient HUD are NO-OPS on this path (the
///   particle render is a single-layer face, not a UI). Mood + audio
///   level still drive palette and breath.
#[derive(Clone, Debug)]
pub enum Backend {
    Svg,
    Particles { character: String },
}

impl Default for Backend {
    fn default() -> Self {
        Backend::Svg
    }
}

/// Shared handle to eliza's video tile. Clone-cheap.
#[derive(Clone)]
pub struct VideoTile {
    pub(crate) latest: Arc<Mutex<Option<VideoFrame>>>,
    /// eliza's own speech loudness, `f32` bits in `[0,1]`.
    pub(crate) level: Arc<AtomicU32>,
    /// Loudest participant's loudness — drives the "listening" mood.
    pub(crate) peer_level: Arc<AtomicU32>,
    /// Set while an LLM call is in flight — drives the "thinking" mood.
    pub(crate) thinking: Arc<AtomicBool>,
    /// Monotonic SystemTime epoch (millis) of the last `flash_hand_raise`
    /// call. The particles render loop checks `now - hand_raise_at < N`
    /// and brightens the status halo + tilts the head while that window
    /// is open. The flash decays naturally without needing a timer task.
    pub(crate) hand_raise_at: Arc<AtomicU64>,
    /// `data:image/jpeg;base64,…` of the frame currently being analyzed
    /// by the vision model. While set, the tile shows a PiP of it and
    /// flips into [`Mood::Vision`].
    pub(crate) vision_thumb: Arc<Mutex<Option<String>>>,
    scene: Arc<Mutex<Option<Scene>>>,
    /// Whiteboard takes priority over the scene card when both are set.
    board: Arc<Mutex<Option<Board>>>,
    /// Ambient topic + accent, refreshed by the ambient monitor. Drives
    /// the HUD chip and a subtle scrim blend — never the main visual.
    ambient: Arc<Mutex<Option<Ambient>>>,
    /// Ambient *image* — image-only, no title. Picks up the topic
    /// visual from the ambient monitor's image-gen path without
    /// putting the topic name on screen as a banner.
    pub(crate) ambient_image: Arc<Mutex<Option<AmbientImage>>>,
    /// Hands out a fresh id per scene so async image jobs can target one.
    next_id: Arc<AtomicU64>,
    pub(crate) running: Arc<AtomicBool>,
    /// Sticky gaze target — the nick the bot is currently addressing
    /// or being addressed by. The particles render loop reads this
    /// and calls `FaceState::set_gaze_lock`, so the bot's eyes turn
    /// toward whoever the conversation is focused on. Cleared once
    /// the exchange ends, idle gaze resumes.
    pub(crate) focus_nick: Arc<Mutex<Option<String>>>,
    /// Which renderer to spawn. Cloned into the render thread.
    backend: Backend,
}

impl VideoTile {
    pub fn new() -> Self {
        Self::with_backend(Backend::Svg)
    }

    /// Build a tile with an explicit renderer choice. CLI plumbs this
    /// from `--render-backend` + `--ghostly-character`.
    pub fn with_backend(backend: Backend) -> Self {
        Self {
            latest: Arc::new(Mutex::new(None)),
            level: Arc::new(AtomicU32::new(0)),
            peer_level: Arc::new(AtomicU32::new(0)),
            thinking: Arc::new(AtomicBool::new(false)),
            hand_raise_at: Arc::new(AtomicU64::new(0)),
            vision_thumb: Arc::new(Mutex::new(None)),
            scene: Arc::new(Mutex::new(None)),
            board: Arc::new(Mutex::new(None)),
            ambient: Arc::new(Mutex::new(None)),
            ambient_image: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(0)),
            running: Arc::new(AtomicBool::new(true)),
            focus_nick: Arc::new(Mutex::new(None)),
            backend,
        }
    }

    /// Lock the rendered face's gaze on `nick`. Call with `Some(asker)`
    /// at the start of an answer; clear with `None` when done. The
    /// particles render loop picks this up next frame.
    pub fn set_focus_nick(&self, nick: Option<String>) {
        if let Ok(mut g) = self.focus_nick.lock() {
            *g = nick;
        }
    }

    /// Show a whiteboard diagram on the tile. Replaces any scene or
    /// previous board. Steps reveal one at a time at
    /// [`BOARD_STEP_MS`] intervals, each with a fade-in over
    /// [`BOARD_REVEAL_MS`].
    pub fn show_board(&self, steps: Vec<Step>, accent: String) {
        let accent = validate_accent(&accent);
        // Clear any scene — board takes the tile.
        *self.scene.lock().expect("scene lock") = None;
        *self.board.lock().expect("board lock") = Some(Board {
            steps,
            shown_at: Instant::now(),
            accent,
        });
    }

    /// Show a PiP of the frame eliza is about to send to the vision
    /// model — kept on screen until [`clear_vision_thumb`] (typically at
    /// the end of `answer_and_speak`), so the tile reads "I'm describing
    /// THIS" while she's still talking about it.
    pub fn set_vision_thumb(&self, data_uri: String) {
        if let Ok(mut g) = self.vision_thumb.lock() {
            *g = Some(data_uri);
        }
    }

    /// Drop the vision PiP. Safe to call when none was set.
    pub fn clear_vision_thumb(&self) {
        if let Ok(mut g) = self.vision_thumb.lock() {
            *g = None;
        }
    }

    /// Apply an ambient topic — the HUD chip swaps from its default
    /// "MOQ ▸ LIVE" readout to the concept text in the supplied accent,
    /// and a faint accent rect blends into the tile's background. Used
    /// by the ambient monitor while she's listening so the tile reflects
    /// what's being discussed in real time. Accent is validated; a bad
    /// one falls back to [`DEFAULT_ACCENT`].
    pub fn set_ambient(&self, concept: String, accent: String) {
        let accent = validate_accent(&accent);
        let concept: String = concept.chars().take(28).collect();
        if let Ok(mut g) = self.ambient.lock() {
            *g = Some(Ambient {
                concept,
                accent,
                set_at: Instant::now(),
            });
        }
    }

    /// Drop the ambient topic. Safe to call when none was set. The HUD
    /// reverts to its default readout on the next frame.
    pub fn clear_ambient(&self) {
        if let Ok(mut g) = self.ambient.lock() {
            *g = None;
        }
    }

    /// Reserve an ambient image slot — the renderer starts holding
    /// space for an upcoming backdrop. The actual image arrives
    /// asynchronously via [`set_ambient_image`]. Unlike [`show_scene`]
    /// there's no title / points / body — ambient images are pure
    /// visual cues with the topic name living on the HUD chip.
    pub fn show_ambient_image(&self) {
        if let Ok(mut g) = self.ambient_image.lock() {
            *g = Some(AmbientImage {
                image: None,
                set_at: Instant::now(),
            });
        }
    }

    /// Attach the fetched image (a JPEG `data:` URI) to the current
    /// ambient slot. Ignored if there's no slot or the slot has aged
    /// out — late images for a stale ambient pick get dropped.
    pub fn set_ambient_image(&self, data_uri: String) {
        if let Ok(mut g) = self.ambient_image.lock() {
            if let Some(ai) = g.as_mut() {
                ai.image = Some((data_uri, Instant::now()));
            }
        }
    }

    /// The [`VideoSource`] to hand to `broadcast.video().set_source(..)`.
    pub fn source(&self) -> PushVideoSource {
        PushVideoSource {
            latest: self.latest.clone(),
        }
    }

    /// Loudness cell for eliza's own voice (the audio path writes it).
    pub fn level_handle(&self) -> Arc<AtomicU32> {
        self.level.clone()
    }

    /// Loudness cell for incoming participant audio (a tap writes it).
    pub fn peer_level_handle(&self) -> Arc<AtomicU32> {
        self.peer_level.clone()
    }

    /// Mark whether an LLM call is in flight (drives the thinking mood).
    /// Fire a hand-raise pulse — the renderer brightens the halo and
    /// gently tilts the head for ~3 s. Call when the bot has been name-
    /// dropped but not directly addressed; visual-only ("I have
    /// something to add") without breaking the strict address policy.
    pub fn flash_hand_raise(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.hand_raise_at
            .store(now_ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// Snapshot of the time since the most recent hand-raise (seconds).
    /// `None` if no flash has ever been requested. Used by the
    /// particles renderer to decay the visual.
    pub fn hand_raise_seconds_ago(&self) -> Option<f32> {
        let stamp = self.hand_raise_at.load(std::sync::atomic::Ordering::Relaxed);
        if stamp == 0 {
            return None;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let elapsed_ms = now_ms.saturating_sub(stamp);
        Some(elapsed_ms as f32 / 1000.0)
    }

    pub fn set_thinking(&self, on: bool) {
        self.thinking.store(on, Ordering::Relaxed);
    }

    /// Put a new scene on the tile. The accent is validated and the
    /// point list capped here so the renderer can trust the spec.
    /// Returns the scene's id — pass it to [`VideoTile::set_scene_image`]
    /// to attach a backdrop once one has been generated.
    pub fn show_scene(&self, mut spec: SceneSpec) -> u64 {
        spec.accent = validate_accent(&spec.accent);
        spec.points.truncate(MAX_POINTS);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Clear any board — scene takes the tile.
        *self.board.lock().expect("board lock") = None;
        *self.scene.lock().expect("scene lock") = Some(Scene {
            spec,
            shown_at: Instant::now(),
            id,
            image: None,
        });
        id
    }

    /// Attach a generated backdrop image (a JPEG `data:` URI) to scene
    /// `id`. Ignored if the current scene has since been replaced by a
    /// newer answer.
    pub fn set_scene_image(&self, id: u64, data_uri: String) {
        let mut guard = self.scene.lock().expect("scene lock");
        if let Some(scene) = guard.as_mut() {
            if scene.id == id {
                scene.image = Some((data_uri, Instant::now()));
            }
        }
    }

    /// Stop the render loop. Call on call-end.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    /// Spawn the render loop on a dedicated thread.
    pub fn spawn_renderer(&self) {
        let tile = self.clone();
        let backend = tile.backend.clone();
        std::thread::Builder::new()
            .name("eliza-video".into())
            .spawn(move || match backend {
                Backend::Svg => tile.render_loop(),
                Backend::Particles { character } => {
                    crate::video_particles::render_loop(tile, &character)
                }
            })
            .expect("spawn video renderer");
    }

    fn render_loop(self) {
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        let mut pixmap = match resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H) {
            Some(p) => p,
            None => {
                tracing::error!("video: could not allocate pixmap");
                return;
            }
        };
        let frame_dt = Duration::from_millis(1000 / FPS);
        let started = Instant::now();
        tracing::info!("eliza video renderer started ({VIDEO_W}x{VIDEO_H} @ {FPS}fps)");

        // Per-frame state tracked across iterations: a rolling history
        // for the EQ strip, and the last mood + when it changed for the
        // glitch transition (~250ms scanline burst on state change).
        let mut level_history: VecDeque<f32> = VecDeque::with_capacity(EQ_BARS);
        let mut peer_history: VecDeque<f32> = VecDeque::with_capacity(EQ_BARS);
        let mut last_mood: Option<Mood> = None;
        let mut transition_at: Option<Instant> = None;

        while self.running.load(Ordering::Relaxed) {
            let tick = Instant::now();
            let t = started.elapsed().as_secs_f32();
            let level = f32::from_bits(self.level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            let peer = f32::from_bits(self.peer_level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            let thinking = self.thinking.load(Ordering::Relaxed);
            let vision_thumb = self
                .vision_thumb
                .lock()
                .ok()
                .and_then(|g| g.clone());
            let ambient = self
                .ambient
                .lock()
                .ok()
                .and_then(|g| {
                    g.as_ref()
                        .filter(|a| a.is_visible())
                        .map(|a| (a.concept.clone(), a.accent.clone()))
                });

            // Vision overrides everything — when she's analyzing a frame,
            // that's the most important thing for the viewer to see, even
            // while she's speaking the answer.
            let mood = if vision_thumb.is_some() {
                Mood::Vision
            } else if level > 0.03 {
                Mood::Speaking
            } else if thinking {
                Mood::Thinking
            } else if peer > 0.03 {
                Mood::Listening
            } else {
                Mood::Idle
            };

            if Some(mood) != last_mood {
                transition_at = Some(Instant::now());
                last_mood = Some(mood);
            }
            // Glitch intensity decays from 1.0 to 0.0 over 600ms after
            // every mood change. Loud, but only on transitions — never
            // ambient (continuous glitch would be exhausting to watch).
            let glitch = transition_at
                .map(|when| (1.0 - when.elapsed().as_secs_f32() / 0.6).clamp(0.0, 1.0))
                .unwrap_or(0.0);

            level_history.push_back(level);
            if level_history.len() > EQ_BARS {
                level_history.pop_front();
            }
            peer_history.push_back(peer);
            if peer_history.len() > EQ_BARS {
                peer_history.pop_front();
            }
            let lh: Vec<f32> = level_history.iter().copied().collect();
            let ph: Vec<f32> = peer_history.iter().copied().collect();

            let state = PresenceState {
                mood,
                t,
                level,
                peer,
                level_history: &lh,
                peer_history: &ph,
                glitch,
                vision_thumb: vision_thumb.as_deref(),
                ambient: ambient
                    .as_ref()
                    .map(|(c, a)| (c.as_str(), a.as_str())),
            };

            let svg = {
                let bguard = self.board.lock().expect("board lock");
                if let Some(board) = bguard.as_ref().filter(|b| b.is_visible()) {
                    board_svg(board, &state)
                } else {
                    drop(bguard);
                    let guard = self.scene.lock().expect("scene lock");
                    match guard.as_ref() {
                        // Show the scene while it's within its hold
                        // window; then the tile returns to presence.
                        Some(scene) if scene.is_visible() => scene_svg(scene, &state),
                        _ => presence_svg(&state),
                    }
                }
            };

            if let Some(frame) = rasterize(&svg, &opt, &mut pixmap) {
                *self.latest.lock().expect("video frame lock") = Some(frame);
            }

            if let Some(rest) = frame_dt.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        tracing::info!("eliza video renderer stopped");
    }
}

impl Default for VideoTile {
    fn default() -> Self {
        Self::new()
    }
}

/// The [`VideoSource`] the H.264 encoder pulls — the most recent
/// rendered frame, `take`n so each frame is encoded at most once.
pub struct PushVideoSource {
    latest: Arc<Mutex<Option<VideoFrame>>>,
}

impl VideoSource for PushVideoSource {
    fn name(&self) -> &str {
        "eliza"
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
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------

/// Cubic ease-out — fast in, gentle settle. Clamps to `[0,1]`.
fn ease_out(p: f32) -> f32 {
    let q = 1.0 - p.clamp(0.0, 1.0);
    1.0 - q * q * q
}

/// Eased reveal progress for an element that starts `delay` seconds into
/// a scene and animates over `dur` seconds.
fn reveal(elapsed: f32, delay: f32, dur: f32) -> f32 {
    ease_out((elapsed - delay) / dur)
}

/// Rasterize an SVG document to an opaque RGBA [`VideoFrame`]. Returns
/// `None` if the SVG fails to parse — a bad scene must not kill the tile.
fn rasterize(
    svg: &str,
    opt: &resvg::usvg::Options,
    pixmap: &mut resvg::tiny_skia::Pixmap,
) -> Option<VideoFrame> {
    let tree = match resvg::usvg::Tree::from_str(svg, opt) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "video: SVG parse failed");
            return None;
        }
    };
    pixmap.fill(resvg::tiny_skia::Color::BLACK);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let data = bytes::Bytes::copy_from_slice(pixmap.data());
    Some(VideoFrame::new_rgba(data, VIDEO_W, VIDEO_H, Duration::ZERO))
}

/// Per-mood accent — pop-punk loud: acid yellow when she's speaking,
/// hot pink when she's seeing, electric mint when she's listening,
/// neon purple when she's thinking.
fn mood_color(mood: Mood) -> &'static str {
    match mood {
        Mood::Idle => "#6cb0ff",
        Mood::Listening => "#3effd6",
        Mood::Thinking => "#c69cff",
        Mood::Speaking => "#ffea3e",
        Mood::Vision => "#ff3ec8",
    }
}

/// Short sticker label for each mood. SHOUTING because pop-punk.
fn mood_label(mood: Mood) -> &'static str {
    match mood {
        Mood::Idle => "STANDBY",
        Mood::Listening => "LISTENING",
        Mood::Thinking => "PROCESSING",
        Mood::Speaking => "ELIZA",
        Mood::Vision => "VISION",
    }
}

/// Validate a model-supplied accent. Accepts `#RRGGBB` only; anything
/// else (a name, bad length, non-hex) falls back to [`DEFAULT_ACCENT`].
fn validate_accent(s: &str) -> String {
    let t = s.trim();
    let ok = t.len() == 7
        && t.starts_with('#')
        && t[1..].chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        t.to_string()
    } else {
        DEFAULT_ACCENT.to_string()
    }
}

/// Greedy word-wrap to at most `max_chars` per line. A single word
/// longer than the limit overflows its line rather than being split.
fn wrap(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= max_chars {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Truncate to `max` characters with an ellipsis — keeps model text
/// from overrunning a fixed-width panel.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Escape the five XML metacharacters so model-authored text can't
/// break the SVG document.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render a block of pre-wrapped lines as staggered, revealing `<text>`
/// elements — used for headlines, subtitles, quotes and context.
#[allow(clippy::too_many_arguments)]
fn lines_svg(
    lines: &[String],
    x: f32,
    y0: f32,
    line_h: f32,
    size: f32,
    weight: u32,
    fill: &str,
    anchor: &str,
    elapsed: f32,
    delay0: f32,
    stagger: f32,
) -> String {
    let mut s = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let p = reveal(elapsed, delay0 + i as f32 * stagger, 0.5);
        if p <= 0.001 {
            continue;
        }
        let y = y0 + i as f32 * line_h;
        s.push_str(&format!(
            r##"<text x="{x:.1}" y="{y:.1}" font-family="{FONT}" font-size="{size:.0}" font-weight="{weight}" fill="{fill}" text-anchor="{anchor}" opacity="{p:.3}" transform="translate(0 {dy:.1})">{txt}</text>
"##,
            dy = (1.0 - p) * 14.0,
            txt = xml_escape(line),
        ));
    }
    s
}

// ---------------------------------------------------------------------
// Presence orb
// ---------------------------------------------------------------------

/// All the per-frame inputs the presence renderer needs. Passed by ref
/// so the audio histories (for the EQ strip) don't have to be cloned.
struct PresenceState<'a> {
    mood: Mood,
    t: f32,
    /// Eliza's own speech loudness (drives the lip-synced mouth and the
    /// EQ strip while she's speaking).
    level: f32,
    /// Loudest peer right now (drives the EQ strip + brackets the rest
    /// of the time).
    peer: f32,
    /// Rolling history of `level` — `EQ_BARS` samples at ~15Hz.
    level_history: &'a [f32],
    /// Rolling history of `peer`.
    peer_history: &'a [f32],
    /// `[0,1]` glitch intensity — non-zero only briefly after a mood
    /// change. Decays inside the renderer so the burst only flashes,
    /// never lingers.
    glitch: f32,
    /// `data:image/jpeg;base64,…` of the frame currently being analyzed,
    /// when she's in [`Mood::Vision`]. Renders as a PiP in the corner.
    vision_thumb: Option<&'a str>,
    /// Ambient `(concept, accent)` from the ambient monitor — the topic
    /// she's silently picking up on while listening. Drives the HUD chip
    /// and a faint scrim blend; does NOT replace the mood.
    ambient: Option<(&'a str, &'a str)>,
}

/// The state-aware presence — pop-punk-cyberpunk: corner brackets that
/// pulse with audio, a sticker chip naming the current state, a bottom
/// EQ strip, a halftone field behind the orb, an RGB-split scanline
/// burst on every state change, and a vision PiP when she's looking at
/// something. The orb in the centre still breathes and lip-syncs.
fn presence_svg(s: &PresenceState) -> String {
    let accent = mood_color(s.mood);
    let dot_r = match s.mood {
        Mood::Idle => 0.9,
        Mood::Listening => 1.2,
        Mood::Thinking | Mood::Vision => 2.2,
        Mood::Speaking => 1.7,
    };

    let breathe = (s.t * 1.6).sin() * 5.0;
    let orb_r = match s.mood {
        Mood::Speaking => 48.0 + breathe + s.level * 64.0,
        Mood::Thinking => 44.0 + (s.t * 4.0).sin() * 4.0,
        Mood::Vision => 46.0 + (s.t * 5.0).sin() * 3.5,
        Mood::Listening => 46.0 + breathe + s.peer * 30.0,
        Mood::Idle => 44.0 + breathe,
    };
    let glow_r = orb_r * 1.95;
    let glow_op = match s.mood {
        Mood::Speaking => 0.14 + s.level * 0.4,
        Mood::Thinking => 0.18 + (s.t * 4.0).sin().abs() * 0.12,
        Mood::Vision => 0.22 + (s.t * 5.0).sin().abs() * 0.15,
        Mood::Listening => 0.16 + s.peer * 0.3,
        Mood::Idle => 0.12,
    };

    // Mood-specific overlay around the orb — distinct gestures for each.
    let overlay = match s.mood {
        Mood::Thinking => format!(
            r##"<circle cx="320" cy="156" r="{r:.1}" fill="none" stroke="{accent}" stroke-width="3" stroke-dasharray="14 12" opacity="0.85" transform="rotate({deg:.1} 320 156)"/>"##,
            r = orb_r + 26.0,
            deg = s.t * 150.0,
        ),
        Mood::Vision => {
            // "Looking" — concentric scanned ring + a rotating tick ring.
            let r1 = orb_r + 18.0;
            let r2 = orb_r + 34.0;
            format!(
                r##"<circle cx="320" cy="156" r="{r1:.1}" fill="none" stroke="{accent}" stroke-width="1.6" stroke-dasharray="8 4" opacity="0.7"/>
<circle cx="320" cy="156" r="{r2:.1}" fill="none" stroke="{accent}" stroke-width="1" stroke-dasharray="2 6" opacity="0.55" transform="rotate({deg:.1} 320 156)"/>"##,
                deg = -s.t * 80.0,
            )
        }
        Mood::Listening => {
            let mut rings = String::new();
            for i in 0..3 {
                let phase = (s.t * 0.6 + i as f32 * 0.33).fract();
                let rr = orb_r + 8.0 + phase * 64.0;
                let op = (1.0 - phase) * 0.6;
                rings.push_str(&format!(
                    r##"<circle cx="320" cy="156" r="{rr:.1}" fill="none" stroke="{accent}" stroke-width="2" opacity="{op:.3}"/>"##,
                ));
            }
            rings
        }
        _ => format!(
            r##"<circle cx="320" cy="156" r="{r:.1}" fill="none" stroke="{accent}" stroke-width="1.5" opacity="0.3"/>"##,
            r = orb_r + 22.0 + (s.t * 2.0).sin() * 3.0,
        ),
    };

    // Face — blinking eyes, a mouth whose openness tracks `level`. When
    // she's loud the mouth gets an RGB-split chromatic ghost — punk
    // distortion, only visible when she's actually speaking.
    let blinking = (s.t % 4.3) < 0.13;
    let eye_r = orb_r * 0.115;
    let eye_ry = eye_r * if blinking { 0.12 } else { 1.0 };
    let eye_y = 156.0 - orb_r * 0.20;
    let eye_dx = orb_r * 0.34;
    let mouth_cy = 156.0 + orb_r * 0.36;
    let mouth_rx = orb_r * 0.27;
    let mouth_ry = 2.0 + s.level.clamp(0.0, 1.0) * orb_r * 0.42;
    let chrom = (s.level * 6.0).clamp(0.0, 2.0); // chromatic offset in px
    let face = if chrom > 0.2 {
        format!(
            r##"<g fill="#0a1020">
<ellipse cx="{lx:.1}" cy="{eye_y:.1}" rx="{eye_r:.1}" ry="{eye_ry:.1}"/>
<ellipse cx="{rx:.1}" cy="{eye_y:.1}" rx="{eye_r:.1}" ry="{eye_ry:.1}"/>
</g>
<ellipse cx="{mxR:.2}" cy="{mouth_cy:.1}" rx="{mouth_rx:.1}" ry="{mouth_ry:.1}" fill="#ff3366" opacity="0.7"/>
<ellipse cx="{mxB:.2}" cy="{mouth_cy:.1}" rx="{mouth_rx:.1}" ry="{mouth_ry:.1}" fill="#33ffee" opacity="0.7"/>
<ellipse cx="320" cy="{mouth_cy:.1}" rx="{mouth_rx:.1}" ry="{mouth_ry:.1}" fill="#0a1020"/>"##,
            lx = 320.0 - eye_dx,
            rx = 320.0 + eye_dx,
            mxR = 320.0 - chrom,
            mxB = 320.0 + chrom,
        )
    } else {
        format!(
            r##"<g fill="#0a1020">
<ellipse cx="{lx:.1}" cy="{eye_y:.1}" rx="{eye_r:.1}" ry="{eye_ry:.1}"/>
<ellipse cx="{rx:.1}" cy="{eye_y:.1}" rx="{eye_r:.1}" ry="{eye_ry:.1}"/>
<ellipse cx="320" cy="{mouth_cy:.1}" rx="{mouth_rx:.1}" ry="{mouth_ry:.1}"/>
</g>"##,
            lx = 320.0 - eye_dx,
            rx = 320.0 + eye_dx,
        )
    };

    let scrim = mood_scrim(accent, s);
    let overlay_primitives = presence_overlay(s);

    // viewBox stays at the design coord space (640×360) — every inner
    // coordinate in this file is authored against that grid. The
    // output `width`/`height` scale to the actual pixmap size.
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 640 360" preserveAspectRatio="xMidYMid slice">
{defs}
<rect width="640" height="360" fill="url(#bg)"/>
<rect width="640" height="360" fill="url(#halftone)" opacity="0.55"/>
{scrim}
<circle cx="320" cy="156" r="{glow_r:.1}" fill="{accent}" opacity="{glow_op:.3}"/>
{overlay}
<circle cx="320" cy="156" r="{orb_r:.1}" fill="url(#orb)"/>
{face}
{overlay_primitives}
</svg>"##,
        w = VIDEO_W,
        h = VIDEO_H,
        defs = presence_defs(accent, dot_r),
    )
}

/// The mood-reactive framing primitives that sit above any base content.
/// Composed on top of the presence orb AND on top of scene cards, so the
/// tile reads as alive whether eliza is idling or presenting.
fn presence_overlay(s: &PresenceState) -> String {
    let accent = mood_color(s.mood);
    let brackets = corner_brackets(accent, s);
    let sticker = state_sticker(s.mood, accent);
    let hud = hud_sticker(s);
    let eq = eq_strip(s, accent);
    let pip = s
        .vision_thumb
        .map(|uri| vision_pip(uri, accent))
        .unwrap_or_default();
    let glitch = if s.glitch > 0.01 {
        glitch_overlay(s.glitch, s.t)
    } else {
        String::new()
    };
    format!("{brackets}\n{eq}\n{pip}\n{sticker}\n{hud}\n{glitch}")
}

/// Shared `<defs>` for the presence — gradients, halftone pattern,
/// sticker drop-shadow. `halftone_dot_r` widens the pattern's dot for
/// busier moods (`Thinking`, `Vision`).
fn presence_defs(accent: &str, halftone_dot_r: f32) -> String {
    format!(
        r##"<defs>
<radialGradient id="bg" cx="50%" cy="40%" r="80%">
<stop offset="0%" stop-color="#16213f"/>
<stop offset="100%" stop-color="#05070f"/>
</radialGradient>
<radialGradient id="orb" cx="42%" cy="38%" r="70%">
<stop offset="0%" stop-color="#f2f7ff"/>
<stop offset="44%" stop-color="{accent}"/>
<stop offset="100%" stop-color="#16306a"/>
</radialGradient>
<pattern id="halftone" x="0" y="0" width="14" height="14" patternUnits="userSpaceOnUse">
<circle cx="7" cy="7" r="{dot_r:.2}" fill="#1c2a48" opacity="0.7"/>
</pattern>
<filter id="sticker_shadow" x="-30%" y="-30%" width="160%" height="160%">
<feGaussianBlur in="SourceAlpha" stdDeviation="1.5"/>
<feOffset dx="1.5" dy="2" result="o"/>
<feFlood flood-color="#000" flood-opacity="0.55"/>
<feComposite in2="o" operator="in"/>
<feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
</defs>"##,
        dot_r = halftone_dot_r,
    )
}

/// Four L-shaped brackets at the tile corners — fat, loud, the primary
/// signal that the tile is *alive*. Length pulses with the audio that's
/// relevant to the current mood (own loudness while speaking, peer
/// loudness while listening, a deliberate breathe otherwise) and the
/// stroke is thick enough to read across the room.
fn corner_brackets(accent: &str, s: &PresenceState) -> String {
    let drive = match s.mood {
        Mood::Speaking => 0.4 + s.level * 0.9,
        Mood::Listening => 0.4 + s.peer * 0.9,
        Mood::Vision => 0.7 + (s.t * 1.8).sin().abs() * 0.3,
        Mood::Thinking => 0.45 + (s.t * 1.6).sin().abs() * 0.45,
        Mood::Idle => 0.35 + (s.t * 0.6).sin().abs() * 0.10,
    }
    .clamp(0.0, 1.0);
    let len = 36.0 + drive * 84.0;
    let pad = 14.0;
    let r = VIDEO_W as f32 - pad;
    let b = VIDEO_H as f32 - pad;
    let l = pad;
    let t = pad;
    // A second, thinner inner bracket at a slight offset — the
    // double-line look is unmistakably HUD/cyberpunk.
    let inner_pad = 22.0;
    let il = inner_pad;
    let it = inner_pad;
    let ir = VIDEO_W as f32 - inner_pad;
    let ib = VIDEO_H as f32 - inner_pad;
    let inner_len = (len - 14.0).max(18.0);
    format!(
        r##"<g stroke="{accent}" stroke-width="6" stroke-linecap="square" fill="none" opacity="1.0">
<path d="M{l:.1} {tl:.1} L{l:.1} {t:.1} L{ll:.1} {t:.1}"/>
<path d="M{rl:.1} {t:.1} L{r:.1} {t:.1} L{r:.1} {tl:.1}"/>
<path d="M{l:.1} {bl:.1} L{l:.1} {b:.1} L{ll:.1} {b:.1}"/>
<path d="M{rl:.1} {b:.1} L{r:.1} {b:.1} L{r:.1} {bl:.1}"/>
</g>
<g stroke="{accent}" stroke-width="1.5" stroke-linecap="square" fill="none" opacity="0.65">
<path d="M{il:.1} {itl:.1} L{il:.1} {it:.1} L{ill:.1} {it:.1}"/>
<path d="M{irl:.1} {it:.1} L{ir:.1} {it:.1} L{ir:.1} {itl:.1}"/>
<path d="M{il:.1} {ibl:.1} L{il:.1} {ib:.1} L{ill:.1} {ib:.1}"/>
<path d="M{irl:.1} {ib:.1} L{ir:.1} {ib:.1} L{ir:.1} {ibl:.1}"/>
</g>"##,
        tl = t + len,
        bl = b - len,
        ll = l + len,
        rl = r - len,
        itl = it + inner_len,
        ibl = ib - inner_len,
        ill = il + inner_len,
        irl = ir - inner_len,
    )
}

/// Big slanted sticker chip in the top-right naming the current state.
/// Loud sans-serif stencil over a soft drop-shadow — feels slapped on,
/// not rendered.
fn state_sticker(mood: Mood, accent: &str) -> String {
    let label = mood_label(mood);
    let w = 38 + label.chars().count() as i32 * 14;
    format!(
        r##"<g transform="translate(606 50) rotate(-5 0 0)">
<rect x="{nx}" y="-20" width="{w}" height="38" rx="3" fill="{accent}" filter="url(#sticker_shadow)"/>
<rect x="{nx}" y="-20" width="{w}" height="4" fill="#0a0f1f" opacity="0.25"/>
<text x="{tx}" y="9" text-anchor="middle" font-family="{FONT}" font-size="20" font-weight="900" fill="#0a0f1f" letter-spacing="3.5">{label}</text>
</g>"##,
        nx = -w,
        tx = -w / 2,
    )
}

/// Small mono-style HUD chip in the top-left — pure cyberpunk garnish.
/// A blinky tick + a fixed system-status readout. When the ambient
/// monitor has picked a topic, the chip swaps to that concept in the
/// ambient accent — so the tile keeps showing she's tracking the
/// conversation even when she isn't speaking.
fn hud_sticker(s: &PresenceState) -> String {
    let tick = if (s.t * 1.5).sin() > 0.0 { "●" } else { "○" };
    let (text, accent): (String, &str) = match s.ambient {
        Some((concept, accent)) => (
            concept.chars().take(22).collect::<String>().to_uppercase(),
            accent,
        ),
        None => ("MOQ ▸ LIVE".to_string(), "#3effd6"),
    };
    let chars = 2 + text.chars().count() as i32; // tick + space + text
    let w = (24 + chars * 11).max(120);
    format!(
        r##"<g transform="translate(30 50) rotate(2 0 0)">
<rect x="-8" y="-14" width="{w}" height="28" rx="3" fill="#0a0f1f" stroke="{accent}" stroke-width="1.5" opacity="0.92"/>
<text x="4" y="5" font-family="{FONT}" font-size="14" font-weight="900" fill="{accent}" letter-spacing="2.5">{tick} {text}</text>
</g>"##,
    )
}

/// Faint accent-tinted overlay over the bg — the "room is bathed in
/// this color" cue. Pulses with audio for the loud moods, ambient for
/// the calm ones. When ambient is set, a second tint in the topic
/// accent blends on top so the room subtly shifts colour with the
/// conversation.
fn mood_scrim(accent: &str, s: &PresenceState) -> String {
    let strength = match s.mood {
        Mood::Idle => 0.04,
        Mood::Listening => 0.07 + s.peer * 0.06,
        Mood::Thinking => 0.08 + (s.t * 1.6).sin().abs() * 0.04,
        Mood::Speaking => 0.06 + s.level * 0.10,
        Mood::Vision => 0.11 + (s.t * 5.0).sin().abs() * 0.05,
    };
    let base = format!(
        r##"<rect width="{w}" height="{h}" fill="{accent}" opacity="{strength:.3}"/>"##,
        w = VIDEO_W,
        h = VIDEO_H,
    );
    match s.ambient {
        Some((_, amb_accent)) => format!(
            r##"{base}<rect width="{w}" height="{h}" fill="{amb_accent}" opacity="0.06"/>"##,
            w = VIDEO_W,
            h = VIDEO_H,
        ),
        None => base,
    }
}

/// Bottom-edge waveform — `EQ_BARS` bars across most of the tile
/// width. Shows her own audio history while speaking, peer audio
/// history otherwise. The "who's making sound right now" cue — tall,
/// saturated, hard to miss.
fn eq_strip(s: &PresenceState, accent: &str) -> String {
    let history: &[f32] = match s.mood {
        Mood::Speaking => s.level_history,
        _ => s.peer_history,
    };
    if history.is_empty() {
        return String::new();
    }
    let bar_w = 14.0;
    let bar_gap = 4.0;
    let total = history.len();
    let strip_w = total as f32 * (bar_w + bar_gap) - bar_gap;
    let x0 = (VIDEO_W as f32 - strip_w) / 2.0;
    let baseline = VIDEO_H as f32 - 30.0;
    let max_h = 52.0;
    let mut bars = String::new();
    for (i, &v) in history.iter().enumerate() {
        // Boost low values so quiet speech still moves the bars.
        let h = (v * max_h * 6.0 + 3.0).clamp(3.0, max_h);
        let x = x0 + i as f32 * (bar_w + bar_gap);
        bars.push_str(&format!(
            r##"<rect x="{x:.1}" y="{y:.1}" width="{bar_w:.1}" height="{h:.1}" rx="2" fill="{accent}" opacity="0.95"/>"##,
            y = baseline - h,
        ));
    }
    // Baseline rail under the bars — anchors the strip visually.
    let rail_x = x0 - 6.0;
    let rail_w = strip_w + 12.0;
    bars.push_str(&format!(
        r##"<rect x="{rail_x:.1}" y="{by:.1}" width="{rail_w:.1}" height="2" fill="{accent}" opacity="0.5"/>"##,
        by = baseline + 2.0,
    ));
    bars
}

/// Picture-in-picture of the frame she's currently analyzing — visible
/// only in [`Mood::Vision`]. Crosshair brackets around it; an
/// `ANALYZING` strip across the bottom of the inset. The whole point:
/// remove all doubt that she's actually looking.
fn vision_pip(data_uri: &str, accent: &str) -> String {
    let w = 144.0;
    let h = 82.0;
    let x = 20.0;
    let y = 60.0;
    let bw = 12.0;
    let xr = x + w;
    let yh = y + h;
    let label_h = 14.0;
    format!(
        r##"<g>
<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="#000" opacity="0.85"/>
<image href="{uri}" x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" preserveAspectRatio="xMidYMid slice"/>
<g stroke="{accent}" stroke-width="2.5" fill="none">
<path d="M{x:.1} {yb:.1} L{x:.1} {y:.1} L{xb:.1} {y:.1}"/>
<path d="M{xrb:.1} {y:.1} L{xr:.1} {y:.1} L{xr:.1} {yb:.1}"/>
<path d="M{x:.1} {ybb:.1} L{x:.1} {yh:.1} L{xb:.1} {yh:.1}"/>
<path d="M{xrb:.1} {yh:.1} L{xr:.1} {yh:.1} L{xr:.1} {ybb:.1}"/>
</g>
<rect x="{x:.1}" y="{ylabel:.1}" width="{w:.1}" height="{label_h:.1}" fill="{accent}"/>
<text x="{xc:.1}" y="{yt:.1}" font-family="{FONT}" font-size="9" font-weight="900" fill="#0a0f1f" text-anchor="middle" letter-spacing="2.5">ANALYZING</text>
</g>"##,
        uri = data_uri,
        xb = x + bw,
        xrb = xr - bw,
        yb = y + bw,
        ybb = yh - bw,
        ylabel = yh - label_h,
        xc = x + w / 2.0,
        yt = yh - 4.0,
    )
}

/// RGB-split + scanline burst that fires on every mood change.
/// Intensity fades from 1 → 0 in the renderer; only flashes on
/// transition. Loud: many scanlines, big chromatic shift, hot colors.
fn glitch_overlay(intensity: f32, t: f32) -> String {
    let seeds: &[(f32, f32, &str)] = &[
        (0.05, 0.8, "#ff3ec8"),
        (0.18, 0.4, "#3effd6"),
        (0.31, 0.6, "#ffea3e"),
        (0.43, 0.3, "#ff3ec8"),
        (0.57, 0.7, "#3effd6"),
        (0.68, 0.5, "#ffea3e"),
        (0.79, 0.9, "#ff3ec8"),
        (0.92, 0.6, "#3effd6"),
    ];
    let mut bars = String::new();
    for &(seed, hseed, color) in seeds {
        let y = ((seed + t * 1.6) % 1.0) * VIDEO_H as f32;
        let h = 3.0 + hseed * 9.0;
        let alpha = intensity * 0.9;
        bars.push_str(&format!(
            r##"<rect x="0" y="{y:.1}" width="{w}" height="{h:.1}" fill="{color}" opacity="{alpha:.3}"/>"##,
            w = VIDEO_W,
        ));
    }
    let off = intensity * 10.0;
    let cab = intensity * 0.13;
    // A wider black-bar tear at the top for the worst of the burst —
    // makes the transition unmistakable.
    let tear_h = intensity * 14.0;
    let tear_y = ((t * 0.7).fract()) * (VIDEO_H as f32 - tear_h);
    format!(
        r##"{bars}
<rect x="0" y="{tear_y:.1}" width="{w}" height="{tear_h:.1}" fill="#000" opacity="{tear_op:.3}"/>
<rect x="{ro:.1}" y="0" width="{w}" height="{h}" fill="#ff0066" opacity="{cab:.3}"/>
<rect x="{bo:.1}" y="0" width="{w}" height="{h}" fill="#00ddff" opacity="{cab:.3}"/>"##,
        ro = -off,
        bo = off,
        w = VIDEO_W,
        h = VIDEO_H,
        tear_op = intensity * 0.7,
    )
}

// ---------------------------------------------------------------------
// Scene cards
// ---------------------------------------------------------------------

/// Reusable `<defs>`: background/glow/panel gradients and the soft
/// drop-shadow + text-glow filters. Accent-tinted.
fn defs(accent: &str) -> String {
    format!(
        r##"<defs>
<linearGradient id="bg" x1="0" y1="0" x2="0.35" y2="1">
<stop offset="0" stop-color="#0b1020"/><stop offset="1" stop-color="#04050d"/>
</linearGradient>
<radialGradient id="glow" cx="50%" cy="50%" r="50%">
<stop offset="0" stop-color="{accent}" stop-opacity="0.34"/>
<stop offset="100%" stop-color="{accent}" stop-opacity="0"/>
</radialGradient>
<radialGradient id="vig" cx="50%" cy="42%" r="78%">
<stop offset="52%" stop-color="#000000" stop-opacity="0"/>
<stop offset="100%" stop-color="#000000" stop-opacity="0.6"/>
</radialGradient>
<linearGradient id="panel" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="#1b2547" stop-opacity="0.95"/>
<stop offset="1" stop-color="#0e1430" stop-opacity="0.95"/>
</linearGradient>
<linearGradient id="scrimV" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="#04050d" stop-opacity="0.44"/>
<stop offset="0.6" stop-color="#04050d" stop-opacity="0.62"/>
<stop offset="1" stop-color="#04050d" stop-opacity="0.9"/>
</linearGradient>
<linearGradient id="scrimL" x1="0" y1="0" x2="1" y2="0">
<stop offset="0" stop-color="#04050d" stop-opacity="0.82"/>
<stop offset="0.55" stop-color="#04050d" stop-opacity="0.12"/>
<stop offset="1" stop-color="#04050d" stop-opacity="0"/>
</linearGradient>
<filter id="shadow" x="-40%" y="-40%" width="180%" height="180%">
<feGaussianBlur in="SourceAlpha" stdDeviation="8"/>
<feOffset dy="6" result="o"/>
<feFlood flood-color="#000000" flood-opacity="0.5"/>
<feComposite in2="o" operator="in"/>
<feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
<filter id="sticker_shadow" x="-30%" y="-30%" width="160%" height="160%">
<feGaussianBlur in="SourceAlpha" stdDeviation="1.5"/>
<feOffset dx="1.5" dy="2" result="o"/>
<feFlood flood-color="#000000" flood-opacity="0.55"/>
<feComposite in2="o" operator="in"/>
<feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
<filter id="textglow" x="-70%" y="-70%" width="240%" height="240%">
<feGaussianBlur stdDeviation="7" result="b"/>
<feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
</defs>"##
    )
}

/// The shared background. With no image it's a deep gradient + drifting
/// accent glow; with a generated/fetched backdrop it's that image (slow
/// Ken Burns pan-zoom) under a legibility scrim.
fn backdrop(accent: &str, t: f32, image: Option<(&str, f32)>) -> String {
    if let Some((uri, age)) = image {
        let fade = ease_out(age / 0.9);
        let zoom = 1.05 + (age * 0.0055).min(0.13);
        let panx = (age * 0.05).sin() * 9.0;
        let pany = (age * 0.043).cos() * 6.0;
        return format!(
            r##"<rect width="640" height="360" fill="#04050d"/>
<g opacity="{fade:.3}" transform="translate(320 180) scale({zoom:.4}) translate(-320 -180) translate({panx:.1} {pany:.1})">
<image href="{uri}" x="0" y="0" width="640" height="360" preserveAspectRatio="xMidYMid slice"/>
</g>
<rect width="640" height="360" fill="url(#scrimV)"/>
<rect width="640" height="360" fill="url(#scrimL)"/>
<rect width="640" height="360" fill="url(#vig)"/>
<rect x="7" y="7" width="626" height="346" rx="14" fill="none" stroke="{accent}" stroke-width="1" opacity="0.18"/>"##
        );
    }
    let gx = 320.0 + (t * 0.17).sin() * 130.0;
    let gy = 150.0 + (t * 0.12).cos() * 64.0;
    format!(
        r##"<rect width="640" height="360" fill="url(#bg)"/>
<ellipse cx="{gx:.0}" cy="{gy:.0}" rx="380" ry="320" fill="url(#glow)"/>
<rect width="640" height="360" fill="url(#vig)"/>
<rect x="7" y="7" width="626" height="346" rx="14" fill="none" stroke="{accent}" stroke-width="1" opacity="0.14"/>"##
    )
}

/// Wrap a scene body in the full SVG document with defs, backdrop, and
/// the presence overlay primitives on top. `image` is an optional
/// backdrop: `(data-uri, age-seconds)`.
fn frame(
    accent: &str,
    t: f32,
    image: Option<(&str, f32)>,
    body: &str,
    presence: &PresenceState,
) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 640 360" preserveAspectRatio="xMidYMid slice">
{defs}
{backdrop}
{body}
{overlay}
</svg>"##,
        w = VIDEO_W,
        h = VIDEO_H,
        defs = defs(accent),
        backdrop = backdrop(accent, t, image),
        overlay = presence_overlay(presence),
    )
}

/// Render the current scene — dispatches on [`SceneKind`]. The presence
/// overlay primitives (brackets, sticker, EQ, glitch, PiP) compose on
/// top so a scene card doesn't take the tile over: eliza still reads as
/// alive while she's presenting.
fn scene_svg(scene: &Scene, presence: &PresenceState) -> String {
    let s = &scene.spec;
    let e = scene.shown_at.elapsed().as_secs_f32();
    let accent = s.accent.as_str();
    let body = match s.kind {
        SceneKind::Hero => hero_body(s, e, accent),
        SceneKind::KeyPoints => key_points_body(s, e, accent),
        SceneKind::Stat => stat_body(s, e, accent),
        SceneKind::Quote => quote_body(s, e, accent),
        SceneKind::Timeline => timeline_body(s, e, accent),
    };
    let image = scene
        .image
        .as_ref()
        .map(|(uri, at)| (uri.as_str(), at.elapsed().as_secs_f32()));
    frame(accent, presence.t, image, &body, presence)
}

// ---------------------------------------------------------------------
// Whiteboard
// ---------------------------------------------------------------------

/// Render the current whiteboard — reveals steps one at a time at
/// [`BOARD_STEP_MS`] intervals, each fading in over [`BOARD_REVEAL_MS`].
/// The presence overlay primitives compose on top so she still reads as
/// alive while drawing.
fn board_svg(board: &Board, presence: &PresenceState) -> String {
    let elapsed_ms = board.shown_at.elapsed().as_millis() as u32;
    let mut body = String::new();
    for (i, step) in board.steps.iter().enumerate() {
        let reveal_at = i as u32 * BOARD_STEP_MS;
        if reveal_at > elapsed_ms {
            continue;
        }
        let age = elapsed_ms - reveal_at;
        let progress = (age as f32 / BOARD_REVEAL_MS as f32).clamp(0.0, 1.0);
        body.push_str(&render_step(step, progress, &board.accent));
    }
    frame(&board.accent, presence.t, None, &body, presence)
}

/// Render one whiteboard step with a fade-in (`progress` 0→1).
fn render_step(step: &Step, progress: f32, accent: &str) -> String {
    let op = progress;
    // Slight grow-in for a "stamped down" feel.
    let scale = 0.94 + progress * 0.06;
    match step {
        Step::Box { x, y, w, h, label } => {
            let cx = x + w / 2.0;
            let cy = y + h / 2.0;
            let label_y = cy + 5.0;
            format!(
                r##"<g opacity="{op:.3}" transform="translate({cx:.1} {cy:.1}) scale({scale:.3}) translate({mcx:.1} {mcy:.1})">
<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="8" fill="url(#panel)" stroke="{accent}" stroke-width="2.5" filter="url(#shadow)"/>
<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="2" rx="0" fill="{accent}" opacity="0.7"/>
<text x="{cx:.1}" y="{label_y:.1}" text-anchor="middle" font-family="{FONT}" font-size="15" font-weight="800" fill="#eaf0ff">{label}</text>
</g>"##,
                mcx = -cx,
                mcy = -cy,
                label = xml_escape(label),
            )
        }
        Step::Arrow { x1, y1, x2, y2, label } => {
            // Arrowhead as an inline triangle at (x2,y2).
            let dx = x2 - x1;
            let dy = y2 - y1;
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let ux = dx / len;
            let uy = dy / len;
            let head_len = 12.0;
            let head_w = 7.0;
            // Tip
            let tx = *x2;
            let ty = *y2;
            // Two base corners of the triangle (perpendicular to dir).
            let bx = tx - head_len * ux;
            let by = ty - head_len * uy;
            let lx = bx - head_w * uy;
            let ly = by + head_w * ux;
            let rx = bx + head_w * uy;
            let ry = by - head_w * ux;
            // Shorten the line so it doesn't poke through the head.
            let lx2 = tx - (head_len - 1.0) * ux;
            let ly2 = ty - (head_len - 1.0) * uy;
            let label_svg = match label {
                Some(l) if !l.is_empty() => {
                    let mlx = (x1 + x2) / 2.0;
                    let mly = (y1 + y2) / 2.0 - 6.0;
                    format!(
                        r##"<text x="{mlx:.1}" y="{mly:.1}" text-anchor="middle" font-family="{FONT}" font-size="12" font-weight="800" fill="{accent}">{l}</text>"##,
                        l = xml_escape(l),
                    )
                }
                _ => String::new(),
            };
            format!(
                r##"<g opacity="{op:.3}">
<line x1="{x1:.1}" y1="{y1:.1}" x2="{lx2:.1}" y2="{ly2:.1}" stroke="{accent}" stroke-width="3" stroke-linecap="round"/>
<path d="M{tx:.1} {ty:.1} L{lx:.1} {ly:.1} L{rx:.1} {ry:.1} Z" fill="{accent}"/>
{label_svg}
</g>"##,
            )
        }
        Step::Text { x, y, content, size } => {
            let fs = size.px();
            let weight = if matches!(size, crate::whiteboard::TextSize::Large) {
                900
            } else {
                700
            };
            format!(
                r##"<text x="{x:.1}" y="{y:.1}" font-family="{FONT}" font-size="{fs}" font-weight="{weight}" fill="#eaf0ff" opacity="{op:.3}">{content}</text>"##,
                content = xml_escape(content),
            )
        }
    }
}

/// Hero: a big headline with a one-line takeaway under it.
fn hero_body(s: &SceneSpec, e: f32, accent: &str) -> String {
    let bar = reveal(e, 0.04, 0.4);
    let head = wrap(&s.title, 17);
    let head_lh = 49.0;
    let head_y0 = 152.0;
    let headsvg = lines_svg(
        &head, 46.0, head_y0, head_lh, 45.0, 800, "#f1f5ff", "start", e, 0.12, 0.09,
    );
    let sub = wrap(&s.subtitle, 50);
    let sub_y0 = head_y0 + (head.len() as f32 - 1.0) * head_lh + 44.0;
    let subsvg = lines_svg(
        &sub, 47.0, sub_y0, 27.0, 19.0, 400, "#95a4c9", "start", e, 0.36, 0.07,
    );
    format!(
        r##"<circle cx="602" cy="344" r="118" fill="none" stroke="{accent}" stroke-width="1.5" opacity="0.12"/>
<circle cx="602" cy="344" r="72" fill="none" stroke="{accent}" stroke-width="1" opacity="0.10"/>
<rect x="46" y="98" width="{bw:.1}" height="4" rx="2" fill="{accent}" opacity="{bar:.3}"/>
{headsvg}{subsvg}"##,
        bw = 18.0 + bar * 30.0,
    )
}

/// Key points: a title and a numbered list of panelled points.
fn key_points_body(s: &SceneSpec, e: f32, accent: &str) -> String {
    let title = wrap(&s.title, 34);
    let title_lh = 33.0;
    let titlesvg = lines_svg(
        &title, 46.0, 78.0, title_lh, 27.0, 700, "#eef2ff", "start", e, 0.04, 0.07,
    );
    let title_bottom = 78.0 + (title.len() as f32 - 1.0) * title_lh;
    let ul = reveal(e, 0.1, 0.4);
    let underline = format!(
        r##"<rect x="46" y="{uy:.1}" width="{uw:.1}" height="3" rx="1.5" fill="{accent}" opacity="{ul:.3}"/>"##,
        uy = title_bottom + 12.0,
        uw = 26.0 + ul * 34.0,
    );

    let pts: Vec<&String> = s.points.iter().take(5).collect();
    let n = pts.len().max(1);
    let region_top = title_bottom + 30.0;
    let pitch = ((338.0 - region_top) / n as f32).min(58.0);
    let panel_h = pitch - 10.0;
    let mut panels = String::new();
    for (i, pt) in pts.iter().enumerate() {
        let p = reveal(e, 0.2 + i as f32 * 0.12, 0.5);
        if p <= 0.001 {
            continue;
        }
        let y = region_top + i as f32 * pitch;
        let cy = y + panel_h / 2.0;
        let dx = (1.0 - p) * 22.0;
        panels.push_str(&format!(
            r##"<g opacity="{p:.3}" transform="translate({dx:.1} 0)">
<rect x="46" y="{y:.1}" width="548" height="{panel_h:.1}" rx="12" fill="url(#panel)" stroke="{accent}" stroke-opacity="0.28" stroke-width="1"/>
<rect x="46" y="{y:.1}" width="548" height="1.4" rx="0.7" fill="#ffffff" opacity="0.06"/>
<circle cx="78" cy="{cy:.1}" r="15" fill="{accent}"/>
<text x="78" y="{nty:.1}" font-family="{FONT}" font-size="16" font-weight="800" fill="#0a0f1f" text-anchor="middle">{num}</text>
<text x="106" y="{tty:.1}" font-family="{FONT}" font-size="18" font-weight="500" fill="#dde6ff">{txt}</text>
</g>
"##,
            nty = cy + 5.6,
            tty = cy + 6.0,
            num = i + 1,
            txt = xml_escape(&truncate(pt, 52)),
        ));
    }
    format!(r##"{titlesvg}{underline}<g filter="url(#shadow)">{panels}</g>"##)
}

/// Stat: a single big number with a label above and context below. The
/// number's font size auto-fits its width so long values still fit.
fn stat_body(s: &SceneSpec, e: f32, accent: &str) -> String {
    let value = truncate(
        s.points.first().map(String::as_str).unwrap_or("—"),
        18,
    );
    let label = s.title.to_uppercase();
    let lp = reveal(e, 0.05, 0.4);
    let np = reveal(e, 0.16, 0.6);
    let scale = 0.68 + 0.32 * np;
    // Auto-fit: keep the number within ~520px of width.
    let chars = value.chars().count().max(1) as f32;
    let fs = (520.0 / (0.60 * chars)).clamp(40.0, 96.0);
    let ctx = wrap(&s.subtitle, 42);
    let ctxsvg = lines_svg(
        &ctx, 320.0, 282.0, 24.0, 17.0, 400, "#93a2cb", "middle", e, 0.5, 0.07,
    );
    format!(
        r##"<text x="320" y="122" font-family="{FONT}" font-size="15" font-weight="700" letter-spacing="3" fill="#8fa0c8" text-anchor="middle" opacity="{lp:.3}">{label}</text>
<rect x="{lx:.1}" y="134" width="{lw:.1}" height="2.4" rx="1.2" fill="{accent}" opacity="{lp:.3}"/>
<g opacity="{np:.3}" transform="translate(320 208) scale({scale:.3}) translate(-320 -208)">
<text x="320" y="{ny:.1}" font-family="{FONT}" font-size="{fs:.0}" font-weight="800" fill="{accent}" text-anchor="middle" filter="url(#textglow)">{value}</text>
</g>
{ctxsvg}"##,
        label = xml_escape(&label),
        value = xml_escape(&value),
        lx = 320.0 - (28.0 + lp * 24.0) / 2.0,
        lw = 28.0 + lp * 24.0,
        ny = 208.0 + fs * 0.34,
    )
}

/// Quote: a large statement with an attribution.
fn quote_body(s: &SceneSpec, e: f32, accent: &str) -> String {
    let q = wrap(&s.title, 30);
    let q_lh = 37.0;
    let q_y0 = 170.0 - (q.len() as f32 - 1.0) * q_lh * 0.5;
    let qsvg = lines_svg(
        &q, 96.0, q_y0, q_lh, 26.0, 500, "#e9eeff", "start", e, 0.14, 0.1,
    );
    let glyph = reveal(e, 0.0, 0.5);
    let attr_delay = 0.14 + q.len() as f32 * 0.1 + 0.1;
    let attr = reveal(e, attr_delay, 0.5);
    let attr_y = q_y0 + (q.len() as f32 - 1.0) * q_lh + 46.0;
    format!(
        r##"<text x="44" y="190" font-family="Georgia, {FONT}" font-size="170" font-weight="700" fill="{accent}" opacity="{go:.3}">“</text>
{qsvg}
<g opacity="{ao:.3}">
<rect x="98" y="{ay:.1}" width="30" height="3" rx="1.5" fill="{accent}"/>
<text x="140" y="{aty:.1}" font-family="{FONT}" font-size="16" font-weight="600" fill="{accent}" letter-spacing="1">{attr_text}</text>
</g>"##,
        go = glyph * 0.26,
        ao = attr,
        ay = attr_y,
        aty = attr_y + 5.0,
        attr_text = xml_escape(&s.subtitle),
    )
}

/// Timeline: ordered steps strung along a connecting line.
fn timeline_body(s: &SceneSpec, e: f32, accent: &str) -> String {
    let title = wrap(&s.title, 34);
    let title_lh = 33.0;
    let titlesvg = lines_svg(
        &title, 46.0, 78.0, title_lh, 27.0, 700, "#eef2ff", "start", e, 0.04, 0.07,
    );
    let title_bottom = 78.0 + (title.len() as f32 - 1.0) * title_lh;
    let ul = reveal(e, 0.1, 0.4);
    let underline = format!(
        r##"<rect x="46" y="{uy:.1}" width="{uw:.1}" height="3" rx="1.5" fill="{accent}" opacity="{ul:.3}"/>"##,
        uy = title_bottom + 12.0,
        uw = 26.0 + ul * 34.0,
    );

    let pts: Vec<&String> = s.points.iter().take(5).collect();
    let n = pts.len().max(1);
    let region_top = title_bottom + 40.0;
    let pitch = ((332.0 - region_top) / n as f32).min(60.0);
    let line_x = 74.0;
    let node_cy = |i: usize| region_top + i as f32 * pitch + pitch * 0.5 - 8.0;

    let mut nodes = String::new();
    for (i, pt) in pts.iter().enumerate() {
        let delay = 0.22 + i as f32 * 0.14;
        let p = reveal(e, delay, 0.5);
        if p <= 0.001 {
            continue;
        }
        let cy = node_cy(i);
        if i > 0 {
            let prev = node_cy(i - 1);
            let seg = cy - prev;
            nodes.push_str(&format!(
                r##"<line x1="{line_x:.1}" y1="{prev:.1}" x2="{line_x:.1}" y2="{cy:.1}" stroke="{accent}" stroke-width="2.5" stroke-dasharray="{seg:.1}" stroke-dashoffset="{off:.1}" opacity="0.55"/>"##,
                off = seg * (1.0 - p),
            ));
        }
        let label = wrap(pt, 40);
        let labelsvg = lines_svg(
            &label, 104.0, cy + 6.0, 22.0, 18.0, 500, "#dce6ff", "start", e, delay + 0.06, 0.05,
        );
        nodes.push_str(&format!(
            r##"<g opacity="{p:.3}">
<circle cx="{line_x:.1}" cy="{cy:.1}" r="14" fill="{accent}"/>
<circle cx="{line_x:.1}" cy="{cy:.1}" r="14" fill="none" stroke="#ffffff" stroke-opacity="0.18" stroke-width="1"/>
<text x="{line_x:.1}" y="{nty:.1}" font-family="{FONT}" font-size="15" font-weight="800" fill="#0a0f1f" text-anchor="middle">{num}</text>
</g>
{labelsvg}"##,
            nty = cy + 5.2,
            num = i + 1,
        ));
    }
    format!("{titlesvg}{underline}{nodes}")
}

// ---------------------------------------------------------------------
// Overlay layer for the particles render backend
// ---------------------------------------------------------------------

/// Build an SVG layer that contains JUST the rich overlays from this
/// module — scene card (with backdrop image), whiteboard, ambient HUD
/// chip, vision PiP — without the orb / corner brackets / EQ strip /
/// state sticker (those would fight the particle face).
///
/// Returns `None` when nothing's worth drawing — so the particles
/// renderer can skip the rasterize + composite step on quiet frames.
/// The caller (`video_particles::render_loop`) rasterizes the SVG to
/// a Pixmap and draw-pixmaps it on top of the particle field.
///
/// `time` is monotonic seconds since the renderer started (drives the
/// HUD blink + the no-image backdrop's drifting glow).
pub(crate) fn overlay_svg_for_particles(tile: &VideoTile, time: f32) -> Option<String> {
    // ── Snapshot the overlay state ──
    let scene_data = tile.scene.lock().ok().and_then(|g| {
        g.as_ref().filter(|s| s.is_visible()).map(|s| {
            let e = s.shown_at.elapsed().as_secs_f32();
            let body = match s.spec.kind {
                SceneKind::Hero => hero_body(&s.spec, e, &s.spec.accent),
                SceneKind::KeyPoints => key_points_body(&s.spec, e, &s.spec.accent),
                SceneKind::Stat => stat_body(&s.spec, e, &s.spec.accent),
                SceneKind::Quote => quote_body(&s.spec, e, &s.spec.accent),
                SceneKind::Timeline => timeline_body(&s.spec, e, &s.spec.accent),
            };
            let image = s
                .image
                .as_ref()
                .map(|(uri, at)| (uri.clone(), at.elapsed().as_secs_f32()));
            (s.spec.accent.clone(), body, image)
        })
    });
    let board_data = tile.board.lock().ok().and_then(|g| {
        g.as_ref().filter(|b| b.is_visible()).map(|b| {
            let elapsed_ms = b.shown_at.elapsed().as_millis() as u32;
            let mut body = String::new();
            for (i, step) in b.steps.iter().enumerate() {
                let reveal_at = i as u32 * BOARD_STEP_MS;
                if reveal_at > elapsed_ms {
                    continue;
                }
                let age = elapsed_ms - reveal_at;
                let progress = (age as f32 / BOARD_REVEAL_MS as f32).clamp(0.0, 1.0);
                body.push_str(&render_step(step, progress, &b.accent));
            }
            (b.accent.clone(), body)
        })
    });
    let ambient = tile.ambient.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|a| a.is_visible())
            .map(|a| (a.concept.clone(), a.accent.clone()))
    });
    let ambient_image_uri = tile.ambient_image.lock().ok().and_then(|g| {
        g.as_ref().filter(|ai| ai.is_visible()).and_then(|ai| {
            ai.image.as_ref().map(|(uri, at)| (uri.clone(), at.elapsed().as_secs_f32()))
        })
    });
    let vision_thumb = tile.vision_thumb.lock().ok().and_then(|g| g.clone());

    // Quiet frame — nothing to draw, skip the rasterize cost.
    if scene_data.is_none()
        && board_data.is_none()
        && ambient.is_none()
        && ambient_image_uri.is_none()
        && vision_thumb.is_none()
    {
        return None;
    }

    // ── Pick an accent for the shared `defs()` (gradients tint to it).
    let accent = scene_data
        .as_ref()
        .map(|(a, _, _)| a.clone())
        .or_else(|| board_data.as_ref().map(|(a, _)| a.clone()))
        .or_else(|| ambient.as_ref().map(|(_, a)| a.clone()))
        .unwrap_or_else(|| DEFAULT_ACCENT.to_string());

    // ── Compose the document ──
    // viewBox stays at the design-coordinate space (640×360) — every
    // overlay body in this module uses hardcoded 640×360-relative
    // positions. The renderer scales to the actual pixmap size, so
    // the layout always fills the canvas regardless of VIDEO_W/H.
    let mut svg = String::new();
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{VIDEO_W}" height="{VIDEO_H}" viewBox="0 0 640 360" preserveAspectRatio="xMidYMid slice">
"##,
    ));
    svg.push_str(&defs(&accent));

    // Scene card (with image backdrop if available) takes the centre
    // of the canvas. The backdrop fills the whole frame; the particle
    // face renders UNDER this overlay so it shows through the
    // semi-transparent scrim around the image.
    //
    // Scene takes priority over ambient image — when an actual
    // informational scene is up, that's the more important visual.
    if let Some((sa, body, image)) = scene_data {
        let img_ref = image.as_ref().map(|(uri, age)| (uri.as_str(), *age));
        svg.push_str(&backdrop(&sa, time, img_ref));
        svg.push_str(&body);
    } else if let Some((_, body)) = board_data {
        // Board has no backdrop — it's strokes-on-the-particle-field.
        svg.push_str(&body);
    } else if let Some((uri, age)) = ambient_image_uri {
        // Ambient image — text-less, low opacity, subtle Ken Burns.
        // No title, no points, no scrim layout — just the picture as
        // a mood backdrop. Stays faint enough that the particle face
        // remains the focal point.
        let fade = ease_out(age / 1.4).min(0.55); // capped low — never dominates
        let zoom = 1.03 + (age * 0.0035).min(0.08);
        let panx = (age * 0.04).sin() * 7.0;
        let pany = (age * 0.033).cos() * 5.0;
        svg.push_str(&format!(
            r##"<g opacity="{fade:.3}" transform="translate(320 180) scale({zoom:.4}) translate(-320 -180) translate({panx:.1} {pany:.1})">
<image href="{uri}" x="0" y="0" width="640" height="360" preserveAspectRatio="xMidYMid slice"/>
</g>
<rect width="640" height="360" fill="url(#scrimV)" opacity="0.4"/>
"##
        ));
    }

    // Ambient HUD chip — inlined from `hud_sticker` so we don't need
    // a full PresenceState.
    if let Some((concept, amb_accent)) = ambient {
        let tick = if (time * 1.5).sin() > 0.0 { "●" } else { "○" };
        let text: String = concept
            .chars()
            .take(22)
            .collect::<String>()
            .to_uppercase();
        let chars = 2 + text.chars().count() as i32;
        let w = (24 + chars * 11).max(120);
        svg.push_str(&format!(
            r##"<g transform="translate(30 50) rotate(2 0 0)">
<rect x="-8" y="-14" width="{w}" height="28" rx="3" fill="#0a0f1f" stroke="{amb_accent}" stroke-width="1.5" opacity="0.92"/>
<text x="4" y="5" font-family="{FONT}" font-size="14" font-weight="900" fill="{amb_accent}" letter-spacing="2.5">{tick} {text}</text>
</g>
"##
        ));
    }

    // Vision PiP — show what she's looking at.
    if let Some(thumb) = vision_thumb {
        svg.push_str(&vision_pip(&thumb, &accent));
    }

    svg.push_str("</svg>");
    Some(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt() -> resvg::usvg::Options<'static> {
        resvg::usvg::Options::default()
    }

    fn sample_spec(kind: SceneKind) -> SceneSpec {
        SceneSpec {
            kind,
            title: "The Apollo Program".into(),
            subtitle: "Humanity first reached the Moon in 1969".into(),
            points: vec![
                "Saturn V cleared the tower".into(),
                "Translunar injection burn".into(),
                "Eagle landed in the Sea of Tranquility".into(),
            ],
            accent: "#6cb0ff".into(),
            image_query: "the moon over a launch pad".into(),
        }
    }

    #[test]
    fn presence_rasterizes_in_every_mood() {
        let opt = opt();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H).unwrap();
        // A bit of history so the EQ strip actually has bars to draw,
        // and a tiny vision thumb so the PiP branch renders too.
        let lh: Vec<f32> = (0..EQ_BARS).map(|i| (i as f32 * 0.1).sin().abs() * 0.5).collect();
        let ph: Vec<f32> = (0..EQ_BARS).map(|i| (i as f32 * 0.13).cos().abs() * 0.4).collect();
        // A 1×1 black JPEG (smallest valid encode), as a data URI.
        let tiny_thumb = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/2wBDAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAv/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFQEBAQAAAAAAAAAAAAAAAAAAAAX/xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oADAMBAAIRAxEAPwA/8AAA/9k=";
        for mood in [
            Mood::Idle,
            Mood::Listening,
            Mood::Thinking,
            Mood::Speaking,
            Mood::Vision,
        ] {
            // Exercise the glitch overlay too — fires on every transition,
            // so render with a mid-strength glitch in this stamp.
            let state = PresenceState {
                mood,
                t: 1.7,
                level: 0.5,
                peer: 0.4,
                level_history: &lh,
                peer_history: &ph,
                glitch: 0.6,
                vision_thumb: if mood == Mood::Vision { Some(tiny_thumb) } else { None },
                ambient: None,
            };
            let frame = rasterize(&presence_svg(&state), &opt, &mut pixmap)
                .expect("presence must rasterize");
            assert_eq!(frame.dimensions, [VIDEO_W, VIDEO_H]);
        }
    }

    #[test]
    fn every_scene_kind_rasterizes_while_animating() {
        let opt = opt();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H).unwrap();
        for kind in [
            SceneKind::Hero,
            SceneKind::KeyPoints,
            SceneKind::Stat,
            SceneKind::Quote,
            SceneKind::Timeline,
        ] {
            // Just-appeared, mid-reveal, and settled.
            for back in [0.0_f32, 0.7, 3.0] {
                let scene = Scene {
                    spec: sample_spec(kind),
                    shown_at: Instant::now() - Duration::from_secs_f32(back),
                    id: 0,
                    image: None,
                };
                let state = PresenceState {
                    mood: Mood::Speaking,
                    t: 1.5,
                    level: 0.4,
                    peer: 0.0,
                    level_history: &[],
                    peer_history: &[],
                    glitch: 0.0,
                    vision_thumb: None,
                    ambient: None,
                };
                let svg = scene_svg(&scene, &state);
                let frame = rasterize(&svg, &opt, &mut pixmap)
                    .unwrap_or_else(|| panic!("{kind:?} must rasterize"));
                assert_eq!(frame.dimensions, [VIDEO_W, VIDEO_H]);
            }
        }
    }

    #[test]
    fn xml_escape_neutralizes_markup() {
        assert_eq!(xml_escape("a<b>&\"c"), "a&lt;b&gt;&amp;&quot;c");
    }

    #[test]
    fn validate_accent_rejects_garbage() {
        assert_eq!(validate_accent("#1A2b3C"), "#1A2b3C");
        assert_eq!(validate_accent(" #abcdef "), "#abcdef");
        assert_eq!(validate_accent("blue"), DEFAULT_ACCENT);
        assert_eq!(validate_accent("#xyz123"), DEFAULT_ACCENT);
        assert_eq!(validate_accent("#12345"), DEFAULT_ACCENT);
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        let lines = wrap("one two three four five six", 9);
        assert!(lines.len() >= 3, "should wrap into several lines");
        assert!(lines.iter().all(|l| !l.starts_with(' ') && !l.ends_with(' ')));
    }

    #[test]
    #[ignore = "dev tool: renders sample scenes to /tmp for visual review"]
    fn dump_scene_pngs() {
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H).unwrap();
        let specs = [
            SceneSpec {
                kind: SceneKind::Hero,
                title: "The Deep Ocean Is Unmapped".into(),
                subtitle: "Over 80% of the seafloor has never been directly observed".into(),
                points: vec![],
                accent: "#3fa9f5".into(),
                image_query: String::new(),
            },
            SceneSpec {
                kind: SceneKind::KeyPoints,
                title: "Why Sleep Matters".into(),
                subtitle: String::new(),
                points: vec![
                    "Consolidates memory and learning".into(),
                    "Clears metabolic waste from the brain".into(),
                    "Regulates mood and hormones".into(),
                    "Restores the immune system".into(),
                ],
                accent: "#b594ff".into(),
                image_query: String::new(),
            },
            SceneSpec {
                kind: SceneKind::Stat,
                title: "Speed of Light".into(),
                subtitle: "The universe's hard limit on how fast information travels".into(),
                points: vec!["299,792 km/s".into()],
                accent: "#ffd166".into(),
                image_query: String::new(),
            },
            SceneSpec {
                kind: SceneKind::Quote,
                title: "The good life is one inspired by love and guided by knowledge".into(),
                subtitle: "Bertrand Russell".into(),
                points: vec![],
                accent: "#ff7a9c".into(),
                image_query: String::new(),
            },
            SceneSpec {
                kind: SceneKind::Timeline,
                title: "How a Bill Becomes Law".into(),
                subtitle: String::new(),
                points: vec![
                    "Introduced in the House".into(),
                    "Reviewed by committee".into(),
                    "Debated and voted on".into(),
                    "Passes to the Senate".into(),
                    "Signed by the President".into(),
                ],
                accent: "#54e2c8".into(),
                image_query: String::new(),
            },
        ];
        // If a sample backdrop is present, also dump image-composited
        // variants so the scrim + Ken Burns can be eyeballed.
        let test_image = std::fs::read("/tmp/openai-img2.png")
            .ok()
            .and_then(|b| crate::imagegen::to_data_uri(&b).ok());
        for spec in specs {
            let name = format!("{:?}", spec.kind).to_lowercase();
            let scene = Scene {
                spec: spec.clone(),
                shown_at: Instant::now() - Duration::from_secs_f32(3.0),
                id: 0,
                image: None,
            };
            let state = PresenceState {
                mood: Mood::Speaking,
                t: 2.0,
                level: 0.45,
                peer: 0.0,
                level_history: &[],
                peer_history: &[],
                glitch: 0.0,
                vision_thumb: None,
                ambient: None,
            };
            let svg = scene_svg(&scene, &state);
            rasterize(&svg, &opt, &mut pixmap).expect("must rasterize");
            pixmap
                .save_png(format!("/tmp/eliza-{name}.png"))
                .expect("save png");
            if let Some(uri) = &test_image {
                let scene = Scene {
                    spec,
                    shown_at: Instant::now() - Duration::from_secs_f32(3.0),
                    id: 0,
                    image: Some((uri.clone(), Instant::now() - Duration::from_secs_f32(5.0))),
                };
                let state = PresenceState {
                mood: Mood::Speaking,
                t: 2.0,
                level: 0.45,
                peer: 0.0,
                level_history: &[],
                peer_history: &[],
                glitch: 0.0,
                vision_thumb: None,
                ambient: None,
            };
            let svg = scene_svg(&scene, &state);
                rasterize(&svg, &opt, &mut pixmap).expect("must rasterize");
                pixmap
                    .save_png(format!("/tmp/eliza-{name}-img.png"))
                    .expect("save png");
            }
        }
    }

    #[test]
    fn show_scene_caps_points_and_validates_accent() {
        let tile = VideoTile::new();
        let mut spec = sample_spec(SceneKind::KeyPoints);
        spec.points = (0..20).map(|i| i.to_string()).collect();
        spec.accent = "not-a-color".into();
        tile.show_scene(spec);
        let guard = tile.scene.lock().unwrap();
        let stored = guard.as_ref().unwrap();
        assert!(stored.spec.points.len() <= MAX_POINTS);
        assert_eq!(stored.spec.accent, DEFAULT_ACCENT);
    }
}
