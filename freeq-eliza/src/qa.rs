//! Question-answering for the live call. When a participant addresses
//! the bot by name in channel chat, we send their question (with the
//! rolling transcript as context) to a Groq model and get back a short
//! answer suitable for both posting and speaking aloud. The answer
//! model is agentic — it searches the web when a question needs it.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::video::{SceneKind, SceneSpec};

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
    /// Tool calls the agentic model ran — present on `groq/compound`.
    #[serde(default)]
    executed_tools: Vec<ExecutedTool>,
}

#[derive(Deserialize)]
struct ExecutedTool {
    #[serde(rename = "type", default)]
    tool_type: String,
    #[serde(default)]
    search_results: Option<SearchResults>,
}

#[derive(Deserialize)]
struct SearchResults {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
}

/// A web source an answer drew on — posted into the channel so people
/// can read more.
#[derive(Debug, Clone)]
pub struct Source {
    pub title: String,
    pub url: String,
}

/// A spoken answer, plus the top web source behind it when the agentic
/// model searched the web.
#[derive(Debug, Clone)]
pub struct Answer {
    pub text: String,
    pub source: Option<Source>,
}

const SYSTEM: &str = "You are Eliza, a helpful AI agent in a live voice \
call. A participant has addressed you by name — answer their question. \
Use the call transcript below as context when the question is about the \
conversation itself; otherwise answer from your general knowledge. When \
the question needs current events or specific facts you're not certain \
of, search the web and answer from what you find. Rules: answer in 1-3 \
short sentences — your reply is spoken aloud, so keep it brief and \
conversational. Don't use markdown, bullet points, or emoji. Never put \
URLs, links, or web addresses in your answer — it is read aloud and they \
are unpronounceable; just name the source in words if you need to. Don't \
repeat the question back. If you genuinely don't know, say so plainly.";

/// Answer `question` against `transcript` via Groq chat completions.
/// `transcript` is the joined `<nick>: <utterance>` lines so far (may
/// be empty early in a call). When the agentic model searched the web,
/// the returned [`Answer`] carries the top source so the caller can
/// post the link.
pub async fn answer(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    transcript: &str,
    question: &str,
) -> Result<Answer> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 320,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": user_prompt(transcript, question) },
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
    let Some(choice) = parsed.choices.into_iter().next() else {
        anyhow::bail!("groq chat returned no choices");
    };
    let text = choice.message.content.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("groq chat returned no content");
    }
    let source = extract_source(&choice.message.executed_tools);
    Ok(Answer { text, source })
}

/// The user-turn prompt: the rolling transcript as context plus the
/// question. Shared by [`answer`] and [`answer_streaming`].
fn user_prompt(transcript: &str, question: &str) -> String {
    let context = if transcript.trim().is_empty() {
        "(no transcript yet — the call just started)".to_string()
    } else {
        transcript.to_string()
    };
    format!("Call transcript so far:\n{context}\n\nQuestion: {question}")
}

/// The top web source behind an answer — the first result of the first
/// `search` tool the agentic model ran, if any.
fn extract_source(tools: &[ExecutedTool]) -> Option<Source> {
    tools
        .iter()
        .filter(|t| t.tool_type == "search")
        .filter_map(|t| t.search_results.as_ref())
        .flat_map(|sr| &sr.results)
        .find(|r| !r.url.trim().is_empty())
        .map(|r| Source {
            title: r.title.trim().to_string(),
            url: r.url.trim().to_string(),
        })
}

const SCENE_SYSTEM: &str = "You design one visual card for Eliza's \
video tile — a glanceable summary of the answer it just gave on a live \
call. Output ONLY a JSON object:\n\
{\"kind\":\"...\",\"title\":\"...\",\"subtitle\":\"...\",\"points\":[\"...\"],\"accent\":\"#RRGGBB\",\"image_query\":\"...\"}\n\
Pick the kind that best fits the answer:\n\
- \"hero\": one big idea. title = a punchy headline (<=6 words). \
subtitle = a one-line takeaway (<=14 words). points = [].\n\
- \"keypoints\": several distinct points. title = the topic (<=5 \
words). points = 2 to 5 items, each <=9 words. subtitle = \"\".\n\
- \"stat\": a single number carries the answer. title = what it \
measures (<=6 words). points = [the value as a short string, e.g. \
\"70%\" or \"1969\"]. subtitle = context (<=14 words).\n\
- \"timeline\": a sequence or process. title = the process (<=5 \
words). points = 2 to 5 ordered steps, each <=8 words. subtitle = \
\"\".\n\
- \"quote\": a striking statement or definition. title = the line \
itself (<=18 words). subtitle = attribution or source (<=5 words). \
points = [].\n\
Rules:\n\
- All text is plain — no markdown, no emoji, no trailing punctuation on \
points.\n\
- accent: a hex colour (#RRGGBB) that suits the topic's mood.\n\
- image_query: a short, concrete subject to illustrate the topic — a \
specific thing, place, person, or scene in 2 to 6 words (e.g. \"Apollo \
11 Moon landing\", \"deep ocean floor\", \"Marie Curie\"). Name \
something real and depictable; it is used to find a photo.\n\
- Keep every field terse: it is read at a glance on a small tile.";

/// Pull a trimmed string field out of a JSON object (empty if absent).
fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Ask the model to design a visual card for the latest answer. Returns
/// a [`SceneSpec`], or `None` when there's nothing worth showing or on
/// any error — eliza then keeps its current tile. Never fails the
/// caller.
pub async fn generate_scene(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    question: &str,
    answer: &str,
) -> Option<SceneSpec> {
    let user = format!(
        "Question addressed to Eliza: {question}\n\nThe answer it gave: \
         {answer}\n\nDesign the card. Output the JSON object:"
    );
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 600,
        "temperature": 0.5,
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": SCENE_SYSTEM },
            { "role": "user", "content": user },
        ],
    });
    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let parsed: ChatResponse = resp.json().await.ok()?;
    let text = parsed.choices.first()?.message.content.trim().to_string();
    let json = extract_json(&text)?;

    let kind = SceneKind::from_tag(&str_field(&json, "kind"));
    let title = str_field(&json, "title");
    let subtitle = str_field(&json, "subtitle");
    let points: Vec<String> = json
        .get("points")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let accent = str_field(&json, "accent");
    let image_query = str_field(&json, "image_query");

    if title.is_empty() && points.is_empty() {
        return None;
    }
    Some(SceneSpec {
        kind,
        title,
        subtitle,
        points,
        accent,
        image_query,
    })
}

/// Pull a JSON object out of a model reply — it may be fenced in
/// markdown or wrapped in stray prose. Takes the outermost `{ … }`.
pub(crate) fn extract_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?.checked_add(1)?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..end]).ok()
}

// ── Streaming answer ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    /// Some providers attach tool info to a `message` even mid-stream.
    #[serde(default)]
    message: StreamDelta,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: String,
    #[serde(default)]
    executed_tools: Vec<ExecutedTool>,
}

/// Streaming variant of [`answer`]: calls `on_delta` with each text
/// fragment as the model produces it, so the caller can begin speaking
/// before the answer is complete. Still accumulates the full text and
/// returns the same [`Answer`] (with the web source, when one was used).
pub async fn answer_streaming(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    transcript: &str,
    question: &str,
    mut on_delta: impl FnMut(&str),
) -> Result<Answer> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 320,
        "temperature": 0.3,
        "stream": true,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": user_prompt(transcript, question) },
        ],
    });

    let mut resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("groq streaming chat request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("groq chat {status}: {err}");
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut text = String::new();
    let mut source: Option<Source> = None;

    while let Some(network_chunk) = resp.chunk().await.context("reading groq chat stream")? {
        buf.extend_from_slice(&network_chunk);
        // Server-Sent Events: process complete `\n`-terminated lines; a
        // partial trailing line waits for the next network chunk.
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(sc) = serde_json::from_str::<StreamChunk>(data) else {
                continue;
            };
            for choice in sc.choices {
                if !choice.delta.content.is_empty() {
                    text.push_str(&choice.delta.content);
                    on_delta(&choice.delta.content);
                }
                if source.is_none() {
                    source = extract_source(&choice.delta.executed_tools)
                        .or_else(|| extract_source(&choice.message.executed_tools));
                }
            }
        }
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("groq streaming chat returned no content");
    }
    Ok(Answer { text, source })
}

// ── Sentence chunking ────────────────────────────────────────────────

/// Accumulates streamed text and emits complete sentences, so an answer
/// can be synthesized to speech sentence-by-sentence as it streams in.
#[derive(Default)]
pub struct SentenceChunker {
    buf: String,
}

impl SentenceChunker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a streamed text fragment; return any sentences it completed.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        while let Some(end) = self.next_sentence_end() {
            let sentence = self.buf[..end].trim().to_string();
            self.buf.drain(..end);
            if sentence.chars().any(char::is_alphanumeric) {
                out.push(sentence);
            }
        }
        out
    }

    /// The trailing text once the stream ends — the final sentence
    /// usually has no whitespace after it to trigger a flush.
    pub fn flush(&mut self) -> Option<String> {
        let rest = self.buf.trim().to_string();
        self.buf.clear();
        rest.chars().any(char::is_alphanumeric).then_some(rest)
    }

    /// Byte index in `buf` just past the first complete sentence (the
    /// start of the whitespace after a sentence-ending `.`/`!`/`?`).
    /// `None` until enough text has arrived to be certain.
    fn next_sentence_end(&self) -> Option<usize> {
        let chars: Vec<(usize, char)> = self.buf.char_indices().collect();
        let mut i = 0;
        while i < chars.len() {
            if !matches!(chars[i].1, '.' | '!' | '?') {
                i += 1;
                continue;
            }
            let term_at = chars[i].0;
            // Consume a run of terminators ("?!", "...").
            while i < chars.len() && matches!(chars[i].1, '.' | '!' | '?') {
                i += 1;
            }
            // Consume any closing quote/bracket.
            while i < chars.len() && matches!(chars[i].1, '"' | '\'' | ')' | ']' | '”' | '’') {
                i += 1;
            }
            // A following char is needed to know the sentence ended.
            let Some(&(next_at, next_c)) = chars.get(i) else {
                return None;
            };
            if next_c.is_whitespace() && !ends_with_abbrev(&self.buf[..term_at]) {
                return Some(next_at);
            }
        }
        None
    }
}

/// Whether `prefix` ends in an abbreviation or initial — so the `.`
/// after it does not end a sentence ("Mr.", "U.S.", "e.g.", "A.").
fn ends_with_abbrev(prefix: &str) -> bool {
    let word = prefix
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '.')
        .to_lowercase();
    if word.is_empty() {
        return false;
    }
    // A single letter is an initial ("A." in "A. Lincoln").
    let letters: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
    if letters.chars().count() == 1 {
        return true;
    }
    const ABBREVS: &[&str] = &[
        "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "no", "fig", "approx",
        "e.g", "i.e", "u.s", "a.m", "p.m", "ph.d",
    ];
    ABBREVS.contains(&word.as_str()) || ABBREVS.contains(&letters.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_pulls_object_out_of_messy_replies() {
        // Bare object.
        let v = extract_json(r#"{"title":"T","steps":["a"]}"#).unwrap();
        assert_eq!(v["title"], "T");
        // Markdown-fenced with surrounding prose.
        let v = extract_json("Sure:\n```json\n{\"title\":\"T\"}\n```\nok").unwrap();
        assert_eq!(v["title"], "T");
        // No object, or invalid JSON → None, never a panic.
        assert!(extract_json("no json here").is_none());
        assert!(extract_json("{not valid").is_none());
    }

    // ---------- SentenceChunker ----------

    #[test]
    fn chunker_emits_completed_sentences_and_holds_the_last() {
        let mut c = SentenceChunker::new();
        // The last sentence has no trailing whitespace → it waits.
        let got = c.push("One thing. Two things! Three?");
        assert_eq!(got, vec!["One thing.", "Two things!"]);
        assert_eq!(c.flush().as_deref(), Some("Three?"));
    }

    #[test]
    fn chunker_reassembles_across_streamed_fragments() {
        let mut c = SentenceChunker::new();
        let mut out = Vec::new();
        // The same text arrives split at awkward points.
        for frag in ["On", "e. Tw", "o. ", "Done"] {
            out.extend(c.push(frag));
        }
        assert_eq!(out, vec!["One.", "Two."]);
        assert_eq!(c.flush().as_deref(), Some("Done"));
    }

    #[test]
    fn chunker_does_not_break_on_abbreviations() {
        let mut c = SentenceChunker::new();
        let got = c.push("Visit the U.S. and see Mr. Smith today. Thanks.");
        // "U.S." and "Mr." must not split the first sentence.
        assert_eq!(got, vec!["Visit the U.S. and see Mr. Smith today."]);
        assert_eq!(c.flush().as_deref(), Some("Thanks."));
    }

    #[test]
    fn chunker_handles_decimals_and_terminator_runs() {
        let mut c = SentenceChunker::new();
        // "3.5" — the dot is between digits, no whitespace, no break.
        let got = c.push("It grew 3.5 times. Really?! Yes.");
        assert_eq!(got, vec!["It grew 3.5 times.", "Really?!"]);
        assert_eq!(c.flush().as_deref(), Some("Yes."));
    }

    #[test]
    fn chunker_drops_punctuation_only_fragments() {
        let mut c = SentenceChunker::new();
        // "..." with nothing alphanumeric is not a real sentence.
        assert!(c.push("... ").is_empty());
        assert_eq!(c.flush(), None);
    }
}
