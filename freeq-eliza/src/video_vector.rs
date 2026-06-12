//! Vector-character video backend — a rigged, hand-drawn face.
//!
//! Unlike the cyberpunk `Svg` presence (HUD/scene cards) and the particle
//! field, this is a *character*: a soft blob-headed being with big eyes
//! that blink and drift, expressive brows, and a mouth that lip-syncs to
//! her speech level. It keeps a stable "skin" identity; mood is carried by
//! a glow halo + cheeks + brow pose (idle, speaking, listening, thinking),
//! not by recolouring the character.
//!
//! It reuses the exact resvg + tiny_skia rasterizer the `Svg` backend
//! runs, so there are zero new dependencies. `face_svg` is a pure function
//! of `(t, level, peer)`, which lets the offline demo harness render the
//! same look without a live call.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iroh_live::media::format::VideoFrame;

use crate::video::{VIDEO_H, VIDEO_W, VideoTile};

const FPS: u64 = 15;

/// What she's doing — derived from audio + the thinking flag, same rule as
/// the other backends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mood {
    Idle,
    Speaking,
    Listening,
    Thinking,
}

impl Mood {
    fn derive(level: f32, peer: f32, thinking: bool) -> Mood {
        if level > 0.03 {
            Mood::Speaking
        } else if thinking {
            Mood::Thinking
        } else if peer > 0.03 {
            Mood::Listening
        } else {
            Mood::Idle
        }
    }
    /// Glow/accent colour for the mood.
    fn accent(self) -> &'static str {
        match self {
            Mood::Idle => "#6cb0ff",
            Mood::Speaking => "#ffd24a",
            Mood::Listening => "#3effd6",
            Mood::Thinking => "#b98cff",
        }
    }
}

/// Blink envelope: open (1.0) almost always; a quick ~140ms close every
/// 3.6s. Returns eye-openness in `[0.05, 1.0]`.
fn blink(t: f32) -> f32 {
    let phase = (t % 3.6) / 3.6;
    if phase > 0.96 {
        let p = (phase - 0.96) / 0.04;
        (1.0 - (p * std::f32::consts::PI).sin()).clamp(0.05, 1.0)
    } else {
        1.0
    }
}

/// Build the animated character as an SVG document (viewBox 640×360, slice
/// to the tile). Pure given `(t, level, peer, thinking)`.
pub fn face_svg(t: f32, level: f32, peer: f32) -> String {
    face_svg_with(t, level, peer, false)
}

pub fn face_svg_with(t: f32, level: f32, peer: f32, thinking: bool) -> String {
    let level = level.clamp(0.0, 1.0);
    let peer = peer.clamp(0.0, 1.0);
    let mood = Mood::derive(level, peer, thinking);
    let accent = mood.accent();

    // Centre + idle motion. Gentle bob and sway when not speaking; a faster
    // micro-bounce while talking gives her energy.
    let cx = 320.0;
    let base_cy = 182.0;
    let bob = if level > 0.03 {
        2.0 * (t * 9.0).sin() + 3.0 * (t * 1.2).sin()
    } else {
        4.0 * (t * 1.1).sin()
    };
    let cy = base_cy + bob;
    let sway = 2.2 * (t * 0.8).sin(); // degrees
    // Breathing squash/stretch (subtle), a touch more when speaking.
    let breath = 1.0 + 0.02 * (t * 1.5).sin() + 0.03 * level;
    let head_rx = 122.0;
    let head_ry = 132.0 * breath;

    let open = blink(t);
    // Gaze drift; look a bit toward the viewer when listening.
    let gx = 7.0 * (t * 0.7).sin();
    let gy = 4.0 * (t * 0.9).sin() - if mood == Mood::Listening { 3.0 } else { 0.0 };

    let eye_dy = -22.0;
    let eye_lx = cx - 42.0;
    let eye_rx_ = cx + 42.0;
    let eye_y = cy + eye_dy;
    let eye_rx = 27.0;
    let eye_ry = 31.0 * open;

    // Eye rendering: open → white sclera + drifting pupil + highlight;
    // nearly shut → a single lid line.
    let eye = |ex: f32| -> String {
        if open < 0.16 {
            format!(
                r##"<line x1="{x1:.1}" y1="{y:.1}" x2="{x2:.1}" y2="{y:.1}" stroke="#20242f" stroke-width="6" stroke-linecap="round"/>"##,
                x1 = ex - eye_rx,
                x2 = ex + eye_rx,
                y = eye_y,
            )
        } else {
            format!(
                r##"<ellipse cx="{ex:.1}" cy="{ey:.1}" rx="{rx:.1}" ry="{ry:.1}" fill="#f6f9ff"/>
<circle cx="{px:.1}" cy="{py:.1}" r="11" fill="#171b26"/>
<circle cx="{hx:.1}" cy="{hy:.1}" r="3.4" fill="#ffffff" opacity="0.9"/>"##,
                ex = ex,
                ey = eye_y,
                rx = eye_rx,
                ry = eye_ry,
                px = ex + gx,
                py = eye_y + gy,
                hx = ex + gx + 4.0,
                hy = eye_y + gy - 4.0,
            )
        }
    };

    // Brows — short rounded bars; pose by mood.
    let (brow_dy, brow_tilt) = match mood {
        Mood::Speaking => (-6.0, -8.0),  // raised, lively
        Mood::Thinking => (2.0, 14.0),   // furrowed inward
        Mood::Listening => (-2.0, -3.0), // attentive
        Mood::Idle => (0.0, 0.0),
    };
    let brow_y = eye_y - 30.0 + brow_dy;
    let brow = |ex: f32, sign: f32| -> String {
        // sign: -1 left, +1 right; tilt rotates the inner end up/down.
        format!(
            r##"<g transform="rotate({rot:.1} {ex:.1} {by:.1})"><rect x="{rx:.1}" y="{ry:.1}" width="40" height="9" rx="4.5" fill="#2a2030"/></g>"##,
            rot = sign * brow_tilt,
            ex = ex,
            by = brow_y,
            rx = ex - 20.0,
            ry = brow_y - 4.5,
        )
    };

    // Mouth — closed soft smile at rest; an open oval that grows with
    // speech level when talking.
    let mouth_y = cy + 64.0;
    let mouth = if level < 0.06 {
        format!(
            r##"<path d="M {x1:.1} {my:.1} Q {cxp:.1} {qy:.1} {x2:.1} {my:.1}" fill="none" stroke="#241019" stroke-width="7" stroke-linecap="round"/>"##,
            x1 = cx - 34.0,
            x2 = cx + 34.0,
            my = mouth_y,
            cxp = cx,
            qy = mouth_y + 16.0,
        )
    } else {
        let mry = 5.0 + 30.0 * level;
        let mrx = 30.0 + 8.0 * level;
        format!(
            r##"<ellipse cx="{cx:.1}" cy="{my:.1}" rx="{mrx:.1}" ry="{mry:.1}" fill="#1a0d14"/>
<ellipse cx="{cx:.1}" cy="{ty:.1}" rx="{trx:.1}" ry="{try_:.1}" fill="#e8607a" opacity="0.85"/>"##,
            cx = cx,
            my = mouth_y,
            mrx = mrx,
            mry = mry,
            ty = mouth_y + mry * 0.45,
            trx = mrx * 0.62,
            try_ = (mry * 0.5).max(2.0),
        )
    };

    // Cheeks bloom with mood (speaking/listening).
    let cheek_op = match mood {
        Mood::Speaking => 0.5,
        Mood::Listening => 0.4,
        _ => 0.18,
    };

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 640 360" preserveAspectRatio="xMidYMid slice">
<defs>
<radialGradient id="bg" cx="50%" cy="42%" r="75%">
<stop offset="0%" stop-color="#0d1018"/><stop offset="100%" stop-color="#05060a"/>
</radialGradient>
<radialGradient id="halo" cx="50%" cy="50%" r="50%">
<stop offset="0%" stop-color="{accent}" stop-opacity="0.55"/>
<stop offset="60%" stop-color="{accent}" stop-opacity="0.12"/>
<stop offset="100%" stop-color="{accent}" stop-opacity="0"/>
</radialGradient>
<linearGradient id="skin" x1="0" y1="0" x2="0" y2="1">
<stop offset="0%" stop-color="#8a93ff"/><stop offset="55%" stop-color="#6d7cff"/><stop offset="100%" stop-color="#3c3f7e"/>
</linearGradient>
</defs>
<rect width="640" height="360" fill="url(#bg)"/>
<ellipse cx="{cx:.1}" cy="{cy:.1}" rx="220" ry="200" fill="url(#halo)"/>
<g transform="rotate({sway:.2} {cx:.1} {cy:.1})">
<ellipse cx="{cx:.1}" cy="{cy:.1}" rx="{hrx:.1}" ry="{hry:.1}" fill="url(#skin)"/>
<ellipse cx="{cx:.1}" cy="{rim_y:.1}" rx="{hrx:.1}" ry="{hry:.1}" fill="{accent}" opacity="0.10"/>
<ellipse cx="{clx:.1}" cy="{cheek_y:.1}" rx="20" ry="13" fill="{accent}" opacity="{cheek_op:.2}"/>
<ellipse cx="{crx:.1}" cy="{cheek_y:.1}" rx="20" ry="13" fill="{accent}" opacity="{cheek_op:.2}"/>
{brow_l}{brow_r}
{eye_l}{eye_r}
{mouth}
</g>
</svg>"##,
        w = VIDEO_W,
        h = VIDEO_H,
        accent = accent,
        cx = cx,
        cy = cy,
        sway = sway,
        hrx = head_rx,
        hry = head_ry,
        rim_y = cy - 6.0,
        clx = cx - 64.0,
        crx = cx + 64.0,
        cheek_y = cy + 30.0,
        cheek_op = cheek_op,
        brow_l = brow(eye_lx, -1.0),
        brow_r = brow(eye_rx_, 1.0),
        eye_l = eye(eye_lx),
        eye_r = eye(eye_rx_),
        mouth = mouth,
    )
}

/// Rasterizes `face_svg` to RGBA frames. Holds the usvg options + pixmap
/// so the font db is loaded once.
pub struct VectorRenderer {
    opt: resvg::usvg::Options<'static>,
    pixmap: resvg::tiny_skia::Pixmap,
}

impl VectorRenderer {
    pub fn new() -> Option<Self> {
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        let pixmap = resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H)?;
        Some(Self { opt, pixmap })
    }

    /// Render one opaque RGBA frame (`VIDEO_W*VIDEO_H*4`).
    pub fn frame_rgba(&mut self, t: f32, level: f32, peer: f32) -> Vec<u8> {
        let svg = face_svg(t, level, peer);
        self.pixmap.fill(resvg::tiny_skia::Color::BLACK);
        if let Ok(tree) = resvg::usvg::Tree::from_str(&svg, &self.opt) {
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::identity(),
                &mut self.pixmap.as_mut(),
            );
        }
        self.pixmap.data().to_vec()
    }
}

/// Vector-backend render loop. Reads the tile's audio cells each frame,
/// rasterizes the character, publishes to `latest`, sleeps to FPS.
pub(crate) fn render_loop(tile: VideoTile) {
    let mut renderer = match VectorRenderer::new() {
        Some(r) => r,
        None => {
            tracing::error!("vector video: could not allocate pixmap");
            return;
        }
    };
    let frame_dt = Duration::from_millis(1000 / FPS);
    let started = Instant::now();
    tracing::info!("eliza vector renderer started ({VIDEO_W}x{VIDEO_H} @ {FPS}fps)");

    while tile.running.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let t = started.elapsed().as_secs_f32();
        let level = f32::from_bits(tile.level.load(Ordering::Relaxed));
        let peer = f32::from_bits(tile.peer_level.load(Ordering::Relaxed));
        let thinking = tile.thinking.load(Ordering::Relaxed);

        let svg = face_svg_with(t, level, peer, thinking);
        renderer.pixmap.fill(resvg::tiny_skia::Color::BLACK);
        if let Ok(tree) = resvg::usvg::Tree::from_str(&svg, &renderer.opt) {
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::identity(),
                &mut renderer.pixmap.as_mut(),
            );
        }
        let frame = VideoFrame::new_rgba(
            bytes::Bytes::copy_from_slice(renderer.pixmap.data()),
            VIDEO_W,
            VIDEO_H,
            Duration::ZERO,
        );
        if let Ok(mut g) = tile.latest.lock() {
            *g = Some(frame);
        }

        if let Some(rest) = frame_dt.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    tracing::info!("eliza vector renderer stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_parses_in_every_state() {
        let opt = resvg::usvg::Options::default();
        for &(l, p) in &[(0.0, 0.0), (0.8, 0.0), (0.0, 0.5)] {
            for &th in &[false, true] {
                let svg = face_svg_with(0.4, l, p, th);
                resvg::usvg::Tree::from_str(&svg, &opt)
                    .unwrap_or_else(|e| panic!("svg parse failed (l={l} p={p} th={th}): {e}"));
            }
        }
    }

    #[test]
    fn frame_is_right_size_and_lights_up() {
        let mut r = VectorRenderer::new().expect("renderer");
        let f = r.frame_rgba(0.4, 0.8, 0.0);
        assert_eq!(f.len(), (VIDEO_W * VIDEO_H * 4) as usize);
        let lit = f.chunks_exact(4).filter(|p| p[0] as u16 + p[1] as u16 + p[2] as u16 > 40).count();
        assert!(lit > 1000, "character should fill a chunk of the frame");
    }
}
