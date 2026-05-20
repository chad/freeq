//! Unit-level tests for `audio_tap`: PCM resampling, channel handling,
//! and the `TapBackend` channel plumbing.

use freeq_utopia::audio_tap::{PcmFrame, TapBackend, to_whisper_pcm};
use iroh_live::media::format::AudioFormat;
use iroh_live::media::traits::AudioStreamFactory;

/// Stereo 48 kHz → mono 16 kHz: length scales by ~1/3 and channels are
/// averaged, not just one channel dropped.
#[tokio::test]
async fn stereo_48k_resamples_to_mono_16k_with_averaging() {
    let stereo = AudioFormat { sample_rate: 48_000, channel_count: 2 };
    // 480 stereo frames = 0.01s. Left = 1.0, right = -1.0; the mono
    // average is 0.0 and a "drop left" strategy would yield -1.0
    // instead.
    let frames = 480;
    let mut input = Vec::with_capacity(frames * 2);
    for _ in 0..frames {
        input.push(1.0f32);
        input.push(-1.0f32);
    }
    let out = to_whisper_pcm(&input, stereo);

    // 480 frames at 48k → 160 samples at 16k. Allow ±1.
    let expected = frames * 16_000 / 48_000;
    assert!(
        (out.len() as i64 - expected as i64).abs() <= 1,
        "expected ~{expected} samples, got {}",
        out.len()
    );

    // Every sample should be ~0.0 because L+R averaged. If we were
    // dropping channels we'd see 1.0 or -1.0.
    for s in &out {
        assert!(s.abs() < 1e-3, "channel-drop fallback would have non-zero value: {s}");
    }
}

/// Empty input: returns empty, no panic, no division by zero.
#[test]
fn empty_input_returns_empty() {
    let f = AudioFormat::mono_48k();
    assert!(to_whisper_pcm(&[], f).is_empty());
}

/// Mono 16 kHz is already the target shape → resample is a passthrough.
#[test]
fn mono_16k_is_passthrough() {
    let f = AudioFormat { sample_rate: 16_000, channel_count: 1 };
    let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.001).sin()).collect();
    let out = to_whisper_pcm(&input, f);
    assert_eq!(out.len(), input.len(), "16k mono path must not resample");
    for (a, b) in input.iter().zip(out.iter()) {
        assert!((a - b).abs() < 1e-6, "samples should be byte-equal: {a} vs {b}");
    }
}

/// 7.1-style multi-channel: averaging shouldn't crash and length
/// shrinks by the channel count ratio.
#[test]
fn multichannel_average_works() {
    let f = AudioFormat { sample_rate: 16_000, channel_count: 6 };
    // 6 channels: 3 are 1.0, 3 are -1.0 → mono average 0.0.
    let frames = 1024;
    let mut input = Vec::with_capacity(frames * 6);
    for _ in 0..frames {
        for c in 0..6 {
            input.push(if c < 3 { 1.0 } else { -1.0 });
        }
    }
    let out = to_whisper_pcm(&input, f);
    assert_eq!(out.len(), frames);
    for s in &out {
        assert!(s.abs() < 1e-3);
    }
}

/// Sending a PcmFrame through the TapBackend channel: receiver gets the
/// same samples + format that the AudioSink received.
#[tokio::test]
async fn tap_backend_round_trips_samples() {
    use iroh_live::media::traits::AudioSink;
    let (backend, mut rx) = TapBackend::channel();
    let format = AudioFormat::mono_48k();
    let mut sink = backend.create_output(format).await.expect("create_output");
    let samples = vec![0.1f32, 0.2, 0.3, 0.4];
    sink.push_samples(&samples).expect("push_samples");

    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("recv timeout")
        .expect("channel closed");
    assert_eq!(frame.samples, samples);
    assert_eq!(frame.format, format);
}

/// Backpressure: push more than the channel capacity. The sink must
/// not block and must not error — frames are dropped silently.
#[tokio::test]
async fn backpressure_drops_silently() {
    use iroh_live::media::traits::AudioSink;
    let (backend, _rx_unused) = TapBackend::channel();
    let format = AudioFormat::mono_48k();
    let mut sink = backend.create_output(format).await.unwrap();
    // The internal channel is bounded at 128. Push 1000 frames; the
    // receiver never consumes. Without `try_send` this would deadlock.
    let one = vec![0.0f32; 16];
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        sink.push_samples(&one).expect("push must never error");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "1000 push_samples in a stalled channel took {elapsed:?} — looks like it blocked",
    );
}

/// Pause / resume / toggle on a sink handle: the handle wraps the same
/// atomic flag as the sink, so calls go through. Mostly here to catch a
/// future regression where someone wires the handle to a stale atomic.
#[tokio::test]
async fn sink_handle_pause_resume() {
    use iroh_live::media::traits::AudioSink;
    let (backend, _rx) = TapBackend::channel();
    let sink = backend.create_output(AudioFormat::mono_48k()).await.unwrap();
    let h = sink.handle();
    assert!(!h.is_paused());
    h.pause();
    assert!(h.is_paused());
    h.resume();
    assert!(!h.is_paused());
    h.toggle_pause();
    assert!(h.is_paused());
}

/// PcmFrame is the public struct we expose; this is mostly a smoke test
/// that the type compiles in user code and that the format field is
/// accessible.
#[test]
fn pcm_frame_public_fields() {
    let f = PcmFrame {
        samples: vec![0.0; 4],
        format: AudioFormat::mono_48k(),
    };
    assert_eq!(f.samples.len(), 4);
    assert_eq!(f.format.sample_rate, 48_000);
}
