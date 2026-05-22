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

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use iroh_live::media::format::{PixelFormat, VideoFormat, VideoFrame};
use iroh_live::media::traits::VideoSource;

/// Tile resolution. 360p is ample and cheap to rasterize on the CPU.
pub const VIDEO_W: u32 = 640;
pub const VIDEO_H: u32 = 360;
const FPS: u64 = 15;
/// Most points a scene shows (extras are dropped).
const MAX_POINTS: usize = 6;
/// How long a scene stays on the tile after it appears before the tile
/// returns to the presence orb.
const SCENE_HOLD: Duration = Duration::from_secs(28);
/// Extra time a scene stays up once its backdrop image arrives — image
/// generation is slow, so a late image still gets airtime.
const IMAGE_HOLD: Duration = Duration::from_secs(22);
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

/// What eliza is doing — read off the audio + a "thinking" flag and
/// shown by the presence.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mood {
    Idle,
    Listening,
    Thinking,
    Speaking,
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

/// Shared handle to eliza's video tile. Clone-cheap.
#[derive(Clone)]
pub struct VideoTile {
    latest: Arc<Mutex<Option<VideoFrame>>>,
    /// eliza's own speech loudness, `f32` bits in `[0,1]`.
    level: Arc<AtomicU32>,
    /// Loudest participant's loudness — drives the "listening" mood.
    peer_level: Arc<AtomicU32>,
    /// Set while an LLM call is in flight — drives the "thinking" mood.
    thinking: Arc<AtomicBool>,
    scene: Arc<Mutex<Option<Scene>>>,
    /// Hands out a fresh id per scene so async image jobs can target one.
    next_id: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
}

impl VideoTile {
    pub fn new() -> Self {
        Self {
            latest: Arc::new(Mutex::new(None)),
            level: Arc::new(AtomicU32::new(0)),
            peer_level: Arc::new(AtomicU32::new(0)),
            thinking: Arc::new(AtomicBool::new(false)),
            scene: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(0)),
            running: Arc::new(AtomicBool::new(true)),
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
        std::thread::Builder::new()
            .name("eliza-video".into())
            .spawn(move || tile.render_loop())
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

        while self.running.load(Ordering::Relaxed) {
            let tick = Instant::now();
            let t = started.elapsed().as_secs_f32();
            let level = f32::from_bits(self.level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            let peer = f32::from_bits(self.peer_level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            let thinking = self.thinking.load(Ordering::Relaxed);
            let mood = if level > 0.03 {
                Mood::Speaking
            } else if thinking {
                Mood::Thinking
            } else if peer > 0.03 {
                Mood::Listening
            } else {
                Mood::Idle
            };

            let svg = {
                let guard = self.scene.lock().expect("scene lock");
                match guard.as_ref() {
                    // Show the scene while it's within its hold window;
                    // then the tile returns to the presence orb.
                    Some(scene) if scene.is_visible() => scene_svg(scene, t, level, mood),
                    _ => presence_svg(mood, t, level, peer),
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

/// Per-mood accent colour for the presence + the corner status dot.
fn mood_color(mood: Mood) -> &'static str {
    match mood {
        Mood::Idle => "#6cb0ff",
        Mood::Listening => "#54e2c8",
        Mood::Thinking => "#b594ff",
        Mood::Speaking => "#9fd2ff",
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

/// The state-aware presence: a glowing orb whose colour and motion say
/// what eliza is doing — idle, listening, thinking, or speaking.
fn presence_svg(mood: Mood, t: f32, level: f32, peer: f32) -> String {
    let accent = mood_color(mood);
    let breathe = (t * 1.6).sin() * 5.0;
    let orb_r = match mood {
        Mood::Speaking => 48.0 + breathe + level * 64.0,
        Mood::Thinking => 44.0 + (t * 4.0).sin() * 4.0,
        Mood::Listening => 46.0 + breathe + peer * 30.0,
        Mood::Idle => 44.0 + breathe,
    };
    let glow_r = orb_r * 1.95;
    let glow_op = match mood {
        Mood::Speaking => 0.14 + level * 0.4,
        Mood::Thinking => 0.18 + (t * 4.0).sin().abs() * 0.12,
        Mood::Listening => 0.16 + peer * 0.3,
        Mood::Idle => 0.12,
    };

    // Mood-specific overlay: a rotating dashed ring while thinking,
    // contracting ripples while listening, a steady ring otherwise.
    let overlay = match mood {
        Mood::Thinking => format!(
            r##"<circle cx="320" cy="156" r="{r:.1}" fill="none" stroke="{accent}" stroke-width="3" stroke-dasharray="14 12" opacity="0.8" transform="rotate({deg:.1} 320 156)"/>"##,
            r = orb_r + 26.0,
            deg = t * 150.0,
        ),
        Mood::Listening => {
            let mut rings = String::new();
            for i in 0..3 {
                let phase = (t * 0.6 + i as f32 * 0.33).fract();
                let rr = orb_r + 8.0 + phase * 64.0;
                let op = (1.0 - phase) * 0.5;
                rings.push_str(&format!(
                    r##"<circle cx="320" cy="156" r="{rr:.1}" fill="none" stroke="{accent}" stroke-width="2" opacity="{op:.3}"/>"##,
                ));
            }
            rings
        }
        _ => format!(
            r##"<circle cx="320" cy="156" r="{r:.1}" fill="none" stroke="{accent}" stroke-width="1.5" opacity="0.3"/>"##,
            r = orb_r + 22.0 + (t * 2.0).sin() * 3.0,
        ),
    };

    let label = match mood {
        Mood::Idle => "eliza",
        Mood::Listening => "listening",
        Mood::Thinking => "thinking",
        Mood::Speaking => "eliza",
    };

    // Face — blinking eyes and a mouth whose openness tracks `level`
    // (eliza's own speech loudness), so she visibly lip-syncs as she
    // talks. At rest `level` is ~0, leaving a calm closed mouth.
    let blinking = (t % 4.3) < 0.13;
    let eye_r = orb_r * 0.115;
    let eye_ry = eye_r * if blinking { 0.12 } else { 1.0 };
    let eye_y = 156.0 - orb_r * 0.20;
    let eye_dx = orb_r * 0.34;
    let mouth_cy = 156.0 + orb_r * 0.36;
    let mouth_rx = orb_r * 0.27;
    let mouth_ry = 2.0 + level.clamp(0.0, 1.0) * orb_r * 0.42;
    let face = format!(
        r##"<g fill="#0a1020">
  <ellipse cx="{lx:.1}" cy="{eye_y:.1}" rx="{eye_r:.1}" ry="{eye_ry:.1}"/>
  <ellipse cx="{rx:.1}" cy="{eye_y:.1}" rx="{eye_r:.1}" ry="{eye_ry:.1}"/>
  <ellipse cx="320" cy="{mouth_cy:.1}" rx="{mouth_rx:.1}" ry="{mouth_ry:.1}"/>
</g>"##,
        lx = 320.0 - eye_dx,
        rx = 320.0 + eye_dx,
    );

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
  <defs>
    <radialGradient id="bg" cx="50%" cy="40%" r="80%">
      <stop offset="0%" stop-color="#16213f"/>
      <stop offset="100%" stop-color="#05070f"/>
    </radialGradient>
    <radialGradient id="orb" cx="42%" cy="38%" r="70%">
      <stop offset="0%" stop-color="#f2f7ff"/>
      <stop offset="44%" stop-color="{accent}"/>
      <stop offset="100%" stop-color="#16306a"/>
    </radialGradient>
  </defs>
  <rect width="{w}" height="{h}" fill="url(#bg)"/>
  <circle cx="320" cy="156" r="{glow_r:.1}" fill="{accent}" opacity="{glow_op:.3}"/>
  {overlay}
  <circle cx="320" cy="156" r="{orb_r:.1}" fill="url(#orb)"/>
  {face}
  <text x="320" y="318" font-family="{FONT}" font-size="26" font-weight="600" fill="#cfe0ff" text-anchor="middle" letter-spacing="6">{label}</text>
</svg>"##,
        w = VIDEO_W,
        h = VIDEO_H,
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

/// The persistent header: a small "ELIZA" wordmark and a live mood dot.
fn header(accent: &str, mood: Mood, t: f32, level: f32) -> String {
    let dot = mood_color(mood);
    let r = 4.2 + level * 5.0 + (t * 2.1).sin().abs();
    format!(
        r##"<text x="46" y="40" font-family="{FONT}" font-size="12.5" font-weight="700" letter-spacing="5" fill="{accent}" opacity="0.82">ELIZA</text>
<circle cx="591" cy="35" r="{ro:.1}" fill="none" stroke="{dot}" stroke-width="1" opacity="0.4"/>
<circle cx="591" cy="35" r="{r:.1}" fill="{dot}"/>"##,
        ro = r + 5.0,
    )
}

/// Wrap a scene body in the full SVG document with defs, backdrop and
/// header. `image` is an optional backdrop: `(data-uri, age-seconds)`.
fn frame(
    accent: &str,
    t: f32,
    mood: Mood,
    level: f32,
    image: Option<(&str, f32)>,
    body: &str,
) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360" viewBox="0 0 640 360">
{defs}
{backdrop}
{header}
{body}
</svg>"##,
        defs = defs(accent),
        backdrop = backdrop(accent, t, image),
        header = header(accent, mood, t, level),
    )
}

/// Render the current scene — dispatches on [`SceneKind`].
fn scene_svg(scene: &Scene, t: f32, level: f32, mood: Mood) -> String {
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
    frame(accent, t, mood, level, image, &body)
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
        for mood in [Mood::Idle, Mood::Listening, Mood::Thinking, Mood::Speaking] {
            let frame = rasterize(&presence_svg(mood, 1.7, 0.5, 0.4), &opt, &mut pixmap)
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
                let svg = scene_svg(&scene, 1.5, 0.4, Mood::Speaking);
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
            let svg = scene_svg(&scene, 2.0, 0.45, Mood::Speaking);
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
                let svg = scene_svg(&scene, 2.0, 0.45, Mood::Speaking);
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
