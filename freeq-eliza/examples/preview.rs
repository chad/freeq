//! Render a being preview driven by a real audio envelope, so the mouth
//! lip-syncs to a spoken line that gets muxed in afterward.
//!
//!   preview <out_dir> <style> <levels_file>
//!
//! `levels_file` is one float per video frame (her speech level 0..1),
//! produced from the TTS audio by scripts/envelope.py. `style` is any
//! `--render-backend` value, or `particles-<character>` for the ghostly
//! faces. Frame count = number of levels.

use std::sync::atomic::Ordering;
use std::thread::sleep;
use std::time::{Duration, Instant};

use freeq_eliza::video::{Backend, VIDEO_H, VIDEO_W, VideoTile};
use freeq_eliza::video_alexandria::AlexandriaRenderer;
use freeq_eliza::video_ascii::{AsciiBotRenderer, AsciiGlitchRenderer, AsciiRainRenderer, AsciiRenderer};
use freeq_eliza::video_face3d::{Face3dRenderer, Persona3d};
use freeq_eliza::video_southpark::{SouthParkRenderer, SpStyle};
use freeq_eliza::video_vector::VectorRenderer;
use iroh_live::media::format::FrameData;
use iroh_live::media::traits::VideoSource;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("out dir");
    let style = args.next().expect("style");
    let levels_path = args.next().expect("levels file");
    std::fs::create_dir_all(&out).expect("create out dir");
    let levels: Vec<f32> = std::fs::read_to_string(&levels_path)
        .expect("read levels")
        .lines()
        .filter_map(|l| l.trim().parse::<f32>().ok())
        .collect();
    let n = levels.len();
    eprintln!("preview {style}: {n} frames → {out}");

    let save = |f: usize, rgba: Vec<u8>| {
        let img: image::RgbaImage =
            image::ImageBuffer::from_raw(VIDEO_W, VIDEO_H, rgba).expect("buf→img");
        img.save(format!("{out}/f{f:04}.png")).expect("save png");
    };
    let tf = |f: usize| f as f32 / 15.0;

    // Pure renderers: frame_rgba(t, level, peer) per frame.
    macro_rules! run_pure {
        ($r:expr) => {{
            let r = $r;
            for f in 0..n {
                save(f, r.frame_rgba(tf(f), levels[f], 0.0));
            }
        }};
    }
    macro_rules! run_pure_mut {
        ($r:expr) => {{
            let mut r = $r;
            for f in 0..n {
                save(f, r.frame_rgba(tf(f), levels[f], 0.0));
            }
        }};
    }

    match style.as_str() {
        "ascii" => run_pure!(AsciiRenderer::new().expect("ascii")),
        "ascii-rain" => run_pure!(AsciiRainRenderer::new().expect("rain")),
        "ascii-glitch" => run_pure!(AsciiGlitchRenderer::new().expect("glitch")),
        "ascii-bot" => run_pure!(AsciiBotRenderer::new().expect("bot")),
        "vector" => run_pure_mut!(VectorRenderer::new().expect("vector")),
        "southpark" => run_pure_mut!(SouthParkRenderer::new(SpStyle::Belligerent).expect("sp")),
        "southpark-goofy" => run_pure_mut!(SouthParkRenderer::new(SpStyle::Goofy).expect("sp")),
        "southpark-burnout" | "southpark-stoner" => {
            run_pure_mut!(SouthParkRenderer::new(SpStyle::Stoner).expect("sp"))
        }
        "3d" => run_pure!(Face3dRenderer::new(Persona3d::neutral())),
        "3d-angry" => run_pure!(Face3dRenderer::new(Persona3d::fat_angry())),
        "3d-joy" => run_pure!(Face3dRenderer::new(Persona3d::slender_joy())),
        "3d-eye" => run_pure!(Face3dRenderer::new(Persona3d::cyclops())),
        "3d-shard" => run_pure!(Face3dRenderer::new(Persona3d::shard())),
        "alexandria" => run_pure_mut!(AlexandriaRenderer::new().expect("alexandria")),
        // State-language demo: ignore the levels and walk the coin through
        // idle → hearing → thinking → speaking so each light direction is
        // legible on its own. Frame count still comes from the levels file.
        "alexandria-states" => {
            let mut r = AlexandriaRenderer::new().expect("alexandria");
            for f in 0..n {
                let t = tf(f);
                let phase = (f * 4) / n.max(1); // quarter each
                let (level, peer, thinking) = match phase {
                    0 => (0.0, 0.0, false),                                   // idle
                    1 => (0.0, 0.5 + 0.4 * (t * 5.0).sin().abs(), false),     // hearing
                    2 => (0.0, 0.0, true),                                    // thinking
                    _ => (levels[f].max(0.35 + 0.4 * (t * 7.0).sin().abs()), 0.0, false), // speaking
                };
                save(f, r.frame_rgba_full(t, level, peer, thinking));
            }
        }
        other => {
            // Legacy live-loop backends (svg / particles-<char>): drive the
            // tile's level cell in real time and pull frames off the source.
            let backend = if let Some(ch) = other.strip_prefix("particles-") {
                Backend::Particles { character: ch.to_string(), ghostly_pack: None }
            } else {
                Backend::Svg
            };
            let tile = VideoTile::with_backend(backend);
            tile.spawn_renderer();
            let mut src = tile.source();
            let lvl = tile.level_handle();
            let start = Instant::now();
            let mut prev: Option<Vec<u8>> = None;
            for f in 0..n {
                lvl.store(levels[f].to_bits(), Ordering::Relaxed);
                let target = start + Duration::from_secs_f32(tf(f));
                while Instant::now() < target {
                    sleep(Duration::from_millis(2));
                }
                let mut newest = None;
                while let Ok(Some(fr)) = src.pop_frame() {
                    if let FrameData::Packed { data, .. } = fr.data {
                        newest = Some(data.to_vec());
                    }
                }
                if let Some(rgba) = newest.or_else(|| prev.clone()) {
                    save(f, rgba.clone());
                    prev = Some(rgba);
                }
            }
            tile.stop();
        }
    }
    eprintln!("done.");
}
