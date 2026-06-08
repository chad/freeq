//! Belligerent cartoon character — a South Park-flavoured construction-
//! paper kid who plays an angry, loudmouth being. Flat fills, thick black
//! outlines, a beanie with earflaps, conjoined oval eyes, and a mouth that
//! flies open into a full scream as her speech level rises.
//!
//! Unlike the friendly `Vector` character, the mood mapping *leans angry*:
//! speaking = YELLING (wide mouth, furrowed brows, reddening face, rage
//! tremor + anger vein when loud); listening = suspicious squint; idle =
//! grumpy frown. Same resvg+tiny_skia path as the other SVG backends — no
//! new deps. `face_svg` is pure for the offline demo harness.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iroh_live::media::format::VideoFrame;

use crate::video::{VIDEO_H, VIDEO_W, VideoTile};

const FPS: u64 = 15;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mood {
    Idle,
    Yelling,
    Suspicious,
    Scheming,
}

impl Mood {
    fn derive(level: f32, peer: f32, thinking: bool) -> Mood {
        if level > 0.03 {
            Mood::Yelling
        } else if thinking {
            Mood::Scheming
        } else if peer > 0.03 {
            Mood::Suspicious
        } else {
            Mood::Idle
        }
    }
}

fn blink(t: f32) -> f32 {
    // Angry characters don't blink much — long gaps.
    let phase = (t % 5.0) / 5.0;
    if phase > 0.975 {
        let p = (phase - 0.975) / 0.025;
        (1.0 - (p * std::f32::consts::PI).sin()).clamp(0.08, 1.0)
    } else {
        1.0
    }
}

/// Cheap deterministic noise in [-1,1] from a float seed (for rage tremor).
fn jitter(seed: f32) -> f32 {
    let x = (seed * 127.1).sin() * 43758.547;
    (x - x.floor()) * 2.0 - 1.0
}

pub fn face_svg(t: f32, level: f32, peer: f32) -> String {
    face_svg_with(t, level, peer, false)
}

pub fn face_svg_with(t: f32, level: f32, peer: f32, thinking: bool) -> String {
    let level = level.clamp(0.0, 1.0);
    let peer = peer.clamp(0.0, 1.0);
    let mood = Mood::derive(level, peer, thinking);
    let rage = level;

    let cx = 320.0;
    let cy = 202.0;

    // Rage tremor — the whole kid shakes when yelling.
    let shake = if rage > 0.08 { 5.0 * rage } else { 0.0 };
    let dx = jitter(t * 37.0) * shake;
    let dy = jitter(t * 41.0) * shake;

    let open = blink(t);
    // Squint: narrows the eyes when suspicious/scheming/grumpy; wide open
    // when screaming.
    let squint = match mood {
        Mood::Yelling => 0.0,
        Mood::Suspicious => 0.55,
        Mood::Scheming => 0.6,
        Mood::Idle => 0.2,
    };

    // ── Eyes (conjoined SP-style ovals) ──────────────────────────────
    let eye_y = cy - 14.0;
    let eye_rx = 37.0;
    let eye_ry = 44.0 * open;
    let lx = cx - 30.0;
    let rx = cx + 30.0;
    // Pupils crowd toward the centre (angry focus); drift aside when
    // suspicious.
    let look = match mood {
        Mood::Suspicious => 16.0,
        Mood::Scheming => -14.0,
        _ => 0.0,
    };
    let pup = |ex: f32, inward: f32| {
        format!(
            r##"<circle cx="{px:.1}" cy="{py:.1}" r="8.5" fill="#101010"/>"##,
            px = ex + inward + look,
            py = eye_y + 8.0,
        )
    };
    // Squint lids — skin-coloured caps over the eye top & bottom.
    let lid = |ex: f32| {
        if squint <= 0.01 {
            String::new()
        } else {
            let h = eye_ry * squint;
            format!(
                r##"<rect x="{x:.1}" y="{ty:.1}" width="{w:.1}" height="{h:.1}" fill="#f1c79c"/>
<rect x="{x:.1}" y="{by:.1}" width="{w:.1}" height="{h:.1}" fill="#f1c79c"/>"##,
                x = ex - eye_rx - 1.0,
                ty = eye_y - eye_ry - 1.0,
                by = eye_y + eye_ry - h + 1.0,
                w = eye_rx * 2.0 + 2.0,
                h = h,
            )
        }
    };

    // ── Brows (thick, angled down-inward; steeper with rage) ─────────
    let brow_y = eye_y - 46.0;
    let brow_angle = 14.0 + 20.0 * rage;
    let brow = |ex: f32, sign: f32| {
        format!(
            r##"<g transform="rotate({rot:.1} {ex:.1} {by:.1})"><rect x="{rx:.1}" y="{ry:.1}" width="64" height="15" rx="6" fill="#241914"/></g>"##,
            rot = sign * brow_angle,
            ex = ex,
            by = brow_y,
            rx = ex - 32.0,
            ry = brow_y - 7.5,
        )
    };

    // ── Mouth: grumpy frown at rest → full scream when loud ──────────
    let mouth_y = cy + 56.0;
    let mouth = if level < 0.08 {
        // frown (bows up in the middle)
        format!(
            r##"<path d="M {x1:.1} {my:.1} Q {cxp:.1} {qy:.1} {x2:.1} {my:.1}" fill="none" stroke="#2a1410" stroke-width="8" stroke-linecap="round"/>"##,
            x1 = cx - 46.0,
            x2 = cx + 46.0,
            my = mouth_y + 6.0,
            cxp = cx,
            qy = mouth_y - 16.0,
        )
    } else {
        let mrx = 40.0 + 18.0 * level;
        let mry = 10.0 + 58.0 * level;
        format!(
            r##"<ellipse cx="{cx:.1}" cy="{my:.1}" rx="{mrx:.1}" ry="{mry:.1}" fill="#1a0c0c" stroke="#2a1410" stroke-width="4"/>
<rect x="{tx:.1}" y="{tty:.1}" width="{tw:.1}" height="11" rx="3" fill="#fbfbf5"/>
<ellipse cx="{cx:.1}" cy="{tongue_y:.1}" rx="{trx:.1}" ry="{try_:.1}" fill="#e0556a"/>"##,
            cx = cx,
            my = mouth_y + mry * 0.4,
            mrx = mrx,
            mry = mry,
            tx = cx - mrx * 0.7,
            tty = mouth_y - mry * 0.55 + mry * 0.4,
            tw = mrx * 1.4,
            tongue_y = mouth_y + mry * 0.75,
            trx = mrx * 0.6,
            try_ = (mry * 0.35).max(3.0),
        )
    };

    // Reddening face + anger vein when screaming.
    let red_op = (rage * 0.5).min(0.5);
    let vein = if rage > 0.45 {
        let vo = ((rage - 0.45) / 0.55).clamp(0.0, 1.0);
        format!(
            r##"<g transform="translate({vx:.1} {vy:.1})" opacity="{vo:.2}" stroke="#e23b2e" stroke-width="6" stroke-linecap="round" fill="none">
<path d="M -14 0 L 0 -12 L 14 0"/><path d="M -10 8 L 0 -2 L 10 8"/></g>"##,
            vx = cx + 92.0,
            vy = cy - 78.0,
            vo = vo,
        )
    } else {
        String::new()
    };

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 640 360" preserveAspectRatio="xMidYMid slice">
<defs><linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">
<stop offset="0%" stop-color="#7ec7df"/><stop offset="62%" stop-color="#9bd0e2"/>
<stop offset="62%" stop-color="#9cc45e"/><stop offset="100%" stop-color="#7fae47"/></linearGradient></defs>
<rect width="640" height="360" fill="url(#sky)"/>
<g transform="translate({dx:.2} {dy:.2})">
<!-- coat / shoulders -->
<path d="M {coat_l:.1} 360 L {coat_l:.1} 326 Q 320 300 {coat_r:.1} 326 L {coat_r:.1} 360 Z" fill="#c0392b" stroke="#2a1410" stroke-width="4"/>
<!-- earflaps -->
<circle cx="{ear_l:.1}" cy="{ear_y:.1}" r="27" fill="#d6453f" stroke="#2a1410" stroke-width="4"/>
<circle cx="{ear_r:.1}" cy="{ear_y:.1}" r="27" fill="#d6453f" stroke="#2a1410" stroke-width="4"/>
<!-- head -->
<ellipse cx="{cx:.1}" cy="{cy:.1}" rx="128" ry="120" fill="#f1c79c" stroke="#2a1410" stroke-width="4"/>
<!-- rage flush -->
<ellipse cx="{cx:.1}" cy="{flush_y:.1}" rx="128" ry="120" fill="#e2392b" opacity="{red_op:.2}"/>
<!-- hat dome + brim -->
<path d="M {hat_l:.1} {hat_b:.1} Q {cx:.1} {hat_t:.1} {hat_r:.1} {hat_b:.1} Z" fill="#d6453f" stroke="#2a1410" stroke-width="4"/>
<rect x="{brim_x:.1}" y="{brim_y:.1}" width="{brim_w:.1}" height="30" rx="15" fill="#f2c14e" stroke="#2a1410" stroke-width="4"/>
{brow_l}{brow_r}
<ellipse cx="{lx:.1}" cy="{eye_y:.1}" rx="{eye_rx:.1}" ry="{eye_ry:.1}" fill="#fbfbf5" stroke="#2a1410" stroke-width="3"/>
<ellipse cx="{rx:.1}" cy="{eye_y:.1}" rx="{eye_rx:.1}" ry="{eye_ry:.1}" fill="#fbfbf5" stroke="#2a1410" stroke-width="3"/>
{pup_l}{pup_r}
{lid_l}{lid_r}
{mouth}
{vein}
</g>
</svg>"##,
        w = VIDEO_W,
        h = VIDEO_H,
        dx = dx,
        dy = dy,
        cx = cx,
        cy = cy,
        flush_y = cy + 6.0,
        red_op = red_op,
        coat_l = cx - 150.0,
        coat_r = cx + 150.0,
        ear_l = cx - 120.0,
        ear_r = cx + 120.0,
        ear_y = cy - 40.0,
        hat_l = cx - 132.0,
        hat_r = cx + 132.0,
        hat_b = cy - 96.0,
        hat_t = cy - 230.0,
        brim_x = cx - 138.0,
        brim_y = cy - 110.0,
        brim_w = 276.0,
        brow_l = brow(lx, 1.0),
        brow_r = brow(rx, -1.0),
        lx = lx,
        rx = rx,
        eye_y = eye_y,
        eye_rx = eye_rx,
        eye_ry = eye_ry,
        pup_l = pup(lx, 12.0),
        pup_r = pup(rx, -12.0),
        lid_l = lid(lx),
        lid_r = lid(rx),
        mouth = mouth,
        vein = vein,
    )
}

pub struct SouthParkRenderer {
    opt: resvg::usvg::Options<'static>,
    pixmap: resvg::tiny_skia::Pixmap,
}

impl SouthParkRenderer {
    pub fn new() -> Option<Self> {
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        let pixmap = resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H)?;
        Some(Self { opt, pixmap })
    }

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

pub(crate) fn render_loop(tile: VideoTile) {
    let mut renderer = match SouthParkRenderer::new() {
        Some(r) => r,
        None => {
            tracing::error!("southpark video: could not allocate pixmap");
            return;
        }
    };
    let frame_dt = Duration::from_millis(1000 / FPS);
    let started = Instant::now();
    tracing::info!("eliza southpark renderer started ({VIDEO_W}x{VIDEO_H} @ {FPS}fps)");

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
    tracing::info!("eliza southpark renderer stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_parses_in_every_state() {
        let opt = resvg::usvg::Options::default();
        for &(l, p) in &[(0.0, 0.0), (0.9, 0.0), (0.0, 0.5)] {
            for &th in &[false, true] {
                let svg = face_svg_with(0.3, l, p, th);
                resvg::usvg::Tree::from_str(&svg, &opt)
                    .unwrap_or_else(|e| panic!("svg parse failed (l={l} p={p} th={th}): {e}"));
            }
        }
    }

    #[test]
    fn frame_is_right_size() {
        let mut r = SouthParkRenderer::new().expect("renderer");
        let f = r.frame_rgba(0.3, 0.9, 0.0);
        assert_eq!(f.len(), (VIDEO_W * VIDEO_H * 4) as usize);
    }
}
