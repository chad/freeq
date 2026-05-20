//! Question-answering for the live call. When a participant addresses
//! the bot by name in channel chat, we feed the rolling transcript +
//! their question to a Groq chat model and get back a short answer
//! suitable for both posting and speaking aloud.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: String,
}

const SYSTEM: &str = "You are a transcription bot sitting in a live voice \
call. A participant has addressed you by name in the text chat. Answer \
their question using the call transcript provided as context. Rules: \
answer in 1-3 short sentences — your reply will be spoken aloud, so keep \
it brief and conversational. Don't use markdown, bullet points, or \
emoji. If the transcript doesn't contain the answer, say so plainly. \
Don't invent facts. Don't repeat the question back.";

/// Answer `question` against `transcript` via Groq chat completions.
/// `transcript` is the joined `<nick>: <utterance>` lines so far (may
/// be empty early in a call).
pub async fn answer(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    transcript: &str,
    question: &str,
) -> Result<String> {
    let context = if transcript.trim().is_empty() {
        "(no transcript yet — the call just started)".to_string()
    } else {
        transcript.to_string()
    };
    let user = format!("Call transcript so far:\n{context}\n\nQuestion: {question}");

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 320,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": user },
        ],
    });

    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("groq chat request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("groq chat {status}: {err}");
    }
    let parsed: ChatResponse = resp.json().await.context("groq chat parse failed")?;
    let text = parsed
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("groq chat returned no content");
    }
    Ok(text)
}

/// If `text` addresses `nick` at the start (`nick:`, `nick,`,
/// `@nick `, or bare `nick ` followed by words), return the remainder
/// — the actual question. Case-insensitive. `None` if the message
/// isn't addressed to the bot.
pub fn extract_addressed<'a>(text: &'a str, nick: &str) -> Option<&'a str> {
    let trimmed = text.trim_start();
    let lower = trimmed.to_lowercase();
    let nick_lower = nick.to_lowercase();

    for prefix in [
        format!("@{nick_lower} "),
        format!("{nick_lower}: "),
        format!("{nick_lower}, "),
        format!("{nick_lower} "),
    ] {
        if lower.starts_with(&prefix) {
            let rest = trimmed[prefix.len()..].trim();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_colon_form() {
        assert_eq!(
            extract_addressed("utopia: what did we decide?", "utopia"),
            Some("what did we decide?")
        );
    }

    #[test]
    fn extracts_comma_and_at_and_bare_forms() {
        assert_eq!(
            extract_addressed("utopia, summarize", "utopia"),
            Some("summarize")
        );
        assert_eq!(
            extract_addressed("@utopia who is talking", "utopia"),
            Some("who is talking")
        );
        assert_eq!(
            extract_addressed("utopia recap please", "utopia"),
            Some("recap please")
        );
    }

    #[test]
    fn case_insensitive_on_nick() {
        assert_eq!(
            extract_addressed("Utopia: hi", "utopia"),
            Some("hi")
        );
    }

    #[test]
    fn ignores_unaddressed_or_mid_sentence_mentions() {
        assert_eq!(extract_addressed("hello everyone", "utopia"), None);
        assert_eq!(
            extract_addressed("ask the utopia later", "utopia"),
            None,
            "a mention mid-sentence is not an address"
        );
    }

    #[test]
    fn ignores_bare_nick_with_no_question() {
        // Just the nick, nothing after → not a question.
        assert_eq!(extract_addressed("utopia", "utopia"), None);
        assert_eq!(extract_addressed("utopia: ", "utopia"), None);
    }
}
