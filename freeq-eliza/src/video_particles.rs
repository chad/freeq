//! Alternative tile renderer — ghostly particle face.
//!
//! Activated by `--render-backend particles --ghostly-character NAME`.
//! Replaces the SVG presence with a procedural particle face from the
//! sibling [`ghostly`] crate.
//!
//! ## Mood → Emotion mapping
//!
//! The SVG path categorises Eliza into five moods (Idle, Listening,
//! Thinking, Speaking, Vision). Ghostly's sentiment system has eight
//! emotion targets; we map the moods to the closest emotion so the
//! palette shifts visibly per mood:
//!
//! | Mood      | Emotion (ghostly) |
//! | --------- | ----------------- |
//! | Idle      | Calm              |
//! | Listening | Curiosity         |
//! | Thinking  | Curiosity         |
//! | Speaking  | Passion           |
//! | Vision    | Awe               |
//!
//! The emotion is applied at low intensity (~0.45) so the *character's*
//! own palette still reads as the dominant identity.
//!
//! ## What's not wired (yet)
//!
//! - Scene cards, whiteboards, ambient HUD, vision PiP, EQ strip —
//!   these are SVG-only overlays. On the particles backend
//!   `show_scene` / `show_board` / `set_vision_thumb` / `set_ambient`
//!   become no-ops in the user-visible output (the SVG-side state
//!   still updates but isn't rendered). Composite mode is future work.
//! - Lip-sync mouth — the SVG face's `level`-driven mouth aperture
//!   does not yet drive the particle field. We pass `level` into the
//!   per-frame breath so the whole face pulses with her speech, but a
//!   true lip-syncing geometry update is TODO.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use ghostly::{
    apply_emotion, characters as gho_characters, Character as GhoCharacter, Emotion, FaceState,
    RenderSettings, Renderer,
};
use iroh_live::media::format::VideoFrame;
use resvg::tiny_skia::{Pixmap, PixmapPaint, Transform};
use resvg::usvg;

use crate::video::{overlay_svg_for_particles, VideoTile, VIDEO_H, VIDEO_W};

const FPS: u64 = 15;
/// Particle count at 1280×720. Roughly 2.3× the 12K we used at 360p
/// — the field needs more density to read as solid at the higher
/// resolution. CPU render still hits 15 fps with comfortable headroom.
const PARTICLES: usize = 28_000;
/// Emotion-blend intensity — how strongly the mood mood overrides the
/// character's resting palette. Low so a fully-Joy Eliza still reads as
/// Eliza, not a generic solar face.
const EMOTION_INTENSITY: f32 = 0.45;

/// Particle-backend render loop. Drains the tile's mood/audio cells
/// each frame, picks an emotion + intensity, asks ghostly to render,
/// and publishes the frame back to the tile.
pub(crate) fn render_loop(tile: VideoTile, character_name: &str) {
    let base_character = match gho_characters::by_name(character_name) {
        Some(c) => c,
        None => {
            tracing::warn!(
                character = %character_name,
                "unknown ghostly character; falling back to 'eliza'"
            );
            gho_characters::by_name("eliza").expect("eliza always exists")
        }
    };
    tracing::info!(character = %base_character.name, particles = PARTICLES, "particle-face renderer started");

    let settings = RenderSettings {
        width: VIDEO_W,
        height: VIDEO_H,
        ..RenderSettings::default()
    };
    let Some(mut renderer) = Renderer::new(settings) else {
        tracing::error!("particle renderer: failed to allocate {VIDEO_W}x{VIDEO_H} pixmap");
        return;
    };

    // Scale 6.5 — the face dominates almost the entire tile. Every
    // character renders at this scale (not just Oblivion); the user
    // wants the agent to feel *present*, not floating in a small
    // patch of a bigger frame.
    const SCALE: f32 = 6.5;
    let mut state = FaceState::new(&base_character, PARTICLES, SCALE, 42);

    // Scratch pixmap for rasterizing the overlay SVG each frame. We
    // allocate once and reuse so the per-frame cost is just the
    // resvg::render pass, not heap thrashing.
    let mut overlay_pixmap = match Pixmap::new(VIDEO_W, VIDEO_H) {
        Some(p) => p,
        None => {
            tracing::error!("particle renderer: failed to allocate overlay pixmap");
            return;
        }
    };
    let mut usvg_opt = usvg::Options::default();
    usvg_opt.fontdb_mut().load_system_fonts();
    // Most recently-rebuilt character — apply_emotion produces a new
    // value, but we don't want to rebuild every frame when the
    // emotion is steady. Compare against the last applied (emotion,
    // intensity) and reuse when unchanged.
    let mut current_character = apply_emotion(&base_character, Emotion::Calm, EMOTION_INTENSITY);
    let mut last_applied: (Emotion, u32) = (Emotion::Calm, quantize(EMOTION_INTENSITY));

    let frame_dt = Duration::from_millis(1000 / FPS);
    let started = Instant::now();
    while tile.running.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let t = started.elapsed().as_secs_f32();

        // Snapshot the same signals the SVG loop reads.
        let level = f32::from_bits(tile.level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let peer = f32::from_bits(tile.peer_level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let thinking = tile.thinking.load(Ordering::Relaxed);
        let has_vision_thumb = tile
            .vision_thumb
            .lock()
            .ok()
            .map(|g| g.is_some())
            .unwrap_or(false);

        // Mood classification — mirrors `presence_svg` in video.rs.
        let emotion = if has_vision_thumb {
            Emotion::Awe
        } else if level > 0.03 {
            Emotion::Passion
        } else if thinking {
            Emotion::Curiosity
        } else if peer > 0.03 {
            Emotion::Curiosity
        } else {
            Emotion::Calm
        };

        // Re-blend the character only when the emotion changes — cheap
        // but not free (rebuilds the contour path Vec).
        let key = (emotion, quantize(EMOTION_INTENSITY));
        if key != last_applied {
            current_character = apply_emotion(&base_character, emotion, EMOTION_INTENSITY);
            last_applied = key;
        }

        // Push the louder of (her own voice, peer audio) into the
        // particle field as a reactivity scalar. The renderer expands
        // the per-particle wobble + size with this — when she's
        // speaking, the face visibly shimmers; in silence it just
        // breathes.
        state.set_audio_level(level.max(peer));

        // Status halo drivers — peer audio drives the breathing
        // "listening" halo; the thinking flag drives the rotating
        // "working" arc. Together they tell the operator at a glance
        // whether the agent is hearing sound and whether it has a
        // call in flight — visible regardless of which way the face
        // happens to be turned.
        //
        // Hand-raise flash boosts the listening level briefly when
        // the bot was name-dropped but not addressed: "I have
        // something to add." Decays linearly over 3 s so the boost
        // reads as a deliberate pulse, not a steady state.
        let hand_raise = match tile.hand_raise_seconds_ago() {
            Some(elapsed) if elapsed < 3.0 => 1.0 - (elapsed / 3.0),
            _ => 0.0,
        };
        state.set_listening_level((peer + hand_raise * 0.7).clamp(0.0, 1.0));
        state.set_working(thinking);

        // Sticky gaze — when the bot is mid-exchange with someone,
        // its eyes turn toward that nick (deterministic hash → angle).
        // Cleared elsewhere when the exchange ends; idle drift resumes.
        let focus = tile.focus_nick.lock().ok().and_then(|g| g.clone());
        state.set_gaze_lock(focus);

        let dt_secs = frame_dt.as_secs_f32();
        // Drive the head turn — picks a new gaze target every few
        // seconds and eases the current yaw/pitch toward it. Makes
        // the face feel like a real being looking around the room.
        state.step_gaze(t, dt_secs);
        state.step_blink(t, dt_secs);
        state.step_eye_saccade(t, dt_secs);
        // Detect speech onset → full-field bright flash + camera
        // shake jolt. Must come after set_audio_level so the rising
        // edge sees the current frame's level.
        state.step_audio_onset(t, dt_secs);

        // Brow expression — derived from emotion. Curiosity (her
        // alert listening / thinking) raises the brow; Passion
        // (speaking) lowers it slightly (drama); Awe (vision) lifts
        // higher; Calm sits neutral.
        let brow = match emotion {
            Emotion::Curiosity => 0.6,
            Emotion::Awe => 0.85,
            Emotion::Passion => -0.35,
            Emotion::Concern => -0.5,
            Emotion::Triumph => 0.7,
            Emotion::Joy | Emotion::Warmth => 0.3,
            Emotion::Calm => 0.0,
        };
        state.set_brow(brow);

        // Tick the ember swarm (only does anything when the character
        // configured an EmberConfig — Oblivion's signature flavour).
        if let Some(cfg) = current_character.render_config.embers {
            state.step_embers(&cfg, dt_secs, SCALE);
        }

        let pixmap = renderer.render(&current_character, &state, t);

        // ── Overlay pass ──
        // Scene card / whiteboard / ambient HUD / vision PiP — same
        // rich visual aids the SVG backend draws, composited on top
        // of the particle face. Skips entirely when there's nothing
        // to draw (quiet listening with no ambient pick yet).
        let mut composed = pixmap.clone();
        if let Some(overlay_svg) = overlay_svg_for_particles(&tile, t) {
            if let Ok(tree) = usvg::Tree::from_str(&overlay_svg, &usvg_opt) {
                // Clear scratch (overlay rasterizes onto a transparent
                // canvas; we composite that onto the particle pixmap).
                overlay_pixmap.data_mut().fill(0);
                resvg::render(
                    &tree,
                    Transform::identity(),
                    &mut overlay_pixmap.as_mut(),
                );
                composed.draw_pixmap(
                    0,
                    0,
                    overlay_pixmap.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            } else {
                tracing::debug!("particle overlay SVG failed to parse, skipping");
            }
        }

        // Publish the frame. Match the iroh-live frame format the SVG
        // path produces (RGBA, same dimensions, zero timestamp — the
        // encoder timestamps frames itself).
        let data = Bytes::copy_from_slice(composed.data());
        let frame = VideoFrame::new_rgba(data, VIDEO_W, VIDEO_H, Duration::ZERO);
        if let Ok(mut g) = tile.latest.lock() {
            *g = Some(frame);
        }

        if let Some(rest) = frame_dt.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    tracing::info!("particle-face renderer stopped");
    // Silence unused-import warning when this fn returns.
    let _ = std::mem::size_of::<GhoCharacter>();
}

/// Quantize intensity to a u32 so a small change doesn't churn the
/// "did the blend change?" check. `0.001` resolution is more than the
/// renderer can perceive.
#[inline]
fn quantize(f: f32) -> u32 {
    (f * 1000.0) as u32
}
