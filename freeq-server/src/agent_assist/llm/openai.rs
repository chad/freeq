//! OpenAI-compatible chat-completions client.
//!
//! Works against any HTTP endpoint that speaks OpenAI's
//! `/chat/completions` shape — which today includes:
//!
//! - OpenAI proper (`api.openai.com/v1`)
//! - Together, Fireworks, Anyscale, Groq, DeepInfra, Replicate, etc.
//! - `vllm serve` (`localhost:8000/v1` or behind a load balancer)
//! - llama.cpp's `server` binary
//! - Ollama (with `/v1` enabled — the default in recent versions)
//! - HuggingFace TGI's OpenAI-compat endpoint
//! - LMDeploy, MLC-LLM, etc.
//!
//! That single shape covers essentially every open-source serving
//! framework, which is the user's target use case ("experiment with
//! open source models").
//!
//! ## Knobs
//!
//! - `base_url` — the prefix the client appends `/chat/completions` to.
//!   Local: `http://localhost:11434/v1` (Ollama). OpenAI:
//!   `https://api.openai.com/v1`. Many OSS endpoints accept the empty
//!   model name; check yours.
//! - `model` — passed through verbatim. We do not validate model names.
//! - `api_key` — sent as `Authorization: Bearer <api_key>` if present.
//!   Many local servers ignore this; some require a placeholder.
//! - `timeout` — hard ceiling on the request. Default 8s. The router
//!   maps a timeout to `INTENT_UNCLEAR`, never a 500.
//!
//! ## Output handling
//!
//! We ask for `response_format: {"type": "json_object"}` (OpenAI native;
//! some OSS servers honor it, others ignore it). If the response is
//! valid JSON we parse it. If not, we make one tolerant attempt to
//! extract a JSON object from inside the text (some models still wrap
//! their JSON in code fences); if that fails we return
//! [`LlmError::Unparseable`] which the router downgrades to
//! `INTENT_UNCLEAR`.

use super::prompts::{system_prompt, user_envelope};
use super::{BoxFuture, ClassificationContext, LlmError, LlmProvider, ToolIntent};
use crate::agent_assist::types::FactBundle;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Concrete provider hitting any OpenAI-compatible chat-completions
/// endpoint.
pub struct OpenAiCompatible {
    name: String,
    base_url: String,
    api_key: Option<String>,
    model: String,
    timeout: Duration,
    http: reqwest::Client,
}

impl OpenAiCompatible {
    /// `base_url` is the prefix to which `/chat/completions` is appended,
    /// e.g. `https://api.openai.com/v1` or `http://127.0.0.1:11434/v1`.
    /// `display_name` is shown in audit logs (e.g. `"openai:gpt-4o-mini"`,
    /// `"ollama:llama3.1:8b"`).
    pub fn new(
        display_name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            name: display_name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            model: model.into(),
            timeout,
            http: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    async fn chat(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system,
                },
                ChatMessage {
                    role: "user",
                    content: user,
                },
            ],
            temperature: 0.0,
            response_format: Some(ResponseFormat {
                kind: "json_object",
            }),
            max_tokens: Some(512),
        };

        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.http.post(&url).json(&body);
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                LlmError::Timeout(self.timeout.as_secs())
            } else {
                LlmError::Transport(format!("{e}"))
            }
        })?;
        if !resp.status().is_success() {
            return Err(LlmError::Transport(format!(
                "HTTP {} from {}",
                resp.status(),
                url
            )));
        }
        let body: ChatResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::Unparseable(format!("response body: {e}")))?;
        body.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| LlmError::Unparseable("no choices in response".into()))
    }
}

impl LlmProvider for OpenAiCompatible {
    fn name(&self) -> &str {
        &self.name
    }

    fn classify_intent<'a>(
        &'a self,
        message: &'a str,
        ctx: &'a ClassificationContext,
    ) -> BoxFuture<'a, Result<Option<ToolIntent>, LlmError>> {
        Box::pin(async move {
            let sys = system_prompt(ctx);
            let usr = user_envelope(message);
            let raw = self.chat(&sys, &usr).await?;
            let intent = parse_intent_lenient(&raw)?;
            // The model is allowed to say "I don't know" by setting
            // tool to null. Surface that as Ok(None).
            Ok(intent.filter(|i| !i.tool.is_empty() && i.tool != "null"))
        })
    }

    fn refine_summary<'a>(
        &'a self,
        bundle: &'a FactBundle,
    ) -> BoxFuture<'a, Result<Option<String>, LlmError>> {
        Box::pin(async move {
            // Conservative MVP: skip refinement to keep the wire
            // summary deterministic. Implementations can be added
            // later by sending bundle.safe_facts (only) to chat() and
            // returning Ok(Some(reply)).
            let _ = bundle;
            Ok(None)
        })
    }
}

/// Test/sibling alias: `first_balanced_object` for use from
/// neighbouring modules (e.g. the mock provider) without making the
/// internal name part of the crate's public surface.
pub(super) fn first_balanced_object_for_test(s: &str) -> Option<&str> {
    first_balanced_object(s)
}

// ─── Wire types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

// ─── Lenient JSON extraction ─────────────────────────────────────────────

/// First try strict JSON parsing. If that fails, try to recover the
/// first balanced `{…}` substring (handles models that wrap JSON in
/// code fences or prose despite being asked not to). On any failure
/// this returns `Ok(None)` rather than `Err` so the caller falls
/// through to `INTENT_UNCLEAR` instead of bubbling a 500.
fn parse_intent_lenient(raw: &str) -> Result<Option<ToolIntent>, LlmError> {
    if let Ok(intent) = serde_json::from_str::<ToolIntent>(raw) {
        return Ok(Some(intent));
    }
    if let Some(json) = first_balanced_object(raw)
        && let Ok(intent) = serde_json::from_str::<ToolIntent>(json)
    {
        return Ok(Some(intent));
    }
    Ok(None)
}

/// Find the first complete `{…}` substring, accounting for nested
/// braces and string-literal escaping. Naive but covers the common
/// "model returned JSON inside ```json fences" case.
///
/// Re-exported via the test helper alias below so the mock provider
/// can use the same extraction without duplicating the parser.
pub(super) fn first_balanced_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|b| *b == b'{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return std::str::from_utf8(&bytes[start..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_strict_json() {
        let raw = r#"{"tool": "validate_client_config", "args": {"x": 1}, "confidence": "high", "summary": "ok"}"#;
        let parsed = parse_intent_lenient(raw).unwrap().unwrap();
        assert_eq!(parsed.tool, "validate_client_config");
        assert_eq!(parsed.args["x"], 1);
    }

    #[test]
    fn recovers_json_from_fenced_block() {
        let raw = "```json\n{\"tool\":\"diagnose_sync\",\"args\":{\"account\":\"did:plc:x\"},\"confidence\":\"medium\",\"summary\":\"\"}\n```";
        let parsed = parse_intent_lenient(raw).unwrap().unwrap();
        assert_eq!(parsed.tool, "diagnose_sync");
    }

    #[test]
    fn recovers_json_with_leading_prose() {
        let raw = "Sure! Here is the JSON:\n{\"tool\":\"validate_client_config\",\"args\":{},\"confidence\":\"low\",\"summary\":\"\"}\nLet me know if you need more.";
        let parsed = parse_intent_lenient(raw).unwrap().unwrap();
        assert_eq!(parsed.tool, "validate_client_config");
    }

    #[test]
    fn returns_none_on_garbage() {
        let raw = "I cannot help with that.";
        assert!(parse_intent_lenient(raw).unwrap().is_none());
    }

    #[test]
    fn first_balanced_object_handles_nested_braces() {
        let raw = r#"prefix {"a": {"b": 1}} suffix"#;
        let extracted = first_balanced_object(raw).unwrap();
        assert_eq!(extracted, r#"{"a": {"b": 1}}"#);
    }

    #[test]
    fn first_balanced_object_ignores_braces_inside_strings() {
        let raw = r#"{"text": "this has } in it", "n": 1}"#;
        let extracted = first_balanced_object(raw).unwrap();
        assert_eq!(extracted, raw);
    }

    #[test]
    fn first_balanced_object_handles_escaped_quotes() {
        let raw = r#"{"text": "she said \"hi\"", "n": 1}"#;
        let extracted = first_balanced_object(raw).unwrap();
        assert_eq!(extracted, raw);
    }
}
