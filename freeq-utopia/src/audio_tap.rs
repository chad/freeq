//! Custom `AudioStreamFactory` that captures decoded PCM samples
//! instead of playing them back. iroh-live's `RemoteBroadcast::audio`
//! takes a `&dyn AudioStreamFactory` and uses its `create_output` to
//! get an `AudioSink` for each remote audio track. We hand it a sink
//! that forwards every `push_samples` buffer into a tokio channel.
//!
//! We build one factory per remote broadcast so each tap channel
//! carries only that participant's samples.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use iroh_live::media::format::AudioFormat;
use iroh_live::media::traits::{AudioSink, AudioSinkHandle, AudioSource, AudioStreamFactory};
use tokio::sync::mpsc;

/// A "decoded sample" arriving from one remote track. `format` is
/// included so the consumer can resample to whisper's 16 kHz mono
/// without guessing.
pub struct PcmFrame {
    pub samples: Vec<f32>,
    pub format: AudioFormat,
}

/// Factory that captures PCM. Build one per remote broadcast and pass
/// `&factory` to `RemoteBroadcast::audio`. PCM lands on `rx`.
pub struct TapBackend {
    tx: mpsc::Sender<PcmFrame>,
}

impl TapBackend {
    /// Returns the factory + a receiver. The factory forwards every
    /// `push_samples` call to the receiver. Bounded channel — we drop
    /// frames on backpressure rather than building a multi-second
    /// queue.
    pub fn channel() -> (Self, mpsc::Receiver<PcmFrame>) {
        let (tx, rx) = mpsc::channel(128);
        (Self { tx }, rx)
    }
}

impl AudioStreamFactory for TapBackend {
    fn create_input(
        &self,
        _format: AudioFormat,
    ) -> futures_util::future::BoxFuture<'static, Result<Box<dyn AudioSource>>> {
        // Bot never publishes audio; we don't need a real mic source,
        // but iroh-live still calls create_input in some paths. Return
        // silence.
        Box::pin(async move {
            Ok(Box::new(SilentSource) as Box<dyn AudioSource>)
        })
    }

    fn create_output(
        &self,
        format: AudioFormat,
    ) -> futures_util::future::BoxFuture<'static, Result<Box<dyn AudioSink>>> {
        let tx = self.tx.clone();
        Box::pin(async move {
            Ok(Box::new(TapSink {
                format,
                paused: Arc::new(AtomicBool::new(false)),
                tx,
            }) as Box<dyn AudioSink>)
        })
    }
}

struct TapSink {
    format: AudioFormat,
    paused: Arc<AtomicBool>,
    tx: mpsc::Sender<PcmFrame>,
}

impl AudioSinkHandle for TapSink {
    fn cloned_boxed(&self) -> Box<dyn AudioSinkHandle> {
        Box::new(NullHandle { paused: self.paused.clone() })
    }
    fn pause(&self) { self.paused.store(true, Ordering::Relaxed); }
    fn resume(&self) { self.paused.store(false, Ordering::Relaxed); }
    fn is_paused(&self) -> bool { self.paused.load(Ordering::Relaxed) }
    fn toggle_pause(&self) {
        let _ = self
            .paused
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(!v));
    }
}

impl AudioSink for TapSink {
    fn format(&self) -> Result<AudioFormat> { Ok(self.format) }
    fn push_samples(&mut self, buf: &[f32]) -> Result<()> {
        // Non-blocking send — drop on backpressure. Whisper running in
        // the background can fall behind on a slow box; we'd rather
        // skip frames than wedge the decoder.
        let _ = self.tx.try_send(PcmFrame {
            samples: buf.to_vec(),
            format: self.format,
        });
        Ok(())
    }
    fn handle(&self) -> Box<dyn AudioSinkHandle> {
        Box::new(NullHandle { paused: self.paused.clone() })
    }
}

struct NullHandle {
    paused: Arc<AtomicBool>,
}

impl AudioSinkHandle for NullHandle {
    fn cloned_boxed(&self) -> Box<dyn AudioSinkHandle> {
        Box::new(NullHandle { paused: self.paused.clone() })
    }
    fn pause(&self) { self.paused.store(true, Ordering::Relaxed); }
    fn resume(&self) { self.paused.store(false, Ordering::Relaxed); }
    fn is_paused(&self) -> bool { self.paused.load(Ordering::Relaxed) }
    fn toggle_pause(&self) {
        let _ = self
            .paused
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(!v));
    }
}

struct SilentSource;
impl AudioSource for SilentSource {
    fn format(&self) -> AudioFormat {
        AudioFormat::mono_48k()
    }
    fn pop_samples(&mut self, buf: &mut [f32]) -> Result<Option<usize>> {
        // Fill with zeros; the bot never broadcasts, so this should
        // not actually be exercised in practice.
        for s in buf.iter_mut() {
            *s = 0.0;
        }
        Ok(Some(buf.len()))
    }
}

// ── Publish side: PushAudioSource + Speaker ─────────────────────────

/// Sample rate the bot's outbound broadcast runs at. 48 kHz — the
/// universal Opus rate that every other freeq client (iOS mic, web
/// mic) publishes at and that receivers' Opus decoders expect from the
/// catalog. Publishing at a non-48 kHz rate decoded to silence on the
/// receivers. TTS output (Groq Orpheus is 24 kHz) is upsampled to this
/// with a windowed-sinc resampler — a naïve linear one left audible
/// imaging artifacts ("bad-radio static").
pub const SPEAK_RATE: u32 = 48_000;

/// The publish-side audio source for the bot's own broadcast. The Opus
/// encoder pulls `pop_samples` continuously; we serve queued TTS audio
/// when there's any, silence otherwise. A continuous stream (silence
/// included) keeps subscribers attached so there's no join latency
/// when the bot does speak.
///
/// `Clone` shares the same queue — so the subscriber's reconnect loop
/// can hand a fresh clone to each new `LocalBroadcast` while the
/// `Speaker` keeps feeding the one queue.
#[derive(Clone)]
pub struct PushAudioSource {
    queue: Arc<std::sync::Mutex<std::collections::VecDeque<f32>>>,
}

impl AudioSource for PushAudioSource {
    fn format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: SPEAK_RATE,
            channel_count: 1,
        }
    }
    fn pop_samples(&mut self, buf: &mut [f32]) -> Result<Option<usize>> {
        let mut q = self.queue.lock().expect("speak queue poisoned");
        for slot in buf.iter_mut() {
            *slot = q.pop_front().unwrap_or(0.0);
        }
        Ok(Some(buf.len()))
    }
}

/// Handle the bot uses to make its broadcast speak. Clone-cheap; the
/// underlying queue is shared with the [`PushAudioSource`] feeding the
/// encoder.
#[derive(Clone)]
pub struct Speaker {
    queue: Arc<std::sync::Mutex<std::collections::VecDeque<f32>>>,
}

impl Speaker {
    /// Create a paired `(Speaker, PushAudioSource)`. The source goes to
    /// the `LocalBroadcast`; the speaker is kept by the orchestrator.
    pub fn new() -> (Speaker, PushAudioSource) {
        let queue = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        (
            Speaker { queue: queue.clone() },
            PushAudioSource { queue },
        )
    }

    /// Queue `pcm` (mono, at `from_rate`) for playback. Resampled to
    /// [`SPEAK_RATE`] and appended — concurrent enqueues just play one
    /// after another.
    pub fn enqueue(&self, pcm: &[f32], from_rate: u32) {
        let resampled = resample_mono(pcm, from_rate, SPEAK_RATE);
        let mut q = self.queue.lock().expect("speak queue poisoned");
        q.extend(resampled);
    }

    /// True while there's still queued audio the encoder hasn't drained.
    pub fn is_speaking(&self) -> bool {
        !self.queue.lock().expect("speak queue poisoned").is_empty()
    }

    /// Approximate seconds of audio still queued — used to wait out a
    /// reply before tearing the broadcast down.
    pub fn queued_secs(&self) -> f32 {
        self.queue.lock().expect("speak queue poisoned").len() as f32 / SPEAK_RATE as f32
    }
}

/// Windowed-sinc mono resampler. Shared by the whisper downsample path
/// and the TTS-playback upsample path.
///
/// Naïve linear interpolation leaves strong spectral images when
/// upsampling (the source's content mirrored above its Nyquist) — on
/// speech that's audible as fizzy/static-y sibilants. A windowed-sinc
/// kernel is the correct band-limited interpolator: for each output
/// sample it sums `2*HALF+1` input taps weighted by a Hann-windowed
/// sinc. The sinc cutoff is `min(1, ratio)` so the same routine also
/// anti-aliases when *down*sampling (e.g. 48→16 kHz for whisper).
pub fn resample_mono(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len_f = input.len() as f64 * ratio;
    // Bound pathological ratios (see to_whisper_pcm's note).
    let out_len = if out_len_f.is_finite() && out_len_f >= 0.0 {
        (out_len_f as usize).min(input.len().saturating_mul(16))
    } else {
        0
    };
    // Kernel half-width in input samples. 16 → 33-tap filter: a good
    // quality/cost balance for speech.
    const HALF: i64 = 16;
    // sinc cutoff (normalized to the input rate): for downsampling we
    // pull it in to the output Nyquist to anti-alias; for upsampling
    // it stays at the input Nyquist (1.0).
    let cutoff = ratio.min(1.0);
    let half_f = HALF as f64;

    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 / ratio; // position in input samples
        let center = src.floor() as i64;
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for k in -HALF..=HALF {
            let j = center + k;
            if j < 0 || j as usize >= input.len() {
                continue;
            }
            let x = src - j as f64; // tap distance, input samples
            if x.abs() > half_f {
                continue;
            }
            // Hann window over [-HALF, HALF].
            let w = 0.5 + 0.5 * (std::f64::consts::PI * x / half_f).cos();
            let weight = sinc(x * cutoff) * w;
            acc += input[j as usize] as f64 * weight;
            norm += weight;
        }
        // Normalize by the realized tap-weight sum — corrects gain at
        // the signal edges where the kernel is truncated.
        let s = if norm.abs() > 1e-9 { acc / norm } else { 0.0 };
        out.push(if s.is_finite() { s as f32 } else { 0.0 });
    }
    out
}

/// Normalized sinc: `sin(pi x) / (pi x)`, with the removable
/// singularity at 0 filled in.
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Naïve resampler / channel-downmixer: interleaved multi-channel f32
/// at `format.sample_rate` → mono f32 at 16 kHz, suitable for whisper.
///
/// Uses linear interpolation. Good enough for speech recognition;
/// don't ship this for music.
///
/// Adversarial input handling:
///   - `channel_count == 0` is normalized to 1 (mono).
///   - `sample_rate == 0` returns an empty buffer; same for empty input.
///   - Inputs shorter than one frame across channels return empty (we
///     never index past the end).
///   - Extreme sample rates (1 Hz, 192 kHz) don't panic.
///   - NaN / ±∞ samples are sanitised to 0.0 — whisper segfaults on
///     non-finite PCM and we'd rather drop a few samples than crash
///     the bot.
pub fn to_whisper_pcm(input: &[f32], format: AudioFormat) -> Vec<f32> {
    let channels = format.channel_count.max(1) as usize;
    let in_rate = format.sample_rate as f32;
    if input.is_empty() || in_rate <= 0.0 {
        return Vec::new();
    }

    // Step 1: downmix to mono by averaging channels. `frames` may be 0
    // when `input.len() < channels`, in which case the loop is a no-op
    // and we return an empty vec rather than panicking on index OOB.
    let frames = input.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut sum = 0.0f32;
        for c in 0..channels {
            // Sanitise non-finite inputs at the source — once they hit
            // the resample step they propagate, and any downstream
            // consumer (whisper.cpp, ffmpeg, …) is allowed to crash on
            // them. Coerce NaN/∞ to 0 so the bot can't be DoSed by a
            // peer who feeds it junk PCM.
            let s = input[f * channels + c];
            sum += if s.is_finite() { s } else { 0.0 };
        }
        mono.push(sum / channels as f32);
    }

    // Step 2: linear resample to 16 kHz.
    let target_rate = 16_000.0_f32;
    if (in_rate - target_rate).abs() < 1.0 {
        return mono;
    }
    if mono.is_empty() {
        return mono;
    }
    let ratio = target_rate / in_rate;
    let out_len_f = mono.len() as f32 * ratio;
    // Guard against huge resample ratios producing absurd allocations
    // (e.g. 192 kHz → 16 kHz is fine; 1 Hz → 16 kHz blows up). Cap at
    // 16× the input length, which still covers normal 8 kHz/16 kHz/22
    // kHz upsampling.
    let out_len = if out_len_f.is_finite() && out_len_f >= 0.0 {
        (out_len_f as usize).min(mono.len().saturating_mul(16))
    } else {
        0
    };
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_idx = i as f32 / ratio;
        let i0 = (src_idx as usize).min(mono.len() - 1);
        let i1 = (i0 + 1).min(mono.len() - 1);
        let frac = src_idx - i0 as f32;
        out.push(mono[i0] * (1.0 - frac) + mono[i1] * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_live::media::traits::{AudioSink, AudioSinkHandle, AudioSource};
    use tokio::runtime::Runtime;

    fn fmt(rate: u32, channels: u32) -> AudioFormat {
        AudioFormat {
            sample_rate: rate,
            channel_count: channels,
        }
    }

    // ---------- to_whisper_pcm ----------

    #[test]
    fn empty_input_returns_empty() {
        assert!(to_whisper_pcm(&[], AudioFormat::mono_48k()).is_empty());
    }

    #[test]
    fn zero_channel_format_does_not_panic_and_treats_as_mono() {
        // channel_count == 0 is treated as 1 (we max with 1 to avoid
        // divide-by-zero) — at 16 kHz the input should pass straight
        // through.
        let buf = vec![0.1, 0.2, 0.3, 0.4];
        let out = to_whisper_pcm(&buf, fmt(16_000, 0));
        assert_eq!(out, buf);
    }

    #[test]
    fn zero_sample_rate_returns_empty() {
        // A 0 Hz format should not panic on division and not produce
        // bogus output. Drop the buffer.
        let buf = vec![1.0, 2.0, 3.0];
        assert!(to_whisper_pcm(&buf, fmt(0, 1)).is_empty());
    }

    #[test]
    fn input_shorter_than_one_frame_returns_empty() {
        // Stereo (2 ch) but only 1 sample → frames == 0. Must not
        // index past the end.
        let out = to_whisper_pcm(&[0.5], fmt(48_000, 2));
        assert!(out.is_empty(), "expected empty, got {out:?}");
    }

    #[test]
    fn matching_sample_rate_passes_through_mono() {
        // 16 kHz mono input ⇒ identical mono output.
        let buf: Vec<f32> = (0..1000).map(|i| (i as f32) * 0.001).collect();
        let out = to_whisper_pcm(&buf, fmt(16_000, 1));
        assert_eq!(out, buf);
    }

    #[test]
    fn stereo_is_downmixed_by_averaging() {
        // L=1.0, R=-1.0 at every frame → mono of zeros.
        let buf: Vec<f32> = std::iter::repeat([1.0_f32, -1.0])
            .take(16_000)
            .flatten()
            .collect();
        let out = to_whisper_pcm(&buf, fmt(16_000, 2));
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn nan_and_inf_are_sanitized_to_zero() {
        // Whisper.cpp segfaults on non-finite samples. Adversarial PCM
        // from a malicious peer must be neutralised here.
        let buf = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        let out = to_whisper_pcm(&buf, fmt(16_000, 1));
        for (i, s) in out.iter().enumerate() {
            assert!(s.is_finite(), "sample {i} = {s} is not finite");
        }
        // The 0.5 sample must survive sanitisation untouched.
        assert!(out.iter().any(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn nan_does_not_propagate_across_downmix() {
        // One NaN in a stereo frame must NOT poison the averaged mono
        // sample. With the unguarded code (sum += NaN; sum / 2 == NaN)
        // this test catches the regression.
        let buf = vec![f32::NAN, 0.5];
        let out = to_whisper_pcm(&buf, fmt(16_000, 2));
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn extreme_low_sample_rate_does_not_panic() {
        // 1 kHz → 16 kHz is a 16× upsample. Without the saturation cap
        // we'd allocate `16 * mono.len()` floats and might panic on
        // overflow on 32-bit targets; here we just verify no panic.
        let buf: Vec<f32> = (0..32).map(|i| i as f32).collect();
        let out = to_whisper_pcm(&buf, fmt(1_000, 1));
        assert!(!out.is_empty());
        assert!(out.len() <= buf.len() * 16);
    }

    #[test]
    fn extreme_high_sample_rate_downsamples() {
        // 192 kHz → 16 kHz is 12× downsample.
        let buf: Vec<f32> = (0..1920).map(|i| (i as f32).sin()).collect();
        let out = to_whisper_pcm(&buf, fmt(192_000, 1));
        // 12× shrink from 1920 ≈ 160. Allow a slack of ±1 for
        // truncation.
        assert!(
            (159..=161).contains(&out.len()),
            "expected ~160, got {}",
            out.len()
        );
    }

    #[test]
    fn extreme_sample_rate_8khz_upsamples_to_16k() {
        // 8 kHz mono → 16 kHz should double the sample count.
        let buf: Vec<f32> = (0..800).map(|i| (i as f32) * 0.01).collect();
        let out = to_whisper_pcm(&buf, fmt(8_000, 1));
        assert!((1599..=1601).contains(&out.len()), "got {}", out.len());
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn single_sample_mono_at_non_target_rate_no_panic() {
        // mono.len() == 1, resample path: i0 == i1 == 0; the old code
        // computed `(i0 + 1).min(mono.len() - 1)` which is fine but a
        // future refactor that dropped the `.min()` would index OOB.
        let out = to_whisper_pcm(&[0.5], fmt(8_000, 1));
        // 2× upsample of 1 sample ⇒ 2 samples, both ≈ 0.5
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));
    }

    // ---------- TapBackend / TapSink ----------

    #[test]
    fn tap_sink_format_matches_requested() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let (backend, _rx) = TapBackend::channel();
            let want = fmt(44_100, 2);
            let sink = backend.create_output(want).await.unwrap();
            assert_eq!(sink.format().unwrap(), want);
        });
    }

    #[test]
    fn tap_sink_push_preserves_samples_bit_for_bit() {
        // Whisper input is f32 — even one ULP of drift compounds across
        // a 10s window. Make sure we don't accidentally normalise /
        // dither / re-quantise on the way through.
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let (backend, mut rx) = TapBackend::channel();
            let mut sink = backend.create_output(fmt(48_000, 1)).await.unwrap();
            let payload = vec![
                0.0_f32,
                1.0,
                -1.0,
                f32::MIN_POSITIVE,
                f32::MAX,
                f32::MIN,
                1.0e-30,
                -3.14159265,
            ];
            sink.push_samples(&payload).unwrap();
            let frame = rx.recv().await.expect("frame not delivered");
            assert_eq!(frame.samples, payload);
            // Bit-equal — covers signaling NaN-ness, ±0, etc.
            for (a, b) in payload.iter().zip(frame.samples.iter()) {
                assert_eq!(a.to_bits(), b.to_bits());
            }
            assert_eq!(frame.format, fmt(48_000, 1));
        });
    }

    #[test]
    fn tap_sink_pause_resume_toggle_state() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let (backend, _rx) = TapBackend::channel();
            let sink = backend.create_output(AudioFormat::mono_48k()).await.unwrap();
            assert!(!sink.is_paused());
            sink.pause();
            assert!(sink.is_paused());
            sink.resume();
            assert!(!sink.is_paused());
            sink.toggle_pause();
            assert!(sink.is_paused());
            sink.toggle_pause();
            assert!(!sink.is_paused());
            // Double pause is idempotent.
            sink.pause();
            sink.pause();
            assert!(sink.is_paused());
        });
    }

    #[test]
    fn tap_sink_push_after_receiver_drop_does_not_error() {
        // Bot's STT loop can fall arbitrarily far behind. We use
        // try_send and drop on backpressure; verify a fully closed
        // channel does not poison the sink.
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let (backend, rx) = TapBackend::channel();
            let mut sink = backend.create_output(AudioFormat::mono_48k()).await.unwrap();
            drop(rx);
            // 1000 pushes against a dead channel — must all return Ok.
            for _ in 0..1000 {
                sink.push_samples(&[0.0; 480]).unwrap();
            }
        });
    }

    #[test]
    fn tap_sink_handle_clone_shares_pause_state() {
        // `handle()` and `cloned_boxed()` should return handles that
        // share state with the source sink — pausing through the handle
        // should affect the sink (and vice versa). Catches the
        // regression where `NullHandle` is cloned but doesn't actually
        // share the atomic.
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let (backend, _rx) = TapBackend::channel();
            let sink = backend.create_output(AudioFormat::mono_48k()).await.unwrap();
            let handle = sink.handle();
            handle.pause();
            assert!(sink.is_paused(), "handle.pause() did not affect sink");
            let cloned = handle.cloned_boxed();
            cloned.resume();
            assert!(!sink.is_paused(), "cloned handle.resume() did not affect sink");
        });
    }

    // ---------- SilentSource ----------

    #[test]
    fn silent_source_fills_with_exact_zeros() {
        // Pass a buffer pre-filled with 1.0s; SilentSource must
        // overwrite every sample with 0.0 — not "leave the buffer
        // alone" which would feed garbage into the encoder.
        let mut src = SilentSource;
        for len in [0usize, 1, 7, 480, 4096] {
            let mut buf = vec![1.0_f32; len];
            let n = src.pop_samples(&mut buf).unwrap();
            assert_eq!(n, Some(len), "len={len}");
            assert!(buf.iter().all(|s| s.to_bits() == 0.0_f32.to_bits()), "len={len}");
        }
    }

    #[test]
    fn silent_source_format_is_mono_48k() {
        assert_eq!(SilentSource.format(), AudioFormat::mono_48k());
    }
}
