//! Visual question answering — letting Eliza *see*.
//!
//! `freeq-av` surfaces every participant's most recent video frame
//! ([`freeq_av::VideoHandle`]). When a participant asks a visual
//! question — "what's on my screen?", "look at this" — Eliza grabs that
//! frame, JPEG-encodes it, and sends it to a Groq vision model.

use std::io::Cursor;

use anyhow::{Context, Result};
use base64::Engine;
use iroh_live::media::format::VideoFrame;
use serde::Deserialize;

const VISION_SYSTEM: &str = "You are Eliza, an AI agent in a live voice \
call. A participant is sharing their screen or camera and has asked you \
about what you see. Answer their question from the image. Rules: answer \
in 1-3 short sentences — your reply is spoken aloud, so be brief and \
conversational. No markdown, no bullet points, no emoji. Never put URLs \
in your answer. If the image is unclear or doesn't show what they asked \
about, say so plainly.";

/// Phrases that mark a question as being about something Eliza should
/// *look at* rather than answer from knowledge or the transcript.
const VISUAL_CUES: &[&str] = &[
    "screen",
    "do you see",
    "can you see",
    "what do you see",
    "look at",
    "looking at",
    "this slide",
    "this image",
    "this picture",
    "this diagram",
    "this chart",
    "this graph",
    "what is this",
    "what's this",
    "describe this",
    "read this",
    "on camera",
    "my camera",
    "in the video",
    "am i showing",
    "i'm showing",
    "im showing",
];

/// Whether `question` is asking Eliza about something visual — so it
/// should be routed to the vision model with a video frame attached.
pub fn is_visual_question(question: &str) -> bool {
    let q = question.to_lowercase();
    VISUAL_CUES.iter().any(|cue| q.contains(cue))
}

/// Encode a decoded video frame as JPEG bytes. The frame's pixels are
/// converted to RGB (JPEG has no alpha channel) at quality suitable for
/// a vision model.
pub fn frame_to_jpeg(frame: &VideoFrame) -> Result<Vec<u8>> {
    let rgb = image::DynamicImage::ImageRgba8(frame.rgba_image().clone()).into_rgb8();
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
        .context("encoding video frame as JPEG")?;
    Ok(out)
}

#[derive(Deserialize)]
struct VisionResponse {
    choices: Vec<VisionChoice>,
}

#[derive(Deserialize)]
struct VisionChoice {
    message: VisionMessage,
}

#[derive(Deserialize)]
struct VisionMessage {
    #[serde(default)]
    content: String,
}

/// Answer `question` about `frame` with a Groq vision model. Returns the
/// description, trimmed and ready to speak.
pub async fn describe(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    question: &str,
    frame: &VideoFrame,
) -> Result<String> {
    let jpeg = frame_to_jpeg(frame)?;
    let data_uri = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&jpeg)
    );

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 320,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": VISION_SYSTEM },
            { "role": "user", "content": [
                { "type": "text", "text": question },
                { "type": "image_url", "image_url": { "url": data_uri } },
            ]},
        ],
    });

    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("groq vision request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("groq vision {status}: {err}");
    }
    let parsed: VisionResponse = resp.json().await.context("groq vision parse failed")?;
    let text = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("groq vision returned no description");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_visual_questions() {
        for q in [
            "what's on my screen",
            "can you see this diagram",
            "look at this",
            "what do you see",
            "describe this picture",
            "read this for me",
            "what am i showing you",
        ] {
            assert!(is_visual_question(q), "should be visual: {q:?}");
        }
    }

    #[test]
    fn ignores_non_visual_questions() {
        for q in [
            "what time is it",
            "summarize the call",
            "who said that",
            "what is the capital of France",
            "do you understand what I mean",
        ] {
            assert!(!is_visual_question(q), "should not be visual: {q:?}");
        }
    }

    #[test]
    fn visual_detection_is_case_insensitive() {
        assert!(is_visual_question("ELIZA WHAT IS THIS"));
        assert!(is_visual_question("Look At This Chart"));
    }

    #[test]
    fn frame_encodes_to_a_jpeg() {
        // A 16×16 opaque-grey RGBA frame round-trips to JPEG bytes.
        let px = vec![128u8; 16 * 16 * 4];
        let frame = VideoFrame::new_rgba(
            bytes::Bytes::from(px),
            16,
            16,
            std::time::Duration::ZERO,
        );
        let jpeg = frame_to_jpeg(&frame).expect("encode");
        assert!(jpeg.len() > 2);
        // JPEG SOI marker.
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }
}
