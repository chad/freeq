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

/// Async STT engine. Held in an `Arc` and shared across per-participant
/// tap tasks.
pub enum SttEngine {
    /// Hosted Groq transcription. `model` is e.g.
    /// `whisper-large-v3-turbo`.
    Groq {
        client: reqwest::Client,
        api_key: String,
        model: String,
    },
    /// Local whisper.cpp (feature-gated).
    #[cfg(feature = "stt")]
    Local(Arc<imp::Whisper>),
    /// No STT — every window transcribes to "".
    Noop,
}

impl SttEngine {
    /// Construct a Groq-backed engine.
    pub fn groq(api_key: String, model: String) -> Self {
        SttEngine::Groq {
            client: reqwest::Client::new(),
            api_key,
            model,
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
            SttEngine::Groq { client, api_key, model } => {
                groq_transcribe(client, api_key, model, pcm_16k_mono).await
            }
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

async fn groq_transcribe(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
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
        // A light prompt nudges the model away from emitting filler
        // for near-silent windows.
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
            let ctx = WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                .context("WhisperContext::new failed; is the model path correct?")?;
            Ok(Self { ctx: Mutex::new(ctx) })
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
        let e = SttEngine::groq("fake-key".into(), "whisper-large-v3-turbo".into());
        assert_eq!(e.transcribe(&vec![0.1; 8_000]).await.unwrap(), "");
    }

    #[test]
    fn engine_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<SttEngine>();
    }
}
