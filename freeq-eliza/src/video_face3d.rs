//! Real-time 3D head backends — shaded low-poly heads rendered on the CPU
//! (no GPU, so they run on the 2-vCPU boxes).
//!
//! A hand-rolled software rasterizer (z-buffer + flat shading + back-face
//! cull) over a procedural head mesh. A [`Persona3d`] parameterises the
//! whole look — proportions, jowls/lumps, skin material, lighting, brows,
//! mouth curve and animation — so very different beings share one renderer:
//!
//! - [`Persona3d::neutral`]     — a calm teal head that turns to show depth.
//! - [`Persona3d::fat_angry`]   — wide, lumpy, sickly, furrowed, snarling,
//!                                shaking with rage.
//! - [`Persona3d::slender_joy`] — tall, smooth, radiant, bright-eyed,
//!                                beaming, floating happily.
//!
//! Eyes blink, the mouth lip-syncs to her speech level, and features ride
//! the head surface so they turn and cull with it. Same RGBA frame
//! contract as every other backend.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iroh_live::media::format::VideoFrame;

use crate::video::{VIDEO_H, VIDEO_W, VideoTile};

const FPS: u64 = 15;

#[derive(Clone, Copy)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}
impl V3 {
    fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn norm(self) -> V3 {
        let l = self.dot(self).sqrt().max(1e-6);
        V3::new(self.x / l, self.y / l, self.z / l)
    }
    fn roty(self, a: f32) -> V3 {
        let (s, c) = a.sin_cos();
        V3::new(c * self.x + s * self.z, self.y, -s * self.x + c * self.z)
    }
    fn rotx(self, a: f32) -> V3 {
        let (s, c) = a.sin_cos();
        V3::new(self.x, c * self.y - s * self.z, s * self.y + c * self.z)
    }
    fn rotz(self, a: f32) -> V3 {
        let (s, c) = a.sin_cos();
        V3::new(c * self.x - s * self.y, s * self.x + c * self.y, self.z)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Neutral,
    Angry,
    Joy,
}

/// The 3D *form* — what kind of being it is, not just its proportions.
#[derive(Clone, Copy, PartialEq)]
enum Form {
    Head,
    Eye,   // a single floating eyeball
    Shard, // a spinning crystal with a glowing slit-eye
}

/// Everything that makes one 3D being look and move unlike another.
#[derive(Clone, Copy)]
pub struct Persona3d {
    kind: Kind,
    form: Form,
    height: f32,
    width: f32,
    depth: f32,
    jowl: f32,  // bulge the lower-front (fat cheeks / double chin)
    taper: f32, // narrow the chin
    lumpy: f32, // asymmetric surface bumps (ugliness / spikes)
    base: (f32, f32, f32),
    glow: (f32, f32, f32),
    ambient: f32,
    diffuse: f32,
    spec: f32,
    spec_pow: f32,
}

impl Persona3d {
    pub fn neutral() -> Self {
        Self {
            kind: Kind::Neutral, form: Form::Head,
            height: 1.08, width: 1.0, depth: 0.96, jowl: 0.0, taper: 0.26, lumpy: 0.0,
            base: (74.0, 200.0, 214.0), glow: (90.0, 150.0, 255.0),
            ambient: 0.28, diffuse: 0.85, spec: 0.4, spec_pow: 8.0,
        }
    }
    pub fn fat_angry() -> Self {
        Self {
            kind: Kind::Angry, form: Form::Head,
            height: 0.92, width: 1.2, depth: 1.1, jowl: 0.95, taper: 0.1, lumpy: 1.0,
            base: (138.0, 150.0, 92.0), // sickly olive
            glow: (200.0, 60.0, 40.0),  // angry red wash
            ambient: 0.24, diffuse: 0.82, spec: 0.12, spec_pow: 4.0,
        }
    }
    pub fn slender_joy() -> Self {
        Self {
            kind: Kind::Joy, form: Form::Head,
            height: 1.26, width: 0.8, depth: 0.9, jowl: 0.0, taper: 0.34, lumpy: 0.0,
            base: (255.0, 206.0, 156.0), // radiant warm
            glow: (255.0, 190.0, 120.0), // golden
            ambient: 0.42, diffuse: 0.72, spec: 0.7, spec_pow: 6.0,
        }
    }
    /// A giant floating eyeball — glossy white sclera, a darting iris that
    /// dilates with her voice, red veins, and no blink (extra unsettling).
    pub fn cyclops() -> Self {
        Self {
            kind: Kind::Neutral, form: Form::Eye,
            height: 0.96, width: 1.06, depth: 1.0, jowl: 0.0, taper: 0.0, lumpy: 0.0,
            base: (232.0, 230.0, 236.0), // sclera
            glow: (80.0, 255.0, 170.0),  // acid-green aura
            ambient: 0.5, diffuse: 0.6, spec: 0.6, spec_pow: 20.0,
        }
    }
    /// A spinning crystal shard with a glowing slit-eye that opens when she
    /// speaks — an eldritch geometric being.
    pub fn shard() -> Self {
        Self {
            kind: Kind::Neutral, form: Form::Shard,
            height: 1.1, width: 1.0, depth: 1.0, jowl: 0.0, taper: 0.0, lumpy: 1.0,
            base: (78.0, 58.0, 120.0),   // dark amethyst facets
            glow: (255.0, 50.0, 230.0),  // magenta core
            ambient: 0.18, diffuse: 0.95, spec: 0.6, spec_pow: 10.0,
        }
    }
}

/// Build the mesh for the persona's form (head / eyeball / crystal shard).
fn mesh_for(p: &Persona3d) -> Vec<[V3; 3]> {
    let (lat, lon) = match p.form {
        Form::Shard => (11, 13), // chunky, faceted
        _ => (22, 28),
    };
    let shape = |theta: f32, phi: f32| -> V3 {
        let (st, ct) = theta.sin_cos();
        let (sp, cp) = phi.sin_cos();
        let mut q = V3::new(st * cp, ct, st * sp);
        match p.form {
            Form::Eye => {
                // A clean sphere, lightly stretched.
                q.x *= p.width;
                q.y *= p.height;
                q.z *= p.depth;
                return q;
            }
            Form::Shard => {
                // Jagged radial spikes → a crystal.
                let n = (theta * 6.0).sin() * (phi * 4.0).sin();
                let r = 1.0 + 0.55 * n.abs().powf(0.45);
                q.x *= r * p.width;
                q.y *= r * p.height;
                q.z *= r * p.depth;
                return q;
            }
            Form::Head => {}
        }
        if p.lumpy > 0.0 {
            let l = (theta * 3.0).sin() * (phi * 2.0).sin()
                + (theta * 5.0 + 1.0).sin() * (phi * 4.0 + 2.0).sin() * 0.6;
            let s = 1.0 + p.lumpy * l * 0.06;
            q.x *= s;
            q.y *= s;
            q.z *= s;
        }
        q.y *= p.height;
        if p.jowl > 0.0 && q.y < 0.25 {
            let f = (0.25 - q.y).clamp(0.0, 1.3);
            q.x *= 1.0 + p.jowl * f * 0.4;
            q.z *= 1.0 + p.jowl * f * 0.5;
        }
        if q.y < 0.1 {
            let tp = 1.0 - ((0.1 - q.y) / 1.18) * p.taper;
            q.x *= tp;
            q.z *= tp * 0.98;
        }
        q.x *= p.width;
        q.z *= p.depth;
        q
    };
    let mut tris = Vec::new();
    for i in 0..lat {
        let t0 = std::f32::consts::PI * i as f32 / lat as f32;
        let t1 = std::f32::consts::PI * (i + 1) as f32 / lat as f32;
        for j in 0..lon {
            let p0 = 2.0 * std::f32::consts::PI * j as f32 / lon as f32;
            let p1 = 2.0 * std::f32::consts::PI * (j + 1) as f32 / lon as f32;
            let a = shape(t0, p0);
            let b = shape(t1, p0);
            let c = shape(t1, p1);
            let d = shape(t0, p1);
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
    }
    tris
}

const FOCAL: f32 = 820.0;
const CAM_Z: f32 = 3.3;

fn project(p: V3) -> (f32, f32, f32) {
    let vz = p.z - CAM_Z;
    let persp = FOCAL / (-vz);
    let sx = VIDEO_W as f32 / 2.0 + p.x * persp;
    let sy = VIDEO_H as f32 / 2.0 - p.y * persp;
    (sx, sy, -vz)
}

fn jitter(seed: f32) -> f32 {
    let x = (seed * 127.1).sin() * 43758.547;
    (x - x.floor()) * 2.0 - 1.0
}

pub struct Face3dRenderer {
    persona: Persona3d,
    mesh: Vec<[V3; 3]>,
}

impl Face3dRenderer {
    pub fn new(persona: Persona3d) -> Self {
        let mesh = mesh_for(&persona);
        Self { persona, mesh }
    }

    pub fn frame_rgba(&self, t: f32, level: f32, peer: f32) -> Vec<u8> {
        let p = &self.persona;
        let level = level.clamp(0.0, 1.0);
        let _ = peer; // these forms react to her own voice, not the listener
        let speaking = level > 0.03;

        let w = VIDEO_W as usize;
        let h = VIDEO_H as usize;
        let mut buf = vec![0u8; w * h * 4];
        let mut zbuf = vec![f32::INFINITY; w * h];

        // ── Background: dark + persona glow ──────────────────────────────
        let cxp = w as f32 * 0.5;
        let cyp = h as f32 * 0.46;
        let glow_r = h as f32 * 0.64;
        let glow_amt = 0.18 + 0.12 * level;
        for y in 0..h {
            let bg = 7.0 + 9.0 * (1.0 - y as f32 / h as f32);
            for x in 0..w {
                let dx = x as f32 - cxp;
                let dy = y as f32 - cyp;
                let d = (dx * dx + dy * dy).sqrt() / glow_r;
                let g = (1.0 - d).clamp(0.0, 1.0).powf(2.5) * glow_amt;
                let idx = (y * w + x) * 4;
                buf[idx] = (bg + p.glow.0 * g).min(255.0) as u8;
                buf[idx + 1] = (bg + p.glow.1 * g).min(255.0) as u8;
                buf[idx + 2] = (bg + p.glow.2 * g).min(255.0) as u8;
                buf[idx + 3] = 255;
            }
        }

        // ── Pose ─────────────────────────────────────────────────────────
        let (yaw, pitch, roll, sdx, sdy, scale) = match p.form {
            Form::Eye => (
                0.16 * (t * 0.5).sin(),
                0.1 * (t * 0.8).sin(),
                0.0,
                0.0,
                0.0,
                1.0,
            ),
            Form::Shard => (
                t * 0.7, // continuous spin
                0.3 + 0.4 * (t * 0.5).sin(),
                t * 0.3,
                0.0,
                0.0,
                1.0 + 0.07 * level + 0.03 * (t * 4.0).sin(), // pulse
            ),
            Form::Head => match p.kind {
                Kind::Neutral => (
                    0.5 * (t * 0.45).sin(),
                    0.12 * (t * 0.7).sin() + if speaking { 0.04 * (t * 9.0).sin() } else { 0.0 },
                    0.0, 0.0, 0.0, 1.0,
                ),
                Kind::Angry => {
                    let shake = 2.0 + 7.0 * level;
                    (
                        0.22 * (t * 0.9).sin(),
                        0.08 * (t * 1.1).sin() + 0.07,
                        0.04 * (t * 1.3).sin(),
                        jitter(t * 41.0) * shake,
                        jitter(t * 47.0) * shake,
                        1.0,
                    )
                }
                Kind::Joy => (
                    0.6 * (t * 0.4).sin(),
                    0.08 * (t * 0.6).sin(),
                    0.12 * (t * 0.5).sin(),
                    0.0,
                    -7.0 * (t * 1.6).sin().abs(),
                    1.0,
                ),
            },
        };
        let xform = |v: V3| {
            let v = V3::new(v.x * scale, v.y * scale, v.z * scale);
            v.rotz(roll).rotx(pitch).roty(yaw)
        };
        let proj = |v: V3| {
            let (x, y, d) = project(v);
            (x + sdx, y + sdy, d)
        };

        // Light (world space, upper-left-front).
        let light = V3::new(-0.45, 0.55, 0.8).norm();
        // Material, with an angry red flush while shouting.
        let mat = if p.kind == Kind::Angry && speaking {
            (
                (p.base.0 + 95.0 * level).min(255.0),
                (p.base.1 - 45.0 * level).max(0.0),
                (p.base.2 - 30.0 * level).max(0.0),
            )
        } else {
            p.base
        };

        for tri in &self.mesh {
            let w0 = xform(tri[0]);
            let w1 = xform(tri[1]);
            let w2 = xform(tri[2]);
            let n = w1.sub(w0).cross(w2.sub(w0)).norm();
            if n.z <= 0.02 {
                continue;
            }
            let ndl = n.dot(light).max(0.0);
            let shade = (p.ambient + p.diffuse * ndl).min(1.3);
            let sp = ndl.powf(p.spec_pow) * p.spec;
            let col = (
                ((mat.0 * shade) + 255.0 * sp).min(255.0) as u8,
                ((mat.1 * shade) + 255.0 * sp).min(255.0) as u8,
                ((mat.2 * shade) + 255.0 * sp).min(255.0) as u8,
            );
            fill_tri(&mut buf, &mut zbuf, proj(w0), proj(w1), proj(w2), col);
        }

        // ── Features ─────────────────────────────────────────────────────
        let blink = {
            let phase = (t % 4.0) / 4.0;
            if phase > 0.965 {
                let q = (phase - 0.965) / 0.035;
                (1.0 - (q * std::f32::consts::PI).sin()).clamp(0.06, 1.0)
            } else {
                1.0
            }
        };
        match p.form {
            Form::Eye => {
                // Darting iris on the sclera sphere; pupil dilates with voice.
                let gx = 0.34 * (t * 0.9).sin();
                let gy = 0.24 * (t * 1.7).sin();
                let gz = (1.0 - gx * gx - gy * gy).max(0.05).sqrt();
                let wp = xform(V3::new(gx, gy, gz));
                let (sx, sy, _) = proj(wp);
                let sc = FOCAL / (CAM_Z - wp.z);
                // Bloodshot veins radiating in toward the iris.
                for k in 0..8 {
                    let a = k as f32 * 0.82;
                    let vl = V3::new(0.82 * a.cos(), 0.82 * a.sin(), 0.57);
                    let vn = xform(vl);
                    if vn.z <= 0.2 {
                        continue;
                    }
                    let (vx, vy, _) = proj(vn);
                    stroke_seg(&mut buf, vx, vy, sx + (vx - sx) * 0.5, sy + (vy - sy) * 0.5, 2.4, (192, 64, 58));
                }
                let ir = 0.34 * sc;
                fill_ellipse(&mut buf, sx, sy, ir, ir, (74, 196, 138));
                fill_ellipse(&mut buf, sx, sy, ir * 0.64, ir * 0.64, (30, 92, 66));
                let pr = ir * (0.30 + 0.32 * level);
                fill_ellipse(&mut buf, sx, sy, pr, pr, (8, 10, 12));
                fill_ellipse(&mut buf, sx - ir * 0.3, sy - ir * 0.3, ir * 0.18, ir * 0.18, (255, 255, 255));
            }
            Form::Shard => {
                // A glowing slit-eye at the centre, camera-facing, opening
                // with her voice — the only stable point on the spinning crystal.
                let cx = VIDEO_W as f32 / 2.0;
                let cy = VIDEO_H as f32 / 2.0;
                let g = p.glow;
                let sx = 22.0 + 9.0 * level;
                let sy = 11.0 + 74.0 * level;
                fill_ellipse(&mut buf, cx, cy, sx, sy, (g.0 as u8, g.1 as u8, g.2 as u8));
                fill_ellipse(&mut buf, cx, cy, sx * 0.52, sy * 0.72, (255, 255, 255));
            }
            Form::Head => {
        let (eye_rx, eye_ry, eye_y) = match p.kind {
            Kind::Angry => (0.072, 0.058, 0.13),
            Kind::Joy => (0.10, 0.115, 0.17),
            Kind::Neutral => (0.085, 0.10, 0.16),
        };

        // Eyes + brows.
        for &sgn in &[-1.0f32, 1.0] {
            let local = V3::new(0.30 * sgn, eye_y, 0.92);
            let nrm = xform(local.norm());
            if nrm.z <= 0.15 {
                continue;
            }
            let wp = xform(local);
            let (sx, sy, _) = proj(wp);
            let sc = FOCAL / (CAM_Z - wp.z);
            let ew = eye_rx * sc;
            let eh = eye_ry * sc * blink;
            fill_ellipse(&mut buf, sx, sy, ew, eh, (245, 249, 255));
            if blink > 0.4 {
                let (phx, phy, pr) = match p.kind {
                    Kind::Angry => (sgn * ew * 0.25, eh * 0.3, 0.5), // glaring, low+inner
                    _ => (0.0, 0.0, 0.42),
                };
                fill_ellipse(&mut buf, sx + phx, sy + phy, ew * pr, eh * pr, (22, 26, 34));
                let hl = if p.kind == Kind::Joy { 0.42 } else { 0.24 };
                fill_ellipse(&mut buf, sx + ew * 0.2, sy - eh * 0.25, ew * hl, eh * hl, (255, 255, 255));
            }
            // Brow.
            let (in_y, out_y, thick, bc) = match p.kind {
                Kind::Angry => (sy - ew * 1.0, sy - ew * 2.5, ew * 0.5, (28, 18, 14)),
                Kind::Joy => (sy - ew * 2.3, sy - ew * 2.0, ew * 0.22, (150, 110, 80)),
                Kind::Neutral => continue,
            };
            let in_x = sx - sgn * ew * 0.6; // toward face centre
            let out_x = sx + sgn * ew * 1.25; // outer
            stroke_seg(&mut buf, in_x, in_y, out_x, out_y, thick, bc);
        }

        // Mouth.
        {
            let local = V3::new(0.0, -0.40, 0.92);
            let nrm = xform(local.norm());
            if nrm.z > 0.15 {
                let wp = xform(local);
                let (mx, my, _) = proj(wp);
                let sc = FOCAL / (CAM_Z - wp.z);
                match p.kind {
                    Kind::Joy => {
                        // beaming smile; opens with speech, top teeth show
                        draw_lips(&mut buf, mx, my, 0.20 * sc, 0.11 * sc, 0.03 * sc, (150, 70, 70));
                        if speaking {
                            let rx = 0.15 * sc;
                            let ry = (0.02 + 0.13 * level) * sc;
                            fill_ellipse(&mut buf, mx, my + ry * 0.4, rx, ry, (90, 30, 40));
                            fill_ellipse(&mut buf, mx, my + ry * 0.05, rx * 0.92, ry * 0.32, (255, 255, 255));
                        }
                    }
                    Kind::Angry => {
                        // snarl: downturned, bared lower teeth when shouting
                        if speaking {
                            let rx = 0.17 * sc;
                            let ry = (0.03 + 0.14 * level) * sc;
                            fill_ellipse(&mut buf, mx, my, rx, ry, (24, 10, 10));
                            fill_ellipse(&mut buf, mx, my + ry * 0.55, rx * 0.85, ry * 0.3, (235, 230, 215)); // gritted teeth
                        }
                        draw_lips(&mut buf, mx, my - 0.02 * sc, 0.18 * sc, -0.06 * sc, 0.035 * sc, (30, 14, 14));
                    }
                    Kind::Neutral => {
                        let rx = (0.16 + 0.04 * level) * sc;
                        let ry = (0.02 + 0.16 * level) * sc;
                        fill_ellipse(&mut buf, mx, my, rx, ry, (26, 14, 18));
                        if level > 0.18 {
                            fill_ellipse(&mut buf, mx, my + ry * 0.4, rx * 0.6, ry * 0.4, (224, 85, 122));
                        }
                    }
                }
            }
        }
            }
        }

        buf
    }
}

/// Lip line — a quadratic arc from corner to corner. `curve > 0` bows the
/// middle downward (a smile, corners up); `curve < 0` bows it up (a frown).
fn draw_lips(buf: &mut [u8], cx: f32, cy: f32, halfw: f32, curve: f32, thick: f32, color: (u8, u8, u8)) {
    let n = 9;
    let mut prev: Option<(f32, f32)> = None;
    for i in 0..=n {
        let s = -1.0 + 2.0 * i as f32 / n as f32;
        let x = cx + s * halfw;
        let y = cy + curve * (1.0 - s * s);
        if let Some((px, py)) = prev {
            stroke_seg(buf, px, py, x, y, thick, color);
        }
        prev = Some((x, y));
    }
}

fn fill_tri(
    buf: &mut [u8],
    zbuf: &mut [f32],
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    c: (f32, f32, f32),
    (r, g, bl): (u8, u8, u8),
) {
    let w = VIDEO_W as i32;
    let h = VIDEO_H as i32;
    let minx = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
    let maxx = a.0.max(b.0).max(c.0).ceil().min((w - 1) as f32) as i32;
    let miny = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
    let maxy = a.1.max(b.1).max(c.1).ceil().min((h - 1) as f32) as i32;
    if minx > maxx || miny > maxy {
        return;
    }
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if area.abs() < 1e-3 {
        return;
    }
    let inv = 1.0 / area;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) * inv;
            let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) * inv;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let depth = w0 * a.2 + w1 * b.2 + w2 * c.2;
            let zi = (y * w + x) as usize;
            if depth < zbuf[zi] {
                zbuf[zi] = depth;
                let idx = zi * 4;
                buf[idx] = r;
                buf[idx + 1] = g;
                buf[idx + 2] = bl;
            }
        }
    }
}

fn fill_ellipse(buf: &mut [u8], cx: f32, cy: f32, rx: f32, ry: f32, (r, g, b): (u8, u8, u8)) {
    if rx < 0.5 || ry < 0.5 {
        return;
    }
    let w = VIDEO_W as i32;
    let h = VIDEO_H as i32;
    let minx = (cx - rx).floor().max(0.0) as i32;
    let maxx = (cx + rx).ceil().min((w - 1) as f32) as i32;
    let miny = (cy - ry).floor().max(0.0) as i32;
    let maxy = (cy + ry).ceil().min((h - 1) as f32) as i32;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let nx = (x as f32 + 0.5 - cx) / rx;
            let ny = (y as f32 + 0.5 - cy) / ry;
            if nx * nx + ny * ny <= 1.0 {
                let idx = ((y * w + x) * 4) as usize;
                buf[idx] = r;
                buf[idx + 1] = g;
                buf[idx + 2] = b;
            }
        }
    }
}

/// Thick line segment via distance-to-segment fill.
fn stroke_seg(buf: &mut [u8], x0: f32, y0: f32, x1: f32, y1: f32, half: f32, (r, g, b): (u8, u8, u8)) {
    if half < 0.4 {
        return;
    }
    let w = VIDEO_W as i32;
    let h = VIDEO_H as i32;
    let minx = (x0.min(x1) - half).floor().max(0.0) as i32;
    let maxx = (x0.max(x1) + half).ceil().min((w - 1) as f32) as i32;
    let miny = (y0.min(y1) - half).floor().max(0.0) as i32;
    let maxy = (y0.max(y1) + half).ceil().min((h - 1) as f32) as i32;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = (dx * dx + dy * dy).max(1e-4);
    let h2 = half * half;
    for y in miny..=maxy {
        for x in minx..=maxx {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let s = (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0);
            let cxs = x0 + s * dx;
            let cys = y0 + s * dy;
            let d2 = (px - cxs) * (px - cxs) + (py - cys) * (py - cys);
            if d2 <= h2 {
                let idx = ((y * w + x) * 4) as usize;
                buf[idx] = r;
                buf[idx + 1] = g;
                buf[idx + 2] = b;
            }
        }
    }
}

pub(crate) fn render_loop(tile: VideoTile) {
    render_loop_with(tile, Persona3d::neutral());
}

pub(crate) fn render_loop_with(tile: VideoTile, persona: Persona3d) {
    let renderer = Face3dRenderer::new(persona);
    let frame_dt = Duration::from_millis(1000 / FPS);
    let started = Instant::now();
    tracing::info!("eliza 3d-head renderer started ({VIDEO_W}x{VIDEO_H} @ {FPS}fps)");

    while tile.running.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let t = started.elapsed().as_secs_f32();
        let level = f32::from_bits(tile.level.load(Ordering::Relaxed));
        let peer = f32::from_bits(tile.peer_level.load(Ordering::Relaxed));
        let rgba = renderer.frame_rgba(t, level, peer);
        let frame =
            VideoFrame::new_rgba(bytes::Bytes::from(rgba), VIDEO_W, VIDEO_H, Duration::ZERO);
        if let Ok(mut g) = tile.latest.lock() {
            *g = Some(frame);
        }
        if let Some(rest) = frame_dt.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    tracing::info!("eliza 3d-head renderer stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_personas() -> [Persona3d; 5] {
        [
            Persona3d::neutral(),
            Persona3d::fat_angry(),
            Persona3d::slender_joy(),
            Persona3d::cyclops(),
            Persona3d::shard(),
        ]
    }

    #[test]
    fn meshes_are_nonempty() {
        for p in all_personas() {
            assert!(mesh_for(&p).len() > 100);
        }
    }

    #[test]
    fn frames_render_for_each_persona() {
        for p in all_personas() {
            let r = Face3dRenderer::new(p);
            let f = r.frame_rgba(4.0, 0.7, 0.0);
            assert_eq!(f.len(), (VIDEO_W * VIDEO_H * 4) as usize);
            assert!(f.chunks_exact(4).all(|px| px[3] == 255));
        }
    }
}
