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

const VISION_SYSTEM: &str = "You are an AI agent in a live voice \
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
    "how many fingers",
    "see me",
    "see us",
    "watch me",
    "watch this",
];

/// Weaker cues that only count when the asker actually has a live
/// video frame. "How many fingers am I holding up?" matches none of
/// the strong cues — and a missed route means the text model fields a
/// visual question and denies it can see at all. With a frame in hand
/// a false positive is cheap (the vision model sees the frame and the
/// question, and says when the image doesn't show what was asked), so
/// the bar drops.
const VISUAL_CUES_WITH_FRAME: &[&str] = &[
    "finger",
    "am i holding",
    "i'm holding",
    "im holding",
    "holding up",
    "am i wearing",
    "i'm wearing",
    "im wearing",
    "do i look",
    "do we look",
    "look like",
    "my face",
    "my hand",
    "my hair",
    "my shirt",
    "behind me",
    "in front of me",
    "next to me",
    "around me",
    "my room",
    "my desk",
    "my environment",
    "what color",
    "what colour",
    "count these",
    "count them",
    "count my",
];

/// Whether `question` is asking Eliza about something visual — so it
/// should be routed to the vision model with a video frame attached.
pub fn is_visual_question(question: &str) -> bool {
    let q = question.to_lowercase();
    VISUAL_CUES.iter().any(|cue| q.contains(cue))
}

/// Looser visual-question test for when the asker has a live frame:
/// strong cues plus phrasings about their own appearance/surroundings
/// ("how many fingers am I holding up", "what am I wearing").
pub fn is_visual_question_with_frame(question: &str) -> bool {
    if is_visual_question(question) {
        return true;
    }
    let q = question.to_lowercase();
    VISUAL_CUES_WITH_FRAME.iter().any(|cue| q.contains(cue))
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

/// Encode a video frame as a `data:image/jpeg;base64,…` URI — the form
/// both the Groq vision API and the video tile's PiP overlay want.
pub fn frame_to_jpeg_data_uri(frame: &VideoFrame) -> Result<String> {
    let jpeg = frame_to_jpeg(frame)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&jpeg)
    ))
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

/// Answer `question` about an image with a Groq vision model. Takes
/// the image as a `data:image/jpeg;base64,…` URI ([`frame_to_jpeg_data_uri`])
/// — pre-encoded so the caller can also pin the same URI on the video
/// tile as a PiP without paying for a second JPEG encode.
pub async fn describe(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    question: &str,
    image_data_uri: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 320,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": VISION_SYSTEM },
            { "role": "user", "content": [
                { "type": "text", "text": question },
                { "type": "image_url", "image_url": { "url": image_data_uri } },
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
    fn frame_lowers_the_routing_bar() {
        // The Yokota miss: no strong cue, but obviously visual when a
        // camera is live.
        for q in [
            "how many fingers am I holding up",
            "what am I wearing today",
            "do I look tired",
            "what's behind me",
            "what color is my shirt",
        ] {
            assert!(is_visual_question_with_frame(q), "frame-visual: {q:?}");
        }
        // Still not visual even with a frame — plain knowledge/transcript.
        for q in [
            "what time is it",
            "summarize the call",
            "what is the capital of France",
        ] {
            assert!(!is_visual_question_with_frame(q), "not visual: {q:?}");
        }
        // Loose cues alone never fire without a frame ("how many
        // fingers" is a STRONG cue — frameless it earns the "turn on
        // your camera" hint — so probe a loose-only phrasing here).
        assert!(!is_visual_question("what am I wearing today"));
        assert!(!is_visual_question("do I look tired"));
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
