//! End-of-call summarization via the Anthropic Messages API.
//!
//! We send the full rolling transcript and ask Claude to produce a
//! short summary followed by an action-items list. Returns the
//! assembled string so the caller can post it as one or more PRIVMSGs.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[allow(dead_code)]
    #[serde(other)]
    Other,
}

const SYSTEM: &str = "You are a meeting note-taker. Given a raw \
transcript of a voice call (lines are `<speaker>: <utterance>`), \
produce a short markdown summary in this exact shape:\n\
\n\
**Summary**\n\
<2-4 sentences capturing what was discussed>\n\
\n\
**Action items**\n\
- <verb-led item> (@<owner if mentioned>)\n\
- ...\n\
\n\
If there are no action items, write `- (none)`. Do not invent facts \
that are not in the transcript. Do not address the reader. Do not \
summarize who attended unless they spoke.";

/// `transcript` is the joined utterance lines. `channel` is just for
/// log context. `model` is e.g. `claude-sonnet-4-5`.
pub async fn summarize(
    api_key: &str,
    model: &str,
    channel: &str,
    transcript: &str,
) -> Result<String> {
    summarize_against(
        "https://api.anthropic.com/v1/messages",
        api_key,
        model,
        channel,
        transcript,
    )
    .await
}

/// Internal variant that lets tests point at a local HTTP server.
/// Kept `pub(crate)` so the public surface stays narrow.
pub(crate) async fn summarize_against(
    url: &str,
    api_key: &str,
    model: &str,
    channel: &str,
    transcript: &str,
) -> Result<String> {
    let body = build_request_body(model, channel, transcript);

    let resp = reqwest::Client::new()
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("anthropic request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("anthropic {status}: {body}");
    }
    let bytes = resp
        .bytes()
        .await
        .context("anthropic response body read failed")?;
    let parsed: ApiResponse = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "anthropic response parse failed (body: {} bytes)",
            bytes.len()
        )
    })?;
    Ok(parsed
        .content
        .into_iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(""))
}

/// Build the Anthropic Messages API request body. Pulled out so tests
/// can pin its shape without needing a live HTTP round-trip.
pub(crate) fn build_request_body(
    model: &str,
    channel: &str,
    transcript: &str,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "system": SYSTEM,
        "messages": [{
            "role": "user",
            "content": format!("Channel: {channel}\n\nTranscript:\n{transcript}")
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    /// Stand up a single-shot HTTP/1.1 server that returns
    /// `status`/`body` to the first incoming request, then closes.
    /// Returns the URL the test should hit + a handle to the captured
    /// request body once it lands.
    async fn one_shot_server(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, Arc<Mutex<Option<Vec<u8>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let captured_w = captured.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Read until we've seen the headers + (optional) body.
            // Anthropic requests are tiny; read up to 64 KiB then
            // respond. We don't need full HTTP/1.1 framing.
            let mut buf = vec![0u8; 65_536];
            let mut total = 0;
            // Read until we've seen the end of headers AND consumed
            // any Content-Length body.
            let payload = loop {
                let n = stream.read(&mut buf[total..]).await.unwrap_or(0);
                if n == 0 {
                    break buf[..total].to_vec();
                }
                total += n;
                if let Some(pos) = find_crlf2(&buf[..total]) {
                    let headers = &buf[..pos];
                    let body_start = pos + 4;
                    let content_length = parse_content_length(headers).unwrap_or(0);
                    if total >= body_start + content_length {
                        break buf[..body_start + content_length].to_vec();
                    }
                }
                if total == buf.len() {
                    break buf[..total].to_vec();
                }
            };
            *captured_w.lock().await = Some(payload);
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                status_line,
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.ok();
            stream.shutdown().await.ok();
        });
        (format!("http://{addr}/v1/messages"), captured)
    }

    fn find_crlf2(b: &[u8]) -> Option<usize> {
        b.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn parse_content_length(headers: &[u8]) -> Option<usize> {
        let s = std::str::from_utf8(headers).ok()?;
        for line in s.split("\r\n") {
            if let Some(rest) = line
                .strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
            {
                return rest.trim().parse().ok();
            }
        }
        None
    }

    fn extract_body(req: &[u8]) -> Option<&[u8]> {
        let pos = req.windows(4).position(|w| w == b"\r\n\r\n")?;
        Some(&req[pos + 4..])
    }

    // ---------- request body shape ----------

    #[test]
    fn request_body_includes_channel_and_transcript() {
        let body = build_request_body("claude-sonnet-4-5", "#foo", "alice: hi\nbob: yo");
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["max_tokens"], 1024);
        // The system prompt must be sent so Claude knows the output
        // shape; without it the bot posts free-form text.
        assert!(
            body["system"]
                .as_str()
                .unwrap()
                .contains("meeting note-taker")
        );
        let content = body["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("Channel: #foo"));
        assert!(content.contains("alice: hi"));
        assert!(content.contains("bob: yo"));
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn request_body_handles_long_transcript_without_truncation() {
        // Known limitation: we don't currently truncate. Pin that —
        // if we add truncation later, this test fails loudly and the
        // change has to be intentional. The Anthropic API returns 400
        // when the prompt overflows the model window; the orchestrator
        // surfaces that error verbatim to the channel (see
        // `tests::body_propagates_4xx_body`).
        let transcript = "alice: hello\n".repeat(100_000);
        let body = build_request_body("claude-sonnet-4-5", "#foo", &transcript);
        let content = body["messages"][0]["content"].as_str().unwrap();
        // Should be at least as long as the input transcript.
        assert!(
            content.len() >= transcript.len(),
            "transcript was truncated"
        );
    }

    // ---------- HTTP round-trips ----------

    #[tokio::test]
    async fn happy_path_returns_concatenated_text_blocks() {
        let response =
            r#"{"content":[{"type":"text","text":"Hello "},{"type":"text","text":"world"}]}"#;
        let (url, captured) = one_shot_server("200 OK", response).await;
        let out = summarize_against(&url, "sk-test", "claude-x", "#c", "alice: hi")
            .await
            .unwrap();
        assert_eq!(out, "Hello world");

        // Inspect the captured request: headers + body sanity check.
        let req = captured.lock().await.clone().unwrap();
        let req_str = String::from_utf8_lossy(&req);
        assert!(
            req_str.contains("x-api-key: sk-test"),
            "missing api key header"
        );
        assert!(req_str.contains("anthropic-version: 2023-06-01"));
        assert!(req_str.contains("content-type: application/json"));
        let body = extract_body(&req).unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body_json["model"], "claude-x");
    }

    #[tokio::test]
    async fn response_with_no_text_content_block_yields_empty_string() {
        // The model occasionally returns a tool-use-only response with
        // no text block. We don't crash on it — we return "" and the
        // caller posts the "session ended" line on its own.
        let response = r#"{"content":[{"type":"tool_use","id":"x","name":"y","input":{}}]}"#;
        let (url, _captured) = one_shot_server("200 OK", response).await;
        let out = summarize_against(&url, "sk", "m", "#c", "t").await.unwrap();
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn response_with_empty_content_array_yields_empty_string() {
        let response = r#"{"content":[]}"#;
        let (url, _captured) = one_shot_server("200 OK", response).await;
        let out = summarize_against(&url, "sk", "m", "#c", "t").await.unwrap();
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn anthropic_4xx_bubbles_body_in_error() {
        // The 4xx body is the only thing telling the operator WHY the
        // request was rejected (overflow, bad model name, expired key).
        // It MUST be preserved in the error chain so the IRC orchestrator
        // can post it back to the channel.
        let response = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt too long"}}"#;
        let (url, _captured) = one_shot_server("400 Bad Request", response).await;
        let err = summarize_against(&url, "sk", "m", "#c", "t")
            .await
            .err()
            .expect("expected 4xx error");
        let s = format!("{err:#}");
        assert!(s.contains("400"), "missing status: {s}");
        assert!(s.contains("prompt too long"), "missing body: {s}");
    }

    #[tokio::test]
    async fn anthropic_5xx_bubbles_body_in_error() {
        let response = r#"{"error":"overloaded"}"#;
        let (url, _captured) = one_shot_server("503 Service Unavailable", response).await;
        let err = summarize_against(&url, "sk", "m", "#c", "t")
            .await
            .err()
            .expect("expected 5xx error");
        let s = format!("{err:#}");
        assert!(s.contains("503"), "missing status: {s}");
        assert!(s.contains("overloaded"), "missing body: {s}");
    }

    #[tokio::test]
    async fn non_json_2xx_body_returns_parse_error() {
        // Some proxies (corporate MITM, gateways) silently swap a 200
        // for an HTML "are you sure" page. We must not silently treat
        // that as a successful empty summary — surface a parse error.
        let response = r#"<html>oops</html>"#;
        let (url, _captured) = one_shot_server("200 OK", response).await;
        let err = summarize_against(&url, "sk", "m", "#c", "t")
            .await
            .err()
            .expect("expected parse error");
        let s = format!("{err:#}");
        assert!(s.contains("parse failed"), "got: {s}");
    }
}
