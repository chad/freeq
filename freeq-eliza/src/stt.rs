//! Speech-to-text. Two backends behind one async `SttEngine`:
//!
//! - **Groq** — the hosted OpenAI-compatible transcription API
//!   (`whisper-large-v3-turbo` by default). Fast, accurate, no local
//!   toolchain. Selected automatically when `GROQ_API_KEY` is set.
//! - **Local whisper** — whisper.cpp via `whisper-rs`, behind the
//!   `stt` cargo feature (needs cmake + a model file).
//! - **Noop** — returns empty transcriptions. The fallback when no
//!   Groq key is set and the `stt` feature is off; lets the full IRC +
//!   MoQ + relay path run in tests without any STT dependency.

#[cfg(feature = "stt")]
use std::path::Path;
#[cfg(feature = "stt")]
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh_live::media::format::AudioFormat;

/// Async STT engine. Held in an `Arc` and shared across per-participant
/// tap tasks.
pub enum SttEngine {
    /// Hosted Groq transcription. `model` is e.g.
    /// `whisper-large-v3-turbo`.
    Groq {
        client: reqwest::Client,
        api_key: String,
        model: String,
        /// Whisper context prompt seeding the vocabulary with the
        /// agent's (and its peers') names. Without it Whisper mangles
        /// an unusual name at the start of an utterance — "Yokota,
        /// what time is it?" came back as "You could tell what time
        /// is it." — so the bot never sees that it was addressed.
        prompt: String,
    },
    /// Local whisper.cpp (feature-gated).
    #[cfg(feature = "stt")]
    Local(Arc<imp::Whisper>),
    /// No STT — every window transcribes to "".
    Noop,
}

impl SttEngine {
    /// Construct a Groq-backed engine. `vocab_names` are the names
    /// Whisper must recognise — the agent's own name first, then any
    /// peer agents — woven into the transcription context prompt.
    pub fn groq(api_key: String, model: String, vocab_names: &[String]) -> Self {
        SttEngine::Groq {
            client: reqwest::Client::new(),
            api_key,
            model,
            prompt: vocab_prompt(vocab_names),
        }
    }

    /// Construct a local-whisper engine. Errors if the model can't be
    /// loaded. Only available with the `stt` feature.
    #[cfg(feature = "stt")]
    pub fn local(model_path: &Path) -> Result<Self> {
        Ok(SttEngine::Local(Arc::new(imp::Whisper::load(model_path)?)))
    }

    /// A no-op engine.
    pub fn noop() -> Self {
        SttEngine::Noop
    }

    /// Human-readable backend name for startup logging.
    pub fn label(&self) -> String {
        match self {
            SttEngine::Groq { model, .. } => format!("groq:{model}"),
            #[cfg(feature = "stt")]
            SttEngine::Local(_) => "local-whisper".to_string(),
            SttEngine::Noop => "noop".to_string(),
        }
    }

    /// Transcribe a window of 16 kHz mono f32 PCM. Returns the
    /// recognized text, trimmed; empty string on silence/noise.
    pub async fn transcribe(&self, pcm_16k_mono: &[f32]) -> Result<String> {
        // Less than ~1s of audio is never worth a round-trip.
        if pcm_16k_mono.len() < 16_000 {
            return Ok(String::new());
        }
        match self {
            SttEngine::Groq {
                client,
                api_key,
                model,
                prompt,
            } => groq_transcribe(client, api_key, model, prompt, pcm_16k_mono).await,
            #[cfg(feature = "stt")]
            SttEngine::Local(whisper) => {
                let whisper = whisper.clone();
                let pcm = pcm_16k_mono.to_vec();
                tokio::task::spawn_blocking(move || whisper.transcribe(&pcm))
                    .await
                    .context("whisper blocking task panicked")?
            }
            SttEngine::Noop => Ok(String::new()),
        }
    }
}

// ── Groq backend ─────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct GroqResponse {
    #[serde(default)]
    text: String,
}

/// Build the Whisper context prompt from the names it must get right.
/// Whisper biases toward vocabulary it has seen in the prompt, so a
/// natural sentence naming the assistant (and any peers) makes
/// addressed utterances transcribe correctly instead of mangling the
/// name into nearby English ("Yokota" → "You could tell").
pub(crate) fn vocab_prompt(vocab_names: &[String]) -> String {
    fn cap(s: &str) -> String {
        let mut c = s.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    }
    let mut names: Vec<String> = Vec::new();
    for n in vocab_names {
        let n = n.trim();
        if n.is_empty() {
            continue;
        }
        let n = cap(n);
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let first = names
        .first()
        .cloned()
        .unwrap_or_else(|| "Eliza".to_string());
    let mut prompt = format!(
        "A live voice call with the assistant {first}. \
         People say \"{first}\" to ask it questions."
    );
    if names.len() > 1 {
        prompt.push_str(&format!(
            " Other participants are named {}.",
            names[1..].join(", ")
        ));
    }
    prompt
}

async fn groq_transcribe(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    prompt: &str,
    pcm_16k_mono: &[f32],
) -> Result<String> {
    let wav = encode_wav_16k_mono(pcm_16k_mono);
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .context("building multipart audio part")?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "json")
        .text("language", "en")
        // Seed Whisper's vocabulary with the assistant's (and peers')
        // names — see `vocab_prompt`. Without this it mangles the name
        // at the start of an utterance into nearby English, so the bot
        // never sees that it was addressed and never replies.
        .text("prompt", prompt.to_string())
        .text("temperature", "0");

    let resp = client
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .context("groq transcription request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("groq transcription {status}: {body}");
    }
    let parsed: GroqResponse = resp.json().await.context("groq response parse failed")?;
    Ok(parsed.text.trim().to_string())
}

/// Encode 16 kHz mono f32 PCM as a 16-bit PCM WAV byte buffer. Groq's
/// API wants a real audio container; a WAV is the cheapest one to
/// produce. Samples are clamped to `[-1.0, 1.0]` before quantization.
pub(crate) fn encode_wav_16k_mono(pcm: &[f32]) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 16_000;
    const CHANNELS: u16 = 1;
    const BITS: u16 = 16;
    let byte_rate = SAMPLE_RATE * CHANNELS as u32 * (BITS as u32 / 8);
    let block_align = CHANNELS * (BITS / 8);
    let data_len = (pcm.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in pcm {
        let clamped = s.clamp(-1.0, 1.0);
        let q = (clamped * 32767.0) as i16;
        out.extend_from_slice(&q.to_le_bytes());
    }
    out
}

// ── PCM conditioning ─────────────────────────────────────────────────

/// Naïve resampler / channel-downmixer: interleaved multi-channel f32
/// at `format.sample_rate` → mono f32 at 16 kHz, suitable for whisper.
///
/// Uses linear interpolation. Good enough for speech recognition;
/// don't ship this for music. (The agent's *playback* path uses the
/// band-limited [`freeq_av::resample_mono`] instead.)
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

// ── Local whisper backend (feature-gated) ────────────────────────────

#[cfg(feature = "stt")]
mod imp {
    use super::*;
    use std::sync::Mutex;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    pub struct Whisper {
        ctx: Mutex<WhisperContext>,
    }

    impl Whisper {
        pub fn load(path: &Path) -> Result<Self> {
            let path_str = path
                .to_str()
                .context("whisper model path is not valid UTF-8")?;
            let ctx =
                WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                    .context("WhisperContext::new failed; is the model path correct?")?;
            Ok(Self {
                ctx: Mutex::new(ctx),
            })
        }

        pub fn transcribe(&self, pcm_16k_mono: &[f32]) -> Result<String> {
            if pcm_16k_mono.len() < 16_000 {
                return Ok(String::new());
            }
            let ctx = self.ctx.lock().expect("whisper context poisoned");
            let mut state = ctx.create_state().context("whisper create_state failed")?;

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some("en"));
            params.set_translate(false);
            params.set_no_context(true);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);

            state
                .full(params, pcm_16k_mono)
                .context("whisper inference failed")?;

            let segments = state.full_n_segments().unwrap_or(0);
            let mut out = String::new();
            for i in 0..segments {
                if let Ok(text) = state.full_get_segment_text(i) {
                    out.push_str(&text);
                }
            }
            Ok(out.trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_44_bytes_plus_pcm() {
        let pcm = vec![0.0f32; 1000];
        let wav = encode_wav_16k_mono(&pcm);
        assert_eq!(wav.len(), 44 + 1000 * 2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn wav_clamps_out_of_range_samples() {
        // +2.0 and -2.0 must not wrap — they clamp to the i16 rails.
        let wav = encode_wav_16k_mono(&[2.0, -2.0]);
        let s0 = i16::from_le_bytes([wav[44], wav[45]]);
        let s1 = i16::from_le_bytes([wav[46], wav[47]]);
        assert_eq!(s0, 32767);
        assert_eq!(s1, -32767);
    }

    #[test]
    fn wav_data_length_field_matches_payload() {
        let pcm = vec![0.5f32; 320];
        let wav = encode_wav_16k_mono(&pcm);
        let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_len, 320 * 2);
    }

    #[tokio::test]
    async fn noop_engine_returns_empty() {
        let e = SttEngine::noop();
        assert_eq!(e.transcribe(&vec![0.1; 32_000]).await.unwrap(), "");
        assert_eq!(e.label(), "noop");
    }

    #[tokio::test]
    async fn sub_second_input_short_circuits() {
        // Even a Groq engine must not round-trip < 1s of audio.
        let e = SttEngine::groq("fake-key".into(), "whisper-large-v3-turbo".into(), &[]);
        assert_eq!(e.transcribe(&vec![0.1; 8_000]).await.unwrap(), "");
    }

    #[test]
    fn engine_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<SttEngine>();
    }

    // ---------- to_whisper_pcm ----------

    fn fmt(rate: u32, channels: u32) -> AudioFormat {
        AudioFormat {
            sample_rate: rate,
            channel_count: channels,
        }
    }

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

    #[test]
    fn stereo_48k_resamples_and_downmixes_in_one_pass() {
        // The real participant-audio path: 48 kHz stereo → 16 kHz mono.
        // L=1.0, R=-1.0 → averaged mono is 0.0 (a "drop one channel"
        // bug would leave ±1.0); 480 frames at 48k → ~160 at 16k.
        let frames = 480;
        let mut input = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            input.push(1.0_f32);
            input.push(-1.0_f32);
        }
        let out = to_whisper_pcm(&input, fmt(48_000, 2));
        let expected = frames * 16_000 / 48_000;
        assert!(
            (out.len() as i64 - expected as i64).abs() <= 1,
            "expected ~{expected} samples, got {}",
            out.len(),
        );
        assert!(out.iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn vocab_prompt_names_the_agent() {
        let p = vocab_prompt(&["yokota".to_string()]);
        assert!(
            p.contains("Yokota"),
            "prompt should carry the agent's name: {p}"
        );
        assert!(
            !p.contains("Eliza"),
            "no hardcoded Eliza when a name is given: {p}"
        );
    }

    #[test]
    fn vocab_prompt_includes_peers() {
        let p = vocab_prompt(&[
            "yokota".to_string(),
            "olive".to_string(),
            "olive".to_string(), // dupes collapse
            "".to_string(),      // blanks dropped
        ]);
        assert!(p.contains("Yokota") && p.contains("Olive"), "{p}");
        assert_eq!(p.matches("Olive").count(), 1, "{p}");
    }

    #[test]
    fn vocab_prompt_defaults_to_eliza() {
        let p = vocab_prompt(&[]);
        assert!(p.contains("Eliza"), "{p}");
    }

    #[test]
    fn multichannel_above_stereo_is_averaged() {
        // 6-channel (5.1-style): 3 channels at +1.0, 3 at -1.0 → mono
        // average 0.0. Averaging must not panic or index past the end.
        let frames = 1024;
        let mut input = Vec::with_capacity(frames * 6);
        for _ in 0..frames {
            for c in 0..6 {
                input.push(if c < 3 { 1.0 } else { -1.0 });
            }
        }
        let out = to_whisper_pcm(&input, fmt(16_000, 6));
        assert_eq!(out.len(), frames);
        assert!(out.iter().all(|s| s.abs() < 1e-3));
    }
}
