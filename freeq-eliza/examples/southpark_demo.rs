//! Offline render of the belligerent South Park-style backend → PNG
//! frame sequence.
//!
//!   cargo run --release --example southpark_demo -- /tmp/sp_frames
//!   ffmpeg -y -framerate 15 -i /tmp/sp_frames/f%04d.png \
//!     -c:v libx264 -pix_fmt yuv420p -movflags +faststart southpark.mp4
//!
//! Envelope leans into a tantrum: grumpy idle → an escalating SCREAM →
//! suspicious glare → grumble.

use freeq_eliza::video::{VIDEO_H, VIDEO_W};
use freeq_eliza::video_southpark::SouthParkRenderer;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/sp_frames".into());
    std::fs::create_dir_all(&out).expect("create out dir");
    let (fps, secs) = (15.0f32, 14.0f32);
    let frames = (fps * secs) as usize;
    let mut r = SouthParkRenderer::new().expect("renderer");
    println!("rendering {frames} frames → {out}");
    for f in 0..frames {
        let t = f as f32 / fps;
        let (level, peer) = envelope(t);
        let rgba = r.frame_rgba(t, level, peer);
        let img: image::RgbaImage =
            image::ImageBuffer::from_raw(VIDEO_W, VIDEO_H, rgba).expect("buffer→image");
        img.save(format!("{out}/f{f:04}.png")).expect("save png");
    }
    println!("done.");
}

fn envelope(t: f32) -> (f32, f32) {
    if (2.0..9.0).contains(&t) {
        // A rant that escalates: ride a rising floor, ranty syllables on top.
        let s = t - 2.0;
        let ramp = (s / 7.0).clamp(0.0, 1.0); // builds toward a peak scream
        let syl = ((s * 11.0).sin() * 0.5 + 0.5).powf(1.4);
        let level = (0.35 + 0.5 * ramp) * (0.55 + 0.45 * syl);
        (level.clamp(0.0, 1.0), 0.0)
    } else if (9.5..12.0).contains(&t) {
        // suspicious glare
        (0.0, 0.5)
    } else {
        (0.0, 0.0)
    }
}
