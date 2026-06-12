//! Offline render of the pre-existing live backends — the `svg` cyberpunk
//! presence and the `particles` ghostly characters (eliza / narrator /
//! utopia / oblivion) — so they sit in the same gallery as the newer
//! styles.
//!
//! These backends have no pure `frame_rgba`; they run real-time render
//! loops that publish to the tile. So we drive them the way the live AV
//! path does: spawn the renderer, feed a synthetic audio envelope into the
//! level cells, and pull frames back off `tile.source()` at 15 fps.
//!
//!   cargo run --release --example legacy_demo -- /tmp/svg_frames svg
//!   cargo run --release --example legacy_demo -- /tmp/utopia_frames particles utopia
//!   ffmpeg -y -framerate 15 -i /tmp/svg_frames/f%04d.png \
//!     -c:v libx264 -pix_fmt yuv420p -movflags +faststart svg.mp4

use std::sync::atomic::Ordering;
use std::thread::sleep;
use std::time::{Duration, Instant};

use freeq_eliza::video::{Backend, VIDEO_H, VIDEO_W, VideoTile};
use iroh_live::media::format::FrameData;
use iroh_live::media::traits::VideoSource;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/legacy_frames".into());
    let backend_name = args.next().unwrap_or_else(|| "svg".into());
    let character = args.next().unwrap_or_else(|| "eliza".into());
    std::fs::create_dir_all(&out).expect("create out dir");

    let backend = match backend_name.as_str() {
        "particles" => Backend::Particles { character: character.clone(), ghostly_pack: None },
        _ => Backend::Svg,
    };
    let tile = VideoTile::with_backend(backend);
    tile.spawn_renderer();
    let mut src = tile.source();
    let lvl = tile.level_handle();
    let pr = tile.peer_level_handle();

    let fps = 15.0f32;
    let secs = 14.0f32;
    let frames = (fps * secs) as usize;
    println!("rendering {frames} frames of {backend_name} {character} → {out} (real-time)");

    let start = Instant::now();
    let mut prev: Option<Vec<u8>> = None;
    for f in 0..frames {
        let t = f as f32 / fps;
        let (level, peer) = envelope(t);
        lvl.store(level.to_bits(), Ordering::Relaxed);
        pr.store(peer.to_bits(), Ordering::Relaxed);

        // Wait until this frame's wall-clock slot, then take the newest
        // frame the renderer has published (drain any backlog).
        let target = start + Duration::from_secs_f32(t);
        while Instant::now() < target {
            sleep(Duration::from_millis(2));
        }
        let mut newest = None;
        while let Ok(Some(fr)) = src.pop_frame() {
            if let FrameData::Packed { data, .. } = fr.data {
                newest = Some(data.to_vec());
            }
        }
        let rgba = newest.or_else(|| prev.clone());
        if let Some(rgba) = rgba {
            let img: image::RgbaImage =
                image::ImageBuffer::from_raw(VIDEO_W, VIDEO_H, rgba.clone()).expect("buffer→image");
            img.save(format!("{out}/f{f:04}.png")).expect("save png");
            prev = Some(rgba);
        }
    }
    tile.stop();
    println!("done.");
}

fn envelope(t: f32) -> (f32, f32) {
    if (2.0..9.0).contains(&t) {
        let s = t - 2.0;
        let phrase = ((s * 0.6).sin() * 0.5 + 0.5).powf(0.5);
        let syl = ((s * 13.0).sin() * 0.5 + 0.5).powf(2.2);
        let micro = ((s * 31.0).sin() * 0.5 + 0.5) * 0.2;
        (((syl * 0.85 + micro) * phrase).clamp(0.0, 1.0), 0.0)
    } else if (9.0..12.0).contains(&t) {
        let s = t - 9.0;
        (0.0, (0.35 + 0.25 * ((s * 5.0).sin() * 0.5 + 0.5)).clamp(0.0, 1.0))
    } else {
        (0.0, 0.0)
    }
}
