//! Text-to-speech via the ElevenLabs API.
//!
//! `synthesize` turns a string into mono f32 PCM. We request
//! `pcm_48000` output — raw, headerless, 16-bit signed-LE mono at
//! 48 kHz — which is exactly the bot's broadcast rate, so the speak
//! path needs no resampling and no container decoding at all.

use anyhow::{Context, Result};

/// Decoded TTS audio: mono f32 PCM plus its sample rate.
pub struct TtsAudio {
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
}

/// ElevenLabs `pcm_48000` is raw 16-bit signed-LE mono at 48 kHz.
pub const ELEVENLABS_PCM_RATE: u32 = 48_000;

/// Synthesize `text` with ElevenLabs. `voice_id` is the ElevenLabs
/// voice (e.g. the "Utopia" voice). `model` is e.g.
/// `eleven_turbo_v2_5`. Voice tuning matches the avatar app's
/// settings. Returns mono f32 PCM at [`ELEVENLABS_PCM_RATE`].
pub async fn synthesize(
    client: &reqwest::Client,
    api_key: &str,
    voice_id: &str,
    model: &str,
    text: &str,
) -> Result<TtsAudio> {
    let body = serde_json::json!({
        "text": text,
        "model_id": model,
        "voice_settings": {
            "stability": 0.7,
            "similarity_boost": 0.75,
            // 0.85 (the avatar app's calm default) sped up ~20% — the
            // bot reads quick answers, not narration.
            "speed": 1.02,
        },
    });
    let url = format!(
        "https://api.elevenlabs.io/v1/text-to-speech/{voice_id}?output_format=pcm_48000"
    );
    let resp = client
        .post(&url)
        .header("xi-api-key", api_key)
        .json(&body)
        .send()
        .await
        .context("elevenlabs TTS request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("elevenlabs TTS {status}: {err}");
    }
    let bytes = resp.bytes().await.context("reading elevenlabs TTS body")?;
    Ok(TtsAudio {
        pcm: decode_pcm_s16le(&bytes),
        sample_rate: ELEVENLABS_PCM_RATE,
    })
}

/// Decode raw 16-bit signed little-endian PCM into f32 `[-1, 1]`. A
/// trailing odd byte (shouldn't happen for valid PCM) is ignored.
pub(crate) fn decode_pcm_s16le(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

/// Encode mono f32 PCM as a 16-bit WAV byte stream. Used only to dump
/// the exact audio the bot is about to speak, so a "static" report can
/// be bisected: a clean dumped WAV proves the static is introduced
/// downstream (Opus encode / transport / playout), not in TTS.
pub fn encode_wav(pcm: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_raw_s16le_pcm() {
        // 0, +half, -half, +max, -max
        let mut raw = Vec::new();
        for s in [0i16, 16384, -16384, 32767, -32768] {
            raw.extend_from_slice(&s.to_le_bytes());
        }
        let pcm = decode_pcm_s16le(&raw);
        assert_eq!(pcm.len(), 5);
        assert!((pcm[0] - 0.0).abs() < 1e-6);
        assert!((pcm[1] - 0.5).abs() < 1e-3);
        assert!((pcm[2] + 0.5).abs() < 1e-3);
        assert!(pcm[3] > 0.99 && pcm[3] <= 1.0);
        assert!((pcm[4] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn raw_pcm_ignores_trailing_odd_byte() {
        // 3 bytes → one i16 sample + a dangling byte that must be dropped.
        let pcm = decode_pcm_s16le(&[0x00, 0x40, 0x7f]);
        assert_eq!(pcm.len(), 1);
        assert!((pcm[0] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn raw_pcm_empty_input() {
        assert!(decode_pcm_s16le(&[]).is_empty());
    }

    #[test]
    fn encode_wav_has_valid_header_and_round_trips() {
        let pcm = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let wav = encode_wav(&pcm, 48_000);
        // 44-byte header + 2 bytes per sample.
        assert_eq!(wav.len(), 44 + pcm.len() * 2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        // Sample rate (offset 24) round-trips.
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 48_000);
        // Decoding the data section back yields the originals.
        let decoded = decode_pcm_s16le(&wav[44..]);
        for (a, b) in pcm.iter().zip(&decoded) {
            assert!((a - b).abs() < 1.0 / 32767.0, "{a} vs {b}");
        }
    }
}
