//! AI image generation for eliza's scene cards.
//!
//! The bot asks an image model for an illustration of an answer's topic;
//! the result is composited behind the scene card as a backdrop. Image
//! generation is slow (~10-20s), so callers run [`generate`] off the
//! answer path — the spoken reply never waits on an image.

use anyhow::{Context, Result, bail};
use base64::Engine;

/// Which image API to call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProvider {
    OpenAi,
    Gemini,
}

impl ImageProvider {
    /// Parse the `--image-provider` flag. Unknown values fall back to
    /// OpenAI.
    pub fn parse(s: &str) -> ImageProvider {
        match s.trim().to_lowercase().as_str() {
            "gemini" | "google" => ImageProvider::Gemini,
            _ => ImageProvider::OpenAi,
        }
    }

    /// Environment variables that may hold this provider's API key, in
    /// priority order.
    pub fn key_vars(self) -> &'static [&'static str] {
        match self {
            ImageProvider::OpenAi => &["OPENAI_API_KEY"],
            ImageProvider::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        }
    }
}

/// Configuration for the AI image-generation fallback used when
/// Wikipedia has no image for a topic.
#[derive(Clone, Debug)]
pub struct AiImageConfig {
    pub provider: ImageProvider,
    pub model: String,
    pub key: String,
}

/// Wrap a topic in a fixed style so generated images sit well as a
/// darkened card backdrop — atmospheric, and crucially text-free.
fn styled_prompt(topic: &str) -> String {
    format!(
        "A clean, modern editorial illustration: {topic}. Cinematic and \
         atmospheric, rich but moody colour, soft depth of field. \
         Absolutely no text, no words, no letters, no captions, no \
         watermark, no user interface. Wide establishing composition."
    )
}

/// Get a backdrop image for `query`. Tries Wikipedia first (fast — about
/// a second); on a miss or error falls back to AI generation (slow —
/// 10-20s) when `ai` is configured. Returns raw encoded image bytes.
pub async fn fetch(
    client: &reqwest::Client,
    query: &str,
    ai: Option<&AiImageConfig>,
) -> Result<Vec<u8>> {
    match wikipedia(client, query).await {
        Ok(Some(bytes)) => {
            tracing::info!(%query, "backdrop: wikipedia hit");
            return Ok(bytes);
        }
        Ok(None) => tracing::info!(%query, "backdrop: no wikipedia image, trying AI"),
        Err(e) => tracing::warn!(%query, error = %e, "backdrop: wikipedia lookup failed"),
    }
    match ai {
        Some(ai) => {
            let bytes = generate(client, ai.provider, &ai.model, &ai.key, query).await?;
            tracing::info!(%query, "backdrop: AI-generated");
            Ok(bytes)
        }
        None => bail!("no backdrop: wikipedia had nothing and AI generation is off"),
    }
}

/// Look up a lead image for `query` on Wikipedia. `Ok(None)` means the
/// topic exists but has no usable image (or no page matched).
async fn wikipedia(client: &reqwest::Client, query: &str) -> Result<Option<Vec<u8>>> {
    const UA: &str = "freeq-utopia/0.1 (https://freeq.at; utopia agent)";
    let resp = client
        .get("https://en.wikipedia.org/w/api.php")
        .header(reqwest::header::USER_AGENT, UA)
        .query(&[
            ("action", "query"),
            ("format", "json"),
            ("formatversion", "2"),
            ("generator", "search"),
            ("gsrsearch", query),
            ("gsrlimit", "1"),
            ("gsrnamespace", "0"),
            ("prop", "pageimages"),
            ("piprop", "thumbnail"),
            ("pithumbsize", "1024"),
        ])
        .send()
        .await
        .context("wikipedia query failed")?;
    if !resp.status().is_success() {
        bail!("wikipedia query {}", resp.status());
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .context("wikipedia: response was not JSON")?;
    let Some(url) = json["query"]["pages"][0]["thumbnail"]["source"].as_str() else {
        return Ok(None);
    };
    let img = client
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .context("wikipedia image fetch failed")?;
    if !img.status().is_success() {
        bail!("wikipedia image fetch {}", img.status());
    }
    let bytes = img.bytes().await.context("wikipedia image body")?;
    Ok(Some(bytes.to_vec()))
}

/// Generate an illustration for `topic` via an AI image model. Returns
/// raw encoded image bytes (PNG). Slow — call off the hot path.
async fn generate(
    client: &reqwest::Client,
    provider: ImageProvider,
    model: &str,
    api_key: &str,
    topic: &str,
) -> Result<Vec<u8>> {
    let prompt = styled_prompt(topic);
    match provider {
        ImageProvider::OpenAi => openai(client, model, api_key, &prompt).await,
        ImageProvider::Gemini => gemini(client, model, api_key, &prompt).await,
    }
}

/// First `n` characters of `s` — keeps an error body from flooding logs.
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn openai(
    client: &reqwest::Client,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> Result<Vec<u8>> {
    // Low quality at 1024² is plenty for a darkened backdrop and keeps
    // generation latency down (~15s vs ~25s).
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "size": "1024x1024",
        "quality": "low",
        "n": 1,
    });
    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("openai image request failed")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("openai image {status}: {}", head(&text, 280));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).context("openai image: response was not JSON")?;
    let b64 = json["data"][0]["b64_json"]
        .as_str()
        .context("openai image: no b64_json in response")?;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("openai image: bad base64")
}

async fn gemini(
    client: &reqwest::Client,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> Result<Vec<u8>> {
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let body = serde_json::json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": { "responseModalities": ["IMAGE"] },
    });
    let resp = client
        .post(&url)
        .query(&[("key", api_key)])
        .json(&body)
        .send()
        .await
        .context("gemini image request failed")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("gemini image {status}: {}", head(&text, 280));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).context("gemini image: response was not JSON")?;
    let parts = json["candidates"][0]["content"]["parts"]
        .as_array()
        .context("gemini image: no parts in response")?;
    for part in parts {
        if let Some(data) = part["inlineData"]["data"].as_str() {
            return base64::engine::general_purpose::STANDARD
                .decode(data)
                .context("gemini image: bad base64");
        }
    }
    bail!("gemini image: no image part in response")
}

/// Decode generated image `bytes`, crop to 16:9, downscale, and
/// re-encode as a JPEG `data:` URI ready to embed in an SVG `<image>`.
/// CPU-bound — run on a blocking thread.
pub fn to_data_uri(bytes: &[u8]) -> Result<String> {
    let img = image::load_from_memory(bytes).context("decode generated image")?;
    let img = img.resize_to_fill(768, 432, image::imageops::FilterType::Lanczos3);
    let mut jpeg = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut jpeg),
        image::ImageFormat::Jpeg,
    )
    .context("re-encode generated image as jpeg")?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parse_defaults_to_openai() {
        assert_eq!(ImageProvider::parse("gemini"), ImageProvider::Gemini);
        assert_eq!(ImageProvider::parse("GOOGLE"), ImageProvider::Gemini);
        assert_eq!(ImageProvider::parse("openai"), ImageProvider::OpenAi);
        assert_eq!(ImageProvider::parse("nonsense"), ImageProvider::OpenAi);
    }

    #[test]
    fn styled_prompt_keeps_topic_and_bans_text() {
        let p = styled_prompt("a lighthouse in a storm");
        assert!(p.contains("a lighthouse in a storm"));
        assert!(p.to_lowercase().contains("no text"));
    }

    #[test]
    fn to_data_uri_round_trips_a_real_image() {
        // Encode a small PNG, then run it through the pipeline.
        let src = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            32,
            32,
            image::Rgba([20, 80, 160, 255]),
        ));
        let mut png = Vec::new();
        src.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let uri = to_data_uri(&png).expect("pipeline must succeed");
        assert!(uri.starts_with("data:image/jpeg;base64,"));
        // The base64 payload must decode back to a valid JPEG.
        let payload = uri.trim_start_matches("data:image/jpeg;base64,");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (768, 432));
    }

    #[test]
    fn to_data_uri_rejects_garbage() {
        assert!(to_data_uri(b"not an image").is_err());
    }
}
