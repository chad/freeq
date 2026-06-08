//! Real-time 3D head backend — a shaded low-poly head, rendered on the
//! CPU (no GPU needed, so it runs on the 2-vCPU boxes).
//!
//! A hand-rolled software rasterizer: a procedurally-generated head mesh
//! (a deformed UV sphere) is rotated each frame, flat-shaded per triangle
//! against one key light (so the facets read as low-poly), and filled with
//! a z-buffer. The face slowly turns side-to-side so the 3D depth is
//! obvious. Eyes blink and the mouth opens with her speech level — these
//! ride on the head surface so they turn with it and hide when it faces
//! away.
//!
//! Same frame contract as every other backend: an RGBA 1280×720 buffer.

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
}

/// Build a head mesh: a UV sphere deformed to head proportions (taller,
/// tapered jaw, slightly flattened front).
fn head_mesh() -> Vec<[V3; 3]> {
    const LAT: usize = 20;
    const LON: usize = 26;
    let shape = |theta: f32, phi: f32| -> V3 {
        let (st, ct) = theta.sin_cos();
        let (sp, cp) = phi.sin_cos();
        let mut p = V3::new(st * cp, ct, st * sp);
        // Slightly taller head.
        p.y *= 1.08;
        // Gently taper the lower half toward a rounded chin (not a point).
        let taper = if p.y < 0.1 {
            1.0 - ((0.1 - p.y) / 1.18) * 0.26
        } else {
            1.0
        };
        p.x *= taper;
        p.z *= taper * 0.96; // slightly flatter front-to-back
        p
    };
    let mut tris = Vec::new();
    for i in 0..LAT {
        let t0 = std::f32::consts::PI * i as f32 / LAT as f32;
        let t1 = std::f32::consts::PI * (i + 1) as f32 / LAT as f32;
        for j in 0..LON {
            let p0 = 2.0 * std::f32::consts::PI * j as f32 / LON as f32;
            let p1 = 2.0 * std::f32::consts::PI * (j + 1) as f32 / LON as f32;
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

/// Project a camera-space point (camera at +Z looking toward −Z) to screen
/// pixels + depth. Returns (sx, sy, depth).
fn project(p: V3) -> (f32, f32, f32) {
    let vz = p.z - CAM_Z; // negative (in front)
    let persp = FOCAL / (-vz);
    let sx = VIDEO_W as f32 / 2.0 + p.x * persp;
    let sy = VIDEO_H as f32 / 2.0 - p.y * persp;
    (sx, sy, -vz)
}

pub struct Face3dRenderer {
    mesh: Vec<[V3; 3]>,
}

impl Face3dRenderer {
    pub fn new() -> Self {
        Self { mesh: head_mesh() }
    }

    pub fn frame_rgba(&self, t: f32, level: f32, peer: f32) -> Vec<u8> {
        let level = level.clamp(0.0, 1.0);
        let peer = peer.clamp(0.0, 1.0);
        let speaking = level > 0.03;
        let listening = !speaking && peer > 0.03;

        let w = VIDEO_W as usize;
        let h = VIDEO_H as usize;
        let mut buf = vec![0u8; w * h * 4];
        let mut zbuf = vec![f32::INFINITY; w * h];

        // ── Background: dark vertical gradient + soft mood glow ──────────
        let accent = if speaking {
            (255.0, 210.0, 74.0)
        } else if listening {
            (62.0, 255.0, 214.0)
        } else {
            (108.0, 176.0, 255.0)
        };
        let cxp = w as f32 * 0.5;
        let cyp = h as f32 * 0.46;
        let glow_r = h as f32 * 0.62;
        for y in 0..h {
            let vy = y as f32 / h as f32;
            let bg = 8.0 + 10.0 * (1.0 - vy); // top a touch lighter
            for x in 0..w {
                let dx = x as f32 - cxp;
                let dy = y as f32 - cyp;
                let d = (dx * dx + dy * dy).sqrt() / glow_r;
                let g = (1.0 - d).clamp(0.0, 1.0).powf(2.5) * 0.22;
                let idx = (y * w + x) * 4;
                buf[idx] = (bg + accent.0 * g) as u8;
                buf[idx + 1] = (bg + accent.1 * g) as u8;
                buf[idx + 2] = (bg + accent.2 * g) as u8;
                buf[idx + 3] = 255;
            }
        }

        // ── Head transform: gentle turn + nod/bob ────────────────────────
        let yaw = 0.5 * (t * 0.45).sin();
        let pitch = 0.12 * (t * 0.7).sin() + if speaking { 0.04 * (t * 9.0).sin() } else { 0.0 };
        let xform = |p: V3| p.rotx(pitch).roty(yaw);

        // Key light (world space, upper-left-front).
        let light = V3::new(-0.45, 0.55, 0.8).norm();
        let base = (74.0, 200.0, 214.0); // cool cyber-teal material

        for tri in &self.mesh {
            let w0 = xform(tri[0]);
            let w1 = xform(tri[1]);
            let w2 = xform(tri[2]);
            // Flat normal.
            let n = w1.sub(w0).cross(w2.sub(w0)).norm();
            // Back-face cull (camera looks toward −Z, so a front face has
            // a normal with positive z).
            if n.z <= 0.02 {
                continue;
            }
            let ndl = n.dot(light).max(0.0);
            let shade = (0.28 + 0.85 * ndl).min(1.25);
            let spec = ndl.powf(8.0) * 0.4;
            let col = (
                ((base.0 * shade) + 255.0 * spec).min(255.0) as u8,
                ((base.1 * shade) + 255.0 * spec).min(255.0) as u8,
                ((base.2 * shade) + 255.0 * spec).min(255.0) as u8,
            );
            let a = project(w0);
            let b = project(w1);
            let c = project(w2);
            fill_tri(&mut buf, &mut zbuf, a, b, c, col);
        }

        // ── Eyes + mouth: ride the head surface, turn + cull with it ────
        let blink = {
            let phase = (t % 4.0) / 4.0;
            if phase > 0.965 {
                let p = (phase - 0.965) / 0.035;
                (1.0 - (p * std::f32::consts::PI).sin()).clamp(0.06, 1.0)
            } else {
                1.0
            }
        };
        // Eye centres + outward normals (on the front of the head).
        for &sgn in &[-1.0f32, 1.0] {
            let local = V3::new(0.30 * sgn, 0.16, 0.92);
            let wp = xform(local);
            let nrm = xform(V3::new(0.30 * sgn, 0.16, 0.92).norm());
            if nrm.z <= 0.15 {
                continue; // facing away
            }
            let (sx, sy, _) = project(wp);
            let sc = FOCAL / (CAM_Z - wp.z);
            let rx = 0.085 * sc;
            let ry = 0.10 * sc * blink;
            fill_ellipse(&mut buf, sx, sy, rx, ry, (245, 249, 255));
            if blink > 0.4 {
                fill_ellipse(&mut buf, sx, sy, rx * 0.42, ry * 0.42, (20, 24, 34));
            }
        }
        // Mouth.
        {
            let local = V3::new(0.0, -0.42, 0.92);
            let wp = xform(local);
            let nrm = xform(V3::new(0.0, -0.42, 0.92).norm());
            if nrm.z > 0.15 {
                let (sx, sy, _) = project(wp);
                let sc = FOCAL / (CAM_Z - wp.z);
                let rx = (0.16 + 0.04 * level) * sc;
                let ry = (0.02 + 0.16 * level) * sc;
                fill_ellipse(&mut buf, sx, sy, rx, ry, (26, 14, 18));
                if level > 0.18 {
                    fill_ellipse(&mut buf, sx, sy + ry * 0.4, rx * 0.6, ry * 0.4, (224, 85, 122));
                }
            }
        }

        buf
    }
}

/// Fill a screen-space triangle with a z-buffer test. Each vertex is
/// `(sx, sy, depth)`.
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

/// Fill an axis-aligned screen-space ellipse (no z-test — drawn on top of
/// the head front).
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

pub(crate) fn render_loop(tile: VideoTile) {
    let renderer = Face3dRenderer::new();
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

    #[test]
    fn mesh_is_nonempty() {
        assert!(head_mesh().len() > 100);
    }

    #[test]
    fn frame_is_right_size_and_draws_head() {
        let r = Face3dRenderer::new();
        let f = r.frame_rgba(0.0, 0.6, 0.0);
        assert_eq!(f.len(), (VIDEO_W * VIDEO_H * 4) as usize);
        // The teal head should colour a meaningful chunk of the centre.
        let w = VIDEO_W as usize;
        let mut teal = 0;
        for y in (VIDEO_H as usize / 4)..(VIDEO_H as usize * 3 / 4) {
            for x in (w / 3)..(w * 2 / 3) {
                let i = (y * w + x) * 4;
                if f[i + 1] > 120 && f[i + 2] > 120 && f[i] < 160 {
                    teal += 1;
                }
            }
        }
        assert!(teal > 5000, "expected a visible head, got {teal} teal px");
    }
}
