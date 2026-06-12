//! Streaming speech-to-text via Deepgram's websocket API.
//!
//! Per-participant: open one persistent websocket to
//! `wss://api.deepgram.com/v1/listen`, push raw PCM frames in as they
//! arrive, get finalised transcripts back on the same socket as soon
//! as Deepgram's endpointing model decides the utterance is done.
//!
//! Why this beats the local VAD + batched-Whisper path: the local
//! segmenter waits a hard 600 ms of silence to call an utterance "done"
//! before sending it for transcription, and the round-trip to Groq
//! Whisper adds another 300–500 ms. Deepgram does endpointing
//! server-side at ~200–300 ms and is already mid-transcription before
//! the speaker stops, so the first text we see arrives within a
//! single-digit-hundred ms of the end of speech instead of a full
//! second-plus.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt, stream::SplitSink, stream::SplitStream};
use iroh_live::media::format::AudioFormat;
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone)]
pub struct StreamingSttConfig {
    pub api_key: String,
    pub model: String,
    /// PCM sample rate. Deepgram needs this in the URL so it doesn't
    /// have to infer; we pass through whatever the source provides
    /// (usually 48 kHz on the freeq side).
    pub sample_rate: u32,
}

impl StreamingSttConfig {
    pub fn nova_3(api_key: String, sample_rate: u32) -> Self {
        Self {
            api_key,
            model: "nova-3".to_string(),
            sample_rate,
        }
    }
}

/// Connect a participant-scoped stream. Returns a (writer, reader)
/// split — the caller feeds PCM into the writer, reads finalised
/// transcripts off the reader.
pub async fn connect(
    cfg: &StreamingSttConfig,
) -> Result<(SplitSink<WsStream, Message>, SplitStream<WsStream>)> {
    let url = format!(
        "wss://api.deepgram.com/v1/listen\
         ?model={model}\
         &encoding=linear16\
         &sample_rate={rate}\
         &channels=1\
         &punctuate=true\
         &smart_format=true\
         &interim_results=false\
         &endpointing=300\
         &vad_events=false",
        model = cfg.model,
        rate = cfg.sample_rate,
    );
    let url_for_err = url.clone();
    let mut req = url
        .into_client_request()
        .with_context(|| format!("building deepgram request: {url_for_err}"))?;
    req.headers_mut().insert(
        "Authorization",
        format!("Token {}", cfg.api_key)
            .parse()
            .context("Token header value")?,
    );
    let (ws, _resp) = connect_async(req)
        .await
        .context("deepgram websocket connect")?;
    let (writer, reader) = ws.split();
    Ok((writer, reader))
}

/// Downmix to mono + convert f32 [-1,1] to little-endian 16-bit PCM —
/// the shape Deepgram wants when `encoding=linear16`. Sanitises
/// NaN/∞ samples to 0 the same way `to_whisper_pcm` does.
pub fn f32_to_mono_i16le(samples: &[f32], format: &AudioFormat) -> Vec<u8> {
    let channels = format.channel_count.max(1) as usize;
    if samples.is_empty() {
        return Vec::new();
    }
    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let mut sum = 0.0f32;
        for c in 0..channels {
            let s = samples[f * channels + c];
            sum += if s.is_finite() { s } else { 0.0 };
        }
        let mono = sum / channels as f32;
        let i = (mono.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&i.to_le_bytes());
    }
    out
}

/// Deepgram response envelope — we only care about the `Results`
/// variant's `is_final` transcripts.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum DeepgramResponse {
    Results(ResultsBody),
    /// Other variants (Metadata, SpeechStarted, UtteranceEnd, Error) —
    /// we don't react to them today but tag-based deserialize keeps
    /// the stream parse-able without dropping the connection.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ResultsBody {
    channel: ResultsChannel,
    is_final: bool,
}

#[derive(Debug, Deserialize)]
struct ResultsChannel {
    alternatives: Vec<ResultsAlt>,
}

#[derive(Debug, Deserialize)]
struct ResultsAlt {
    #[serde(default)]
    transcript: String,
}

/// Parse one inbound message; return Some(text) on a finalised
/// utterance with non-empty text, None for any other message.
pub fn parse_final_transcript(text: &str) -> Option<String> {
    let resp: DeepgramResponse = serde_json::from_str(text).ok()?;
    let DeepgramResponse::Results(body) = resp else {
        return None;
    };
    if !body.is_final {
        return None;
    }
    let alt = body.channel.alternatives.into_iter().next()?;
    let t = alt.transcript.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// Send a CloseStream control frame so Deepgram returns final
/// transcripts and closes cleanly.
pub async fn close_stream(writer: &mut SplitSink<WsStream, Message>) -> Result<()> {
    writer
        .send(Message::Text(
            r#"{"type":"CloseStream"}"#.to_string().into(),
        ))
        .await
        .context("sending CloseStream")
}

/// Send a chunk of raw little-endian 16-bit PCM. Wraps the binary
/// frame so the caller doesn't need to depend on tungstenite's types
/// directly.
pub async fn send_pcm(writer: &mut SplitSink<WsStream, Message>, pcm_le: Vec<u8>) -> Result<()> {
    if pcm_le.is_empty() {
        return Ok(());
    }
    writer
        .send(Message::Binary(Bytes::from(pcm_le)))
        .await
        .map_err(|e| anyhow!("deepgram send: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_finalised_text() {
        let raw = r#"{"type":"Results","channel":{"alternatives":[{"transcript":"hello there"}]},"is_final":true,"speech_final":true,"duration":1.2}"#;
        assert_eq!(parse_final_transcript(raw).as_deref(), Some("hello there"));
    }

    #[test]
    fn drops_partial_results() {
        let raw = r#"{"type":"Results","channel":{"alternatives":[{"transcript":"hello"}]},"is_final":false,"speech_final":false}"#;
        assert!(parse_final_transcript(raw).is_none());
    }

    #[test]
    fn drops_empty_finalised() {
        let raw = r#"{"type":"Results","channel":{"alternatives":[{"transcript":""}]},"is_final":true,"speech_final":true}"#;
        assert!(parse_final_transcript(raw).is_none());
    }

    #[test]
    fn ignores_other_message_types() {
        let raw = r#"{"type":"Metadata","request_id":"abc"}"#;
        assert!(parse_final_transcript(raw).is_none());
        let raw = r#"{"type":"SpeechStarted","channel":[0]}"#;
        assert!(parse_final_transcript(raw).is_none());
    }

    #[test]
    fn pcm_conversion_clamps_and_packs() {
        let samples = [0.0_f32, 1.0, -1.0, 0.5, f32::NAN, f32::INFINITY];
        let fmt = AudioFormat {
            sample_rate: 48_000,
            channel_count: 1,
        };
        let bytes = f32_to_mono_i16le(&samples, &fmt);
        assert_eq!(bytes.len(), 12); // 6 i16 little-endian samples
        // 0.0 → 0
        assert_eq!(&bytes[0..2], &[0x00, 0x00]);
        // 1.0 → 32767 (0x7FFF)
        assert_eq!(&bytes[2..4], &[0xFF, 0x7F]);
        // -1.0 → -32767 (0x8001)
        assert_eq!(&bytes[4..6], &[0x01, 0x80]);
        // 0.5 → 16383 (0x3FFF)
        assert_eq!(&bytes[6..8], &[0xFF, 0x3F]);
        // NaN, INFINITY → 0
        assert_eq!(&bytes[8..10], &[0x00, 0x00]);
        assert_eq!(&bytes[10..12], &[0x00, 0x00]);
    }

    #[test]
    fn pcm_downmixes_stereo() {
        // Two interleaved frames of stereo, each summing to 0.5 mono.
        let samples = [0.5_f32, 0.5, -0.25, 0.75];
        let fmt = AudioFormat {
            sample_rate: 48_000,
            channel_count: 2,
        };
        let bytes = f32_to_mono_i16le(&samples, &fmt);
        // Two i16 frames out.
        assert_eq!(bytes.len(), 4);
        // (0.5 + 0.5) / 2 = 0.5 → 16383
        let f1 = i16::from_le_bytes([bytes[0], bytes[1]]);
        assert!((15_000..=17_000).contains(&f1));
        // (-0.25 + 0.75) / 2 = 0.25 → ~8192
        let f2 = i16::from_le_bytes([bytes[2], bytes[3]]);
        assert!((7_000..=9_000).contains(&f2));
    }
}
