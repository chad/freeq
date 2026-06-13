//! Alexandria — an ancient bronze coin that thinks.
//!
//! The being is a coin: the Pharos lighthouse of Alexandria embossed in
//! bronze relief, circuit traces etched into the patina, cyan LEDs and
//! amber jewels set around a beaded rim. Its inner life is told as
//! *light moving through the metal*, with an unambiguous direction for
//! each state:
//!
//! * **hearing**  — cyan pulses flow INWARD along the traces, toward the
//!   tower; the rim LEDs swell with the speaker's loudness.
//! * **thinking** — amber pulses CIRCULATE; the rim jewels chase each
//!   other around the coin (a clockwork spinner); the tower windows
//!   flicker like memory access.
//! * **speaking** — the beacon FLARES with rotating rays; pulses flow
//!   OUTWARD; the tower windows fill bottom-up with her speech level
//!   (a VU meter built into the architecture).
//! * **idle**     — everything dims; the beacon breathes slowly; a
//!   single lazy pulse wanders the traces so the coin never reads dead.
//!
//! States don't snap — [`AlexandriaRenderer`] keeps smoothed envelopes
//! (fast attack, slower release) and [`coin_svg`] blends every state's
//! light by its envelope weight, so transitions crossfade.
//!
//! Same machinery as the other faces: pure SVG re-rendered per frame,
//! rasterized with resvg. Single-layer; scene/board/HUD overlays NO-OP.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iroh_live::media::format::VideoFrame;

use crate::video::{VIDEO_H, VIDEO_W, VideoTile};

const FPS: u64 = 15;

/// Coin centre + radius in the 640×360 design space.
const CX: f32 = 320.0;
const CY: f32 = 180.0;
const COIN_R: f32 = 150.0;

/// Smoothed per-state intensities in `[0,1]` — the renderer's whole
/// input. `speak`/`hear`/`think` crossfade (attack ≈ 120 ms, release
/// ≈ 450 ms); `level`/`peer` are lightly smoothed loudness.
#[derive(Clone, Copy, Debug, Default)]
pub struct Env {
    pub speak: f32,
    pub hear: f32,
    pub think: f32,
    pub level: f32,
    pub peer: f32,
}

/// One asymmetric-EMA step: rises with `tau_up`, falls with `tau_down`.
fn smooth(cur: &mut f32, target: f32, dt: f32, tau_up: f32, tau_down: f32) {
    let tau = if target > *cur { tau_up } else { tau_down };
    let k = 1.0 - (-dt / tau).exp();
    *cur += (target - *cur) * k;
}

impl Env {
    /// Advance the envelopes one frame toward what the raw signals say.
    /// Priority mirrors the other backends: speaking > thinking >
    /// hearing — only one state *targets* 1.0, but during transitions
    /// several envelopes are non-zero and the visuals blend.
    pub fn step(&mut self, level: f32, peer: f32, thinking: bool, dt: f32) {
        let level = level.clamp(0.0, 1.0);
        let peer = peer.clamp(0.0, 1.0);
        let speaking = level > 0.03;
        let hearing = peer > 0.03 && !speaking && !thinking;
        let think = thinking && !speaking;
        smooth(
            &mut self.speak,
            if speaking { 1.0 } else { 0.0 },
            dt,
            0.10,
            0.45,
        );
        smooth(
            &mut self.think,
            if think { 1.0 } else { 0.0 },
            dt,
            0.14,
            0.50,
        );
        smooth(
            &mut self.hear,
            if hearing { 1.0 } else { 0.0 },
            dt,
            0.12,
            0.45,
        );
        smooth(&mut self.level, level, dt, 0.05, 0.22);
        smooth(&mut self.peer, peer, dt, 0.06, 0.30);
    }

    /// How idle the coin is — 1.0 when no state light is up.
    fn idle(&self) -> f32 {
        (1.0 - self.speak.max(self.hear).max(self.think)).clamp(0.0, 1.0)
    }
}

// ── Static geometry ─────────────────────────────────────────────────

/// Right-half circuit traces: tower-edge → bends → LED pad. The left
/// half is the mirror (`x → 640 − x`). Authored against the design grid.
const TRACES_R: &[&[(f32, f32)]] = &[
    &[
        (352.0, 140.0),
        (390.0, 140.0),
        (405.0, 125.0),
        (430.0, 125.0),
    ],
    &[
        (350.0, 160.0),
        (400.0, 160.0),
        (415.0, 175.0),
        (445.0, 175.0),
    ],
    &[
        (352.0, 188.0),
        (385.0, 188.0),
        (400.0, 203.0),
        (433.0, 203.0),
    ],
    &[
        (340.0, 118.0),
        (370.0, 118.0),
        (385.0, 103.0),
        (414.0, 103.0),
    ],
    &[(332.0, 98.0), (355.0, 98.0), (370.0, 83.0), (396.0, 83.0)],
    &[
        (368.0, 232.0),
        (398.0, 232.0),
        (410.0, 244.0),
        (436.0, 244.0),
    ],
];

/// A trace polyline (owned, mirrored as needed) + its total length.
fn all_traces() -> Vec<Vec<(f32, f32)>> {
    let mut out: Vec<Vec<(f32, f32)>> = Vec::with_capacity(TRACES_R.len() * 2);
    for t in TRACES_R {
        out.push(t.to_vec());
        out.push(t.iter().map(|&(x, y)| (640.0 - x, y)).collect());
    }
    out
}

/// Interpolated point at parameter `p ∈ [0,1]` along a polyline
/// (0 = tower end, 1 = LED pad end).
fn point_along(pts: &[(f32, f32)], p: f32) -> (f32, f32) {
    let p = p.clamp(0.0, 1.0);
    let mut segs = Vec::with_capacity(pts.len().saturating_sub(1));
    let mut total = 0.0;
    for w in pts.windows(2) {
        let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        segs.push(d);
        total += d;
    }
    if total <= f32::EPSILON {
        return pts[0];
    }
    let mut want = p * total;
    for (i, d) in segs.iter().enumerate() {
        if want <= *d {
            let f = want / d;
            return (
                pts[i].0 + (pts[i + 1].0 - pts[i].0) * f,
                pts[i].1 + (pts[i + 1].1 - pts[i].1) * f,
            );
        }
        want -= d;
    }
    *pts.last().unwrap()
}

/// Tower windows as (x, y, row) — row 0 is the lowest (VU fills upward).
fn windows() -> Vec<(f32, f32, u32)> {
    let mut w = Vec::new();
    // Mid tier: 4 cols × 5 rows.
    for (r, y) in [236.0, 222.0, 208.0, 194.0, 180.0].iter().enumerate() {
        for x in [298.0, 310.0, 322.0, 334.0] {
            w.push((x, *y, r as u32));
        }
    }
    // Top tier: 2 cols × 2 rows.
    for (r, y) in [142.0, 128.0].iter().enumerate() {
        for x in [310.0, 324.0] {
            w.push((x, *y, (r + 5) as u32));
        }
    }
    w
}
/// Highest window row index + 1 (for VU normalization).
const WINDOW_ROWS: f32 = 7.0;

// ── The coin ────────────────────────────────────────────────────────

/// Render the coin as an SVG document. Pure in `(t, e)` so the offline
/// preview harness and tests can drive it without a live call.
pub fn coin_svg(t: f32, e: &Env) -> String {
    let idle = e.idle();
    let traces = all_traces();

    // Whole-coin micro-motion: a slow wobble + metal breath. Enough to
    // read "alive at rest" without ever being busy.
    let wob = 0.8 * (t * 0.5).sin();
    let breath = 1.0 + 0.004 * (t * 0.8).sin();

    // ── Beaded rim + jewels + patina (static positions) ──
    let mut beads = String::new();
    for i in 0..48 {
        let a = i as f32 / 48.0 * std::f32::consts::TAU;
        beads.push_str(&format!(
            r##"<circle cx="{x:.1}" cy="{y:.1}" r="3" fill="#5d3d24" stroke="#2a180c" stroke-width="0.6"/>"##,
            x = CX + a.cos() * 141.0,
            y = CY + a.sin() * 141.0,
        ));
    }
    // Amber jewels — the thinking state's clockwork chase runs here.
    let mut jewels = String::new();
    for i in 0..8 {
        let a = (i as f32 / 8.0) * std::f32::consts::TAU + 0.39; // offset clears the flame
        let chase = ((a - t * 2.6).cos()).max(0.0).powi(3);
        let twinkle = 0.10 * ((t * 0.9 + i as f32 * 1.7).sin() * 0.5 + 0.5);
        let bright = (0.40 + twinkle + 0.60 * chase * e.think).clamp(0.0, 1.0);
        let x = CX + a.cos() * 120.0;
        let y = CY + a.sin() * 120.0;
        jewels.push_str(&format!(
            r##"<circle cx="{x:.1}" cy="{y:.1}" r="{gr:.1}" fill="#ffb52e" opacity="{gop:.3}"/>
<circle cx="{x:.1}" cy="{y:.1}" r="6" fill="url(#jewel)" opacity="{op:.3}"/>
<circle cx="{hx:.1}" cy="{hy:.1}" r="1.6" fill="#fff6d8" opacity="{hop:.3}"/>"##,
            gr = 9.0 + 5.0 * chase * e.think,
            gop = 0.10 + 0.45 * chase * e.think,
            op = 0.55 + 0.45 * bright,
            hx = x - 1.5,
            hy = y - 1.5,
            hop = 0.5 + 0.5 * bright,
        ));
    }
    // Patina blotches + hairline cracks — antiquity, static.
    let patina = r##"<ellipse cx="236" cy="106" rx="30" ry="18" fill="#4a7a68" opacity="0.13"/>
<ellipse cx="412" cy="252" rx="36" ry="22" fill="#4a7a68" opacity="0.11"/>
<ellipse cx="268" cy="268" rx="24" ry="14" fill="#56806d" opacity="0.10"/>
<ellipse cx="402" cy="96" rx="20" ry="12" fill="#4a7a68" opacity="0.12"/>
<path d="M 180 158 q 24 10 38 34" fill="none" stroke="#241308" stroke-width="1.2" opacity="0.5"/>
<path d="M 462 218 q -18 14 -22 36" fill="none" stroke="#241308" stroke-width="1.0" opacity="0.45"/>"##;

    // ── Circuit traces: groove + faint live wire + state pulses ──
    let mut trace_svg = String::new();
    let wire_glow = (0.20 + 0.25 * e.hear + 0.20 * e.think + 0.25 * e.speak).min(0.6);
    for pts in &traces {
        let d: String = pts
            .iter()
            .enumerate()
            .map(|(i, (x, y))| format!("{}{x:.1} {y:.1}", if i == 0 { "M " } else { " L " }))
            .collect();
        trace_svg.push_str(&format!(
            r##"<path d="{d}" fill="none" stroke="#1f1209" stroke-width="4" stroke-linejoin="round"/>
<path d="{d}" fill="none" stroke="#54e6d2" stroke-width="1.4" stroke-linejoin="round" opacity="{wire_glow:.3}"/>"##,
        ));
    }
    // LED pads at the rim ends of every trace.
    let mut leds = String::new();
    for (i, pts) in traces.iter().enumerate() {
        let (x, y) = *pts.last().unwrap();
        let twinkle = 0.12 * ((t * 0.9 + i as f32 * 1.3).sin() * 0.5 + 0.5);
        let b = (0.30
            + twinkle
            + e.hear * 0.65 * (0.35 + 0.65 * e.peer)
            + e.speak * 0.55 * (0.30 + 0.70 * e.level))
            .clamp(0.0, 1.0);
        leds.push_str(&format!(
            r##"<circle cx="{x:.1}" cy="{y:.1}" r="{gr:.1}" fill="#39e8ff" opacity="{gop:.3}"/>
<circle cx="{x:.1}" cy="{y:.1}" r="3.4" fill="#bdf6ff" opacity="{op:.3}"/>"##,
            gr = 6.0 + 4.0 * b,
            gop = 0.10 + 0.35 * b,
            op = 0.35 + 0.65 * b,
        ));
    }

    // ── State pulses traveling the traces ──
    // Drawn per state, weighted by its envelope, so a transition shows
    // both light patterns mid-fade instead of a hard cut.
    let mut pulses = String::new();
    let mut pulse = |x: f32, y: f32, p: f32, color: &str, op: f32| {
        // Fade in/out at the trace ends so pulses never pop.
        let end_fade = (p * std::f32::consts::PI).sin();
        let op = op * end_fade;
        if op > 0.02 {
            pulses.push_str(&format!(
                r##"<circle cx="{x:.1}" cy="{y:.1}" r="6.5" fill="{color}" opacity="{gop:.3}"/>
<circle cx="{x:.1}" cy="{y:.1}" r="2.6" fill="#ffffff" opacity="{op:.3}"/>"##,
                gop = op * 0.45,
            ));
        }
    };
    for (i, pts) in traces.iter().enumerate() {
        let phase = i as f32 * 0.137;
        // Hearing: inward (pad → tower), pace follows the speaker.
        if e.hear > 0.02 {
            let p = 1.0 - (t * (0.45 + 0.5 * e.peer) + phase).fract();
            let (x, y) = point_along(pts, p);
            pulse(x, y, p, "#5ffce0", 0.95 * e.hear);
        }
        // Speaking: outward (tower → pad), pace follows her voice.
        if e.speak > 0.02 {
            let p = (t * (0.7 + 0.8 * e.level) + phase).fract();
            let (x, y) = point_along(pts, p);
            pulse(x, y, p, "#9ff2ff", 0.95 * e.speak);
        }
        // Thinking: amber circulation — alternate directions per trace.
        if e.think > 0.02 {
            let raw = (t * 1.05 + phase * 1.4).fract();
            let p = if i % 2 == 0 { raw } else { 1.0 - raw };
            let (x, y) = point_along(pts, p);
            pulse(x, y, p, "#ffd97a", 0.9 * e.think);
        }
    }
    // Idle: one lazy wanderer so the coin always shows faint life.
    if idle > 0.05 {
        let n = traces.len();
        let slot = ((t / 3.5) as usize) % n;
        let p = (t / 3.5).fract();
        let (x, y) = point_along(&traces[slot], 1.0 - p);
        pulse(x, y, p, "#54e6d2", 0.40 * idle);
    }

    // ── Pharos tower (embossed relief) ──
    // Every solid gets a dark drop copy offset +2,+2 — cheap depth that
    // reads as struck metal.
    let tower_fill = "#7d532f";
    let tower_line = "#2e1b0e";
    let mut tower = String::new();
    let mut relief = |shape: &str| {
        tower.push_str(&format!(
            r##"<g transform="translate(2 2)" fill="#241308" stroke="none">{shape_shadow}</g>
<g fill="{tower_fill}" stroke="{tower_line}" stroke-width="1.6">{shape}</g>"##,
            shape_shadow =
                shape.replace(&format!(r##"fill="{tower_fill}""##), r##"fill="#241308""##),
        ));
    };
    // Foundation, base wall, end bastions + crenellations.
    relief(&format!(
        r##"<rect x="235" y="274" width="170" height="10" fill="{tower_fill}"/>"##
    ));
    relief(&format!(
        r##"<rect x="250" y="252" width="140" height="22" fill="{tower_fill}"/>"##
    ));
    relief(&format!(
        r##"<rect x="244" y="244" width="22" height="32" fill="{tower_fill}"/><rect x="374" y="244" width="22" height="32" fill="{tower_fill}"/>"##
    ));
    let mut teeth = String::new();
    for i in 0..6 {
        teeth.push_str(&format!(
            r##"<rect x="{x}" y="246" width="9" height="7" fill="{tower_fill}"/>"##,
            x = 272 + i * 17,
        ));
    }
    relief(&teeth);
    // Mid tier (tapered), cornice, top tier, balcony, cupola.
    relief(&format!(
        r##"<polygon points="288,252 352,252 344,168 296,168" fill="{tower_fill}"/>"##
    ));
    relief(&format!(
        r##"<rect x="290" y="162" width="60" height="7" fill="{tower_fill}"/>"##
    ));
    relief(&format!(
        r##"<polygon points="300,162 340,162 334,118 306,118" fill="{tower_fill}"/>"##
    ));
    relief(&format!(
        r##"<rect x="302" y="112" width="36" height="7" fill="{tower_fill}"/>"##
    ));
    relief(&format!(
        r##"<rect x="312" y="96" width="16" height="16" fill="{tower_fill}"/>"##
    ));
    // Highlight edges (struck-metal glint along the top-left).
    tower.push_str(
        r##"<path d="M 250 252 h 140 M 288 252 L 296 168 M 300 162 L 306 118" fill="none" stroke="#b07f4d" stroke-width="1.2" opacity="0.6"/>"##,
    );

    // ── Tower windows: VU meter (speaking) / memory flicker (thinking) ──
    let mut wins = String::new();
    let vu = (e.level * 1.25).clamp(0.0, 1.0);
    for (x, y, row) in windows() {
        let row_frac = (row as f32 + 0.5) / WINDOW_ROWS;
        let vu_lit = if row_frac < vu { 1.0 } else { 0.0 };
        let flicker = if ((t * 6.0 + row as f32 * 1.7 + x * 0.23).sin()) > 0.55 {
            1.0
        } else {
            0.0
        };
        let lit = (0.12
            + e.speak * 0.88 * vu_lit
            + e.think * 0.55 * flicker
            + idle * 0.10 * ((t * 0.4 + x * 0.7 + y).sin() * 0.5 + 0.5).powi(2))
        .clamp(0.0, 1.0);
        // Crossfade window colour from dark socket to lit cyan.
        wins.push_str(&format!(
            r##"<rect x="{x:.1}" y="{y:.1}" width="6" height="9" fill="#160c05"/>
<rect x="{x:.1}" y="{y:.1}" width="6" height="9" fill="#7ef6ff" opacity="{lit:.3}"/>"##,
        ));
        if lit > 0.5 {
            wins.push_str(&format!(
                r##"<rect x="{gx:.1}" y="{gy:.1}" width="10" height="13" rx="2" fill="#39e8ff" opacity="{gop:.3}"/>"##,
                gx = x - 2.0,
                gy = y - 2.0,
                gop = (lit - 0.5) * 0.5,
            ));
        }
    }

    // ── Beacon: breath (idle) / heartbeat (thinking) / flare (speaking) ──
    let beat = ((t * 2.4).sin() * 0.5 + 0.5).powi(2);
    let breath_g = (t * 1.5).sin() * 0.5 + 0.5;
    let beacon_r =
        11.0 + idle * 3.0 * breath_g + e.think * 6.0 * beat + e.speak * (10.0 + 30.0 * e.level);
    let beacon_op = (0.22 + 0.12 * breath_g * idle + 0.30 * e.think * beat - 0.10 * e.hear
        + e.speak * (0.30 + 0.40 * e.level))
        .clamp(0.05, 0.95);
    let flame_s = 1.0 + 0.35 * e.speak * e.level + 0.08 * (t * 3.1).sin();
    let mut rays = String::new();
    if e.speak > 0.03 {
        let ray_len = 26.0 + 54.0 * e.level;
        let rot = t * 24.0;
        for i in 0..8 {
            let a = (i as f32 / 8.0) * std::f32::consts::TAU + rot.to_radians();
            rays.push_str(&format!(
                r##"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="#cdfbff" stroke-width="2" opacity="{op:.3}"/>"##,
                x1 = CX + a.cos() * 14.0,
                y1 = 88.0 + a.sin() * 14.0,
                x2 = CX + a.cos() * (14.0 + ray_len),
                y2 = 88.0 + a.sin() * (14.0 + ray_len),
                op = e.speak * (0.25 + 0.55 * e.level),
            ));
        }
    }
    let beacon = format!(
        r##"{rays}
<circle cx="320" cy="88" r="{gr:.1}" fill="url(#beacon)" opacity="{beacon_op:.3}"/>
<g transform="translate(320 84) scale({flame_s:.3}) translate(-320 -84)">
<path d="M 320 66 C 314 74 312 80 315 86 C 317 90 323 90 325 86 C 328 80 326 74 320 66 Z" fill="#ffd97a" stroke="#8a5a1c" stroke-width="1"/>
<path d="M 320 73 C 317 78 316.5 82 318.5 85 C 319.6 87 321.6 87 322.4 85 C 324 82 323 78 320 73 Z" fill="#bdf6ff" opacity="0.9"/>
</g>
<circle cx="320" cy="86" r="3.4" fill="#ffffff" opacity="{core:.3}"/>"##,
        gr = beacon_r * 2.2,
        core = (0.5 + 0.5 * e.speak).min(1.0),
    );

    // ── Moving specular sheen — light catching the metal ──
    let sheen_x = CX + 70.0 * (t * 0.18).sin();
    let sheen = format!(
        r##"<g transform="rotate(-28 {sheen_x:.1} {CY:.1})" opacity="0.055">
<ellipse cx="{sheen_x:.1}" cy="{CY:.1}" rx="34" ry="170" fill="#ffffff"/>
</g>"##,
    );

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 640 360" preserveAspectRatio="xMidYMid slice">
<defs>
<radialGradient id="bg" cx="50%" cy="44%" r="75%">
<stop offset="0%" stop-color="#0c0a08"/><stop offset="100%" stop-color="#020202"/>
</radialGradient>
<radialGradient id="coin" cx="40%" cy="32%" r="78%">
<stop offset="0%" stop-color="#9a6a40"/><stop offset="55%" stop-color="#6e4628"/><stop offset="100%" stop-color="#41281710"/>
</radialGradient>
<radialGradient id="beacon" cx="50%" cy="50%" r="50%">
<stop offset="0%" stop-color="#dffcff" stop-opacity="0.95"/><stop offset="45%" stop-color="#39e8ff" stop-opacity="0.5"/><stop offset="100%" stop-color="#39e8ff" stop-opacity="0"/>
</radialGradient>
<radialGradient id="jewel" cx="38%" cy="32%" r="75%">
<stop offset="0%" stop-color="#ffe9ad"/><stop offset="60%" stop-color="#e8a83c"/><stop offset="100%" stop-color="#8a5a1c"/>
</radialGradient>
</defs>
<rect width="640" height="360" fill="url(#bg)"/>
<g transform="rotate({wob:.2} {CX:.1} {CY:.1}) translate({CX:.1} {CY:.1}) scale({breath:.4}) translate({ncx:.1} {ncy:.1})">
<circle cx="{CX:.1}" cy="{CY:.1}" r="{outer_r:.1}" fill="#241308"/>
<circle cx="{CX:.1}" cy="{CY:.1}" r="{COIN_R:.1}" fill="url(#coin)" stroke="#1c0f06" stroke-width="2"/>
<circle cx="{CX:.1}" cy="{CY:.1}" r="134" fill="none" stroke="#33200f" stroke-width="2.5" opacity="0.8"/>
{patina}
{beads}
{trace_svg}
{tower}
{wins}
{leds}
{jewels}
{pulses}
{beacon}
{sheen}
</g>
</svg>"##,
        w = VIDEO_W,
        h = VIDEO_H,
        outer_r = COIN_R + 5.0,
        ncx = -CX,
        ncy = -CY,
    )
}

// ── Renderer ────────────────────────────────────────────────────────

/// Rasterizes [`coin_svg`] to RGBA frames, holding the smoothed state
/// envelopes across frames (that's what makes transitions glide).
pub struct AlexandriaRenderer {
    opt: resvg::usvg::Options<'static>,
    pixmap: resvg::tiny_skia::Pixmap,
    env: Env,
}

impl AlexandriaRenderer {
    pub fn new() -> Option<Self> {
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        let pixmap = resvg::tiny_skia::Pixmap::new(VIDEO_W, VIDEO_H)?;
        Some(Self {
            opt,
            pixmap,
            env: Env::default(),
        })
    }

    /// Preview-harness entrypoint — same shape as the other pure
    /// renderers (`thinking` unavailable there, defaults to off).
    pub fn frame_rgba(&mut self, t: f32, level: f32, peer: f32) -> Vec<u8> {
        self.frame_rgba_full(t, level, peer, false)
    }

    /// Full per-frame render: advance envelopes, draw, rasterize.
    pub fn frame_rgba_full(&mut self, t: f32, level: f32, peer: f32, thinking: bool) -> Vec<u8> {
        self.env.step(level, peer, thinking, 1.0 / FPS as f32);
        let svg = coin_svg(t, &self.env);
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

/// Alexandria render loop — reads the tile's audio cells + thinking flag
/// each frame, publishes frames, sleeps to FPS.
pub(crate) fn render_loop(tile: VideoTile) {
    let mut r = match AlexandriaRenderer::new() {
        Some(r) => r,
        None => {
            tracing::error!("alexandria video: could not allocate pixmap");
            return;
        }
    };
    let frame_dt = Duration::from_millis(1000 / FPS);
    let started = Instant::now();
    tracing::info!("eliza alexandria renderer started ({VIDEO_W}x{VIDEO_H} @ {FPS}fps)");

    while tile.running.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let t = started.elapsed().as_secs_f32();
        let level = f32::from_bits(tile.level.load(Ordering::Relaxed));
        let peer = f32::from_bits(tile.peer_level.load(Ordering::Relaxed));
        let thinking = tile.thinking.load(Ordering::Relaxed);

        let rgba = r.frame_rgba_full(t, level, peer, thinking);
        let frame =
            VideoFrame::new_rgba(bytes::Bytes::from(rgba), VIDEO_W, VIDEO_H, Duration::ZERO);
        if let Ok(mut g) = tile.latest.lock() {
            *g = Some(frame);
        }

        if let Some(rest) = frame_dt.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    tracing::info!("eliza alexandria renderer stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(speak: f32, hear: f32, think: f32, level: f32, peer: f32) -> Env {
        Env {
            speak,
            hear,
            think,
            level,
            peer,
        }
    }

    #[test]
    fn svg_parses_in_every_state() {
        let opt = resvg::usvg::Options::default();
        for e in [
            env(0.0, 0.0, 0.0, 0.0, 0.0),
            env(1.0, 0.0, 0.0, 0.8, 0.0),
            env(0.0, 1.0, 0.0, 0.0, 0.6),
            env(0.0, 0.0, 1.0, 0.0, 0.0),
            // Mid-transition blends must also be valid documents.
            env(0.5, 0.3, 0.4, 0.4, 0.3),
        ] {
            for t in [0.0, 0.7, 3.9, 31.4] {
                let svg = coin_svg(t, &e);
                resvg::usvg::Tree::from_str(&svg, &opt)
                    .unwrap_or_else(|err| panic!("svg parse failed ({e:?} t={t}): {err}"));
            }
        }
    }

    #[test]
    fn frame_is_right_size_and_lights_up() {
        let mut r = AlexandriaRenderer::new().expect("renderer");
        let f = r.frame_rgba(0.4, 0.8, 0.0);
        assert_eq!(f.len(), (VIDEO_W * VIDEO_H * 4) as usize);
        let lit = f
            .chunks_exact(4)
            .filter(|p| p[0] as u16 + p[1] as u16 + p[2] as u16 > 40)
            .count();
        assert!(
            lit > 5000,
            "coin should fill a chunk of the frame, lit={lit}"
        );
    }

    #[test]
    fn envelopes_attack_fast_and_release_slow() {
        let mut e = Env::default();
        // ~0.5s of speech: speak env should be nearly full.
        for _ in 0..8 {
            e.step(0.8, 0.0, false, 1.0 / 15.0);
        }
        assert!(e.speak > 0.9, "attack too slow: {}", e.speak);
        // One frame after she stops it must NOT have snapped to zero…
        e.step(0.0, 0.0, false, 1.0 / 15.0);
        assert!(e.speak > 0.6, "release snapped: {}", e.speak);
        // …but after ~2s it should be gone.
        for _ in 0..30 {
            e.step(0.0, 0.0, false, 1.0 / 15.0);
        }
        assert!(e.speak < 0.1, "release too slow: {}", e.speak);
    }

    #[test]
    fn thinking_yields_to_speaking() {
        let mut e = Env::default();
        for _ in 0..10 {
            e.step(0.0, 0.0, true, 1.0 / 15.0);
        }
        assert!(e.think > 0.9);
        for _ in 0..10 {
            e.step(0.7, 0.0, true, 1.0 / 15.0);
        }
        assert!(e.speak > 0.8, "speaking should take over: {}", e.speak);
        assert!(
            e.think < 0.4,
            "thinking should fade under speech: {}",
            e.think
        );
    }

    #[test]
    fn point_along_walks_the_polyline() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        assert_eq!(point_along(&pts, 0.0), (0.0, 0.0));
        assert_eq!(point_along(&pts, 1.0), (10.0, 10.0));
        let (x, y) = point_along(&pts, 0.5);
        assert!(
            (x - 10.0).abs() < 0.01 && y.abs() < 0.01,
            "midpoint at the bend: ({x},{y})"
        );
    }
}
