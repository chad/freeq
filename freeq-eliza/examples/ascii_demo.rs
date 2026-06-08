//! Offline render of the ASCII video backend → a PNG frame sequence, so
//! the "terminal being" look can be reviewed without a live AV call.
//!
//!   cargo run --release --example ascii_demo -- /tmp/ascii_frames
//!
//! Then encode (15 fps to match the renderer):
//!   ffmpeg -framerate 15 -i /tmp/ascii_frames/f%04d.png \
//!     -c:v libx264 -pix_fmt yuv420p -movflags +faststart ascii.mp4
//!
//! It reuses the production `AsciiRenderer::frame_rgba`, driving it with a
//! synthetic timeline: idle → speaking (a syllable-like envelope so the
//! mouth flaps naturally) → listening → idle.

use freeq_eliza::video::{VIDEO_H, VIDEO_W};
use freeq_eliza::video_ascii::AsciiRenderer;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/ascii_frames".into());
    std::fs::create_dir_all(&out).expect("create out dir");

    let fps = 15.0f32;
    let secs = 14.0f32;
    let frames = (fps * secs) as usize;
    let r = AsciiRenderer::new().expect("renderer");

    println!("rendering {frames} frames ({VIDEO_W}x{VIDEO_H}) → {out}");
    for f in 0..frames {
        let t = f as f32 / fps;
        let (level, peer) = envelope(t);
        let rgba = r.frame_rgba(t, level, peer);
        let img: image::RgbaImage =
            image::ImageBuffer::from_raw(VIDEO_W, VIDEO_H, rgba).expect("buffer→image");
        let path = format!("{out}/f{f:04}.png");
        img.save(&path).expect("save png");
    }
    println!("done. encode with:\n  ffmpeg -y -framerate 15 -i {out}/f%04d.png -c:v libx264 -pix_fmt yuv420p -movflags +faststart ascii.mp4");
}

/// Synthetic (level, peer) timeline.
///  0–2s   idle (closed mouth, blink, breathing)
///  2–9s   speaking — a syllable envelope opens/closes the mouth
///  9–12s  listening — peer audio pulses, face tints mint
///  12–14s idle again
fn envelope(t: f32) -> (f32, f32) {
    if (2.0..9.0).contains(&t) {
        // Syllables ~4/sec, each a quick attack/decay, with phrase gaps so
        // it reads like speech rather than a buzz.
        let s = t - 2.0;
        let phrase = ((s * 0.6).sin() * 0.5 + 0.5).powf(0.5); // slow envelope, occasional dips
        let syl = ((s * 13.0).sin() * 0.5 + 0.5).powf(2.2); // fast syllable flaps
        let micro = ((s * 31.0).sin() * 0.5 + 0.5) * 0.2;
        let level = (syl * 0.85 + micro) * phrase;
        (level.clamp(0.0, 1.0), 0.0)
    } else if (9.0..12.0).contains(&t) {
        let s = t - 9.0;
        let peer = 0.35 + 0.25 * ((s * 5.0).sin() * 0.5 + 0.5);
        (0.0, peer.clamp(0.0, 1.0))
    } else {
        (0.0, 0.0)
    }
}
