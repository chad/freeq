//! Offline render of the vector-character backend → a PNG frame sequence.
//!
//!   cargo run --release --example vector_demo -- /tmp/vector_frames
//!   ffmpeg -y -framerate 15 -i /tmp/vector_frames/f%04d.png \
//!     -c:v libx264 -pix_fmt yuv420p -movflags +faststart vector.mp4
//!
//! Reuses the production `VectorRenderer`, driven by the same synthetic
//! idle → speaking → listening → idle timeline as the ASCII demo.

use freeq_eliza::video::{VIDEO_H, VIDEO_W};
use freeq_eliza::video_vector::VectorRenderer;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/vector_frames".into());
    std::fs::create_dir_all(&out).expect("create out dir");

    let fps = 15.0f32;
    let secs = 14.0f32;
    let frames = (fps * secs) as usize;
    let mut r = VectorRenderer::new().expect("renderer");

    println!("rendering {frames} frames ({VIDEO_W}x{VIDEO_H}) → {out}");
    for f in 0..frames {
        let t = f as f32 / fps;
        let (level, peer) = envelope(t);
        let rgba = r.frame_rgba(t, level, peer);
        let img: image::RgbaImage =
            image::ImageBuffer::from_raw(VIDEO_W, VIDEO_H, rgba).expect("buffer→image");
        img.save(format!("{out}/f{f:04}.png")).expect("save png");
    }
    println!("done. encode with:\n  ffmpeg -y -framerate 15 -i {out}/f%04d.png -c:v libx264 -pix_fmt yuv420p -movflags +faststart vector.mp4");
}

fn envelope(t: f32) -> (f32, f32) {
    if (2.0..9.0).contains(&t) {
        let s = t - 2.0;
        let phrase = ((s * 0.6).sin() * 0.5 + 0.5).powf(0.5);
        let syl = ((s * 13.0).sin() * 0.5 + 0.5).powf(2.2);
        let micro = ((s * 31.0).sin() * 0.5 + 0.5) * 0.2;
        let level = ((syl * 0.85 + micro) * phrase).clamp(0.0, 1.0);
        (level, 0.0)
    } else if (9.0..12.0).contains(&t) {
        let s = t - 9.0;
        let peer = (0.35 + 0.25 * ((s * 5.0).sin() * 0.5 + 0.5)).clamp(0.0, 1.0);
        (0.0, peer)
    } else {
        (0.0, 0.0)
    }
}
