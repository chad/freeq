//! Conditioning transcribed speech and spoken replies.
//!
//! - [`is_hallucination`] drops the canonical phantom phrases a Whisper
//!   model emits for near-silence.
//! - [`split_speech_and_links`] separates a reply's speakable text from
//!   the URLs it mentioned — a voice agent can't pronounce a URL, so it
//!   posts links as text instead.

/// Whether `text` is a known Whisper silence/noise hallucination.
///
/// Even with voice-activity gating, a short burst of non-speech noise
/// occasionally slips a window through to the recognizer; these are the
/// canonical phantom outputs across Whisper variants. The match is exact
/// after trimming surrounding whitespace/punctuation and lowercasing —
/// the same phrase *inside* a real sentence is not a hallucination.
pub fn is_hallucination(text: &str) -> bool {
    let t = text
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_lowercase();
    matches!(
        t.as_str(),
        "" | "you"
            | "thank you"
            | "thanks for watching"
            | "thank you for watching"
            | "bye"
            | "okay"
            | "so"
            | "the"
    )
}

/// Split a reply into a speakable form plus the links it contained.
///
/// A voice agent reads its reply aloud, and URLs are unpronounceable, so
/// they're pulled out of the spoken text — markdown links `[label](url)`
/// keep only `label` in speech, bare `http(s)`/`www.` URLs are dropped
/// entirely — and the caller posts the collected URLs as text instead.
/// Whitespace left where a URL was removed is collapsed.
///
/// Only `http(s)`/`www.` URLs are surfaced as links; a markdown link to
/// some other target keeps its label but contributes no link.
pub fn split_speech_and_links(text: &str) -> (String, Vec<String>) {
    let mut links: Vec<String> = Vec::new();
    let mut spoken = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // Markdown image: ![alt](url) — DROP THE WHOLE THING from speech.
        // The alt text is for the image, not for reading aloud. Surface
        // the URL as a link so it can still be posted to the channel.
        if let Some(stripped) = rest.strip_prefix("![") {
            if let Some(mid) = stripped.find("](") {
                if let Some(close) = stripped[mid + 2..].find(')') {
                    let url = stripped[mid + 2..mid + 2 + close].trim();
                    if url.starts_with("http") || url.starts_with("www.") {
                        links.push(url.to_string());
                    }
                    rest = &stripped[mid + 2 + close + 1..];
                    continue;
                }
            }
        }
        // Any HTML/XML-ish tag — drop it. Two cases:
        //
        // 1. A paired `<tag>…</tag>` block (e.g. `<tool>python(print("…"))</tool>`).
        //    Agentic LLMs emit these as tool-call envelopes — the content
        //    inside is implementation noise (code, JSON, scratch state)
        //    that must NEVER be spoken. We strip the entire span.
        //
        // 2. An unpaired tag (`<img src=…>`, a stray `<video>` opener with
        //    no close in this chunk). Strip up to the next `>`.
        //
        // Bare `<` not followed by alpha or `/` is preserved (so literal
        // `"5 < 10"` survives).
        if rest.starts_with('<') {
            let after = &rest[1..];
            let looks_like_tag = after
                .chars()
                .next()
                .is_some_and(|c| c == '/' || c.is_ascii_alphabetic());
            if looks_like_tag {
                if let Some(end) = rest.find('>') {
                    // Try the paired form: extract the tag name (skip any
                    // leading `/` on closers; the name runs until the
                    // first non-word char). If we find a matching
                    // `</name>` *after* this opener, drop everything from
                    // here through that closer.
                    if let Some(name) = parse_tag_name(&rest[1..end]) {
                        let close_marker = format!("</{name}");
                        let search_from = end + 1;
                        if search_from < rest.len() {
                            // Case-insensitive search for the closer.
                            let hay_lower = rest[search_from..].to_lowercase();
                            let close_lower = close_marker.to_lowercase();
                            if let Some(rel_close) = hay_lower.find(&close_lower) {
                                let close_abs = search_from + rel_close;
                                // Skip past the closer's `>`.
                                if let Some(close_end) = rest[close_abs..].find('>') {
                                    rest = &rest[close_abs + close_end + 1..];
                                    continue;
                                }
                            }
                        }
                    }
                    // Unpaired (or no close in this chunk): drop the tag
                    // alone — content after it, if any, is spoken.
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        // Markdown link: [label](url) — usually speak the label and
        // surface the URL. EXCEPTION: if the URL points at an image or
        // a video, the label is alt text — drop the whole thing.
        if let Some(stripped) = rest.strip_prefix('[') {
            if let Some(mid) = stripped.find("](") {
                if let Some(close) = stripped[mid + 2..].find(')') {
                    let label = &stripped[..mid];
                    let url = stripped[mid + 2..mid + 2 + close].trim();
                    let url_is_media = looks_like_media_url(url);
                    if !url_is_media {
                        spoken.push_str(label);
                    }
                    if url.starts_with("http") || url.starts_with("www.") {
                        links.push(url.to_string());
                    }
                    rest = &stripped[mid + 2 + close + 1..];
                    continue;
                }
            }
        }
        // Bare URL — drop it from speech entirely.
        if rest.starts_with("http://")
            || rest.starts_with("https://")
            || rest.starts_with("www.")
        {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(|c| ",.;:!?)]}\"'".contains(c));
            links.push(url.to_string());
            rest = &rest[url.len()..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        spoken.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    // Collapse the whitespace left where URLs were removed.
    let spoken = spoken.split_whitespace().collect::<Vec<_>>().join(" ");
    (spoken, links)
}

/// Pull the tag name out of the body of an opening tag — i.e. what's
/// between `<` and `>` in `<tool foo="bar">` is `tool foo="bar"`, and
/// the name is `tool`. Returns `None` if the body doesn't start with a
/// letter (which means it's a closer like `/tool` or junk). We only
/// keep ASCII alphanumeric/`-`/`_` characters for the name — enough
/// for every real tag, and conservative against pathological input.
fn parse_tag_name(body: &str) -> Option<String> {
    let body = body.trim_start();
    let mut chars = body.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut name = String::new();
    name.push(first.to_ascii_lowercase());
    for c in chars {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            name.push(c.to_ascii_lowercase());
        } else {
            break;
        }
    }
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Whether `url` looks like an image or video — we use this to decide
/// whether a `[label](url)` is a *media link* (drop the label too;
/// it's alt text, not for reading) vs. a normal hyperlink (speak the
/// label).
fn looks_like_media_url(url: &str) -> bool {
    let u = url.trim().to_lowercase();
    // data URIs.
    if u.starts_with("data:image/") || u.starts_with("data:video/") || u.starts_with("data:audio/") {
        return true;
    }
    // Strip query/fragment so `.jpg?w=200` still counts.
    let path = u.split(['?', '#']).next().unwrap_or(&u);
    const MEDIA_EXTS: &[&str] = &[
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".bmp", ".tiff", ".heic",
        ".mp4", ".webm", ".mov", ".avi", ".mkv", ".m4v",
        ".mp3", ".wav", ".m4a", ".ogg", ".flac",
    ];
    MEDIA_EXTS.iter().any(|ext| path.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- is_hallucination ----------

    #[test]
    fn empty_or_blank_is_a_hallucination() {
        assert!(is_hallucination(""));
        assert!(is_hallucination("   "));
        assert!(is_hallucination("  ...  "));
    }

    #[test]
    fn canonical_whisper_phantoms_are_caught() {
        for phantom in [
            "Thank you.",
            "thank you",
            "Thanks for watching!",
            "thank you for watching",
            "You",
            "Bye.",
            "Okay",
            "so",
            "the",
        ] {
            assert!(is_hallucination(phantom), "{phantom:?} should be a hallucination");
        }
    }

    #[test]
    fn real_speech_is_not_a_hallucination() {
        for real in [
            "thank you for the summary",
            "so what did we decide",
            "the meeting starts at noon",
            "okay let's move on",
        ] {
            assert!(!is_hallucination(real), "{real:?} is real speech");
        }
    }

    // ---------- split_speech_and_links ----------

    #[test]
    fn plain_text_passes_through_with_no_links() {
        let (spoken, links) = split_speech_and_links("the answer is forty two");
        assert_eq!(spoken, "the answer is forty two");
        assert!(links.is_empty());
    }

    #[test]
    fn markdown_link_keeps_label_and_surfaces_url() {
        let (spoken, links) =
            split_speech_and_links("see [the docs](https://freeq.at/docs) for more");
        assert_eq!(spoken, "see the docs for more");
        assert_eq!(links, vec!["https://freeq.at/docs"]);
    }

    #[test]
    fn bare_url_is_dropped_from_speech() {
        let (spoken, links) =
            split_speech_and_links("read https://example.com/page now");
        assert_eq!(spoken, "read now");
        assert_eq!(links, vec!["https://example.com/page"]);
    }

    #[test]
    fn trailing_punctuation_is_trimmed_off_a_bare_url() {
        // The sentence-ending "." is trimmed off the *link* but stays in
        // the spoken text (it's punctuation, not part of the URL).
        let (spoken, links) = split_speech_and_links("source: https://example.com.");
        assert_eq!(links, vec!["https://example.com"]);
        assert_eq!(spoken, "source: .");
    }

    #[test]
    fn www_prefixed_url_is_treated_as_a_link() {
        let (spoken, links) = split_speech_and_links("visit www.freeq.at today");
        assert_eq!(spoken, "visit today");
        assert_eq!(links, vec!["www.freeq.at"]);
    }

    #[test]
    fn markdown_image_syntax_drops_the_alt_text_from_speech() {
        // The model occasionally emits ![alt](url) for an image. The alt
        // text isn't for reading aloud — drop the whole thing; surface
        // the URL so the caller can still post it as text.
        let (spoken, links) =
            split_speech_and_links("Here it is: ![A photo of a fluffy cat](https://example.com/cat.jpg) cute!");
        assert!(!spoken.to_lowercase().contains("photo"), "alt text leaked: {spoken:?}");
        assert!(!spoken.to_lowercase().contains("fluffy"), "alt text leaked: {spoken:?}");
        assert_eq!(spoken, "Here it is: cute!");
        assert_eq!(links, vec!["https://example.com/cat.jpg"]);
    }

    #[test]
    fn html_img_tag_dropped_with_attributes() {
        // <img src=… alt=…> — TTS would read "src equals http alt equals…"
        // literally. The whole tag must go.
        let (spoken, links) = split_speech_and_links(
            "Look <img src=\"https://x.com/y.png\" alt=\"the chart\" width=\"640\"> at that.",
        );
        assert_eq!(spoken, "Look at that.");
        assert!(links.is_empty(), "we keep this simple — don't extract src attrs");
    }

    #[test]
    fn html_video_iframe_audio_tags_dropped() {
        // Same lesson, broader strip — any tag-like construct is gone.
        for tag in [
            r#"<video src="https://x.com/y.mp4" poster="https://x.com/p.jpg" controls></video>"#,
            r#"<iframe src="https://x.com/embed" width="640" height="360"></iframe>"#,
            r#"<audio src="https://x.com/y.mp3" controls></audio>"#,
        ] {
            let (spoken, _) = split_speech_and_links(&format!("before {tag} after"));
            assert_eq!(spoken, "before after", "tag wasn't stripped: {tag}");
        }
    }

    #[test]
    fn paired_tool_block_strips_inner_content() {
        // Agentic Groq compound model emitted exactly this in production
        // and we were speaking the python code aloud. The whole block
        // must be gone — content included.
        let leaky = r#"Yes, I can hear you clearly. <tool>python(print("Scene card: 'Audio Connection', key points: ['Hearing confirmed']"))</tool>"#;
        let (spoken, _) = split_speech_and_links(leaky);
        assert!(!spoken.to_lowercase().contains("scene card"), "leaked: {spoken:?}");
        assert!(!spoken.to_lowercase().contains("python"), "leaked: {spoken:?}");
        assert!(!spoken.to_lowercase().contains("hearing confirmed"), "leaked: {spoken:?}");
        assert!(spoken.contains("hear you clearly"));
    }

    #[test]
    fn paired_function_call_block_strips_inner_content() {
        // Different envelope same lesson — function/function_call/code
        // blocks are all tool-call artifacts.
        for tag in ["function", "function_call", "code"] {
            let s = format!("Answer: <{tag}>secret_internal_payload</{tag}> done.");
            let (spoken, _) = split_speech_and_links(&s);
            assert!(
                !spoken.to_lowercase().contains("secret_internal_payload"),
                "leaked from <{tag}>: {spoken:?}"
            );
        }
    }

    #[test]
    fn unpaired_tag_only_strips_the_tag_itself() {
        // No matching close in this chunk → fall back to single-tag drop
        // so the rest of the prose still survives.
        let (spoken, _) = split_speech_and_links("Look <span>at this");
        assert_eq!(spoken, "Look at this");
    }

    #[test]
    fn paired_tag_match_is_case_insensitive() {
        let (spoken, _) = split_speech_and_links("a <TOOL>x</tool> b");
        assert_eq!(spoken, "a b");
    }

    #[test]
    fn bare_less_than_in_text_is_preserved() {
        // "<" not followed by alpha/`/` is not a tag and must survive.
        let (spoken, _) = split_speech_and_links("5 < 10 < 100");
        assert!(spoken.contains('<'), "literal '<' got eaten: {spoken:?}");
    }

    #[test]
    fn markdown_link_to_image_url_drops_alt_text() {
        // `[A photo of a cat](.../cat.jpg)` — the label is alt text, not
        // a speakable label. Drop it from speech but keep the URL.
        let (spoken, links) =
            split_speech_and_links("There: [A photo of a cat](https://x.com/cat.jpg) cute.");
        assert!(!spoken.to_lowercase().contains("photo"), "alt leaked: {spoken:?}");
        assert!(!spoken.to_lowercase().contains("cat"), "alt leaked: {spoken:?}");
        assert_eq!(spoken, "There: cute.");
        assert_eq!(links, vec!["https://x.com/cat.jpg"]);
    }

    #[test]
    fn markdown_link_to_video_url_drops_label_too() {
        let (spoken, links) =
            split_speech_and_links("Watch [the highlight reel](https://x.com/clip.mp4) then.");
        assert!(!spoken.to_lowercase().contains("highlight"), "label leaked: {spoken:?}");
        assert_eq!(spoken, "Watch then.");
        assert_eq!(links, vec!["https://x.com/clip.mp4"]);
    }

    #[test]
    fn markdown_link_to_normal_page_still_speaks_label() {
        // Non-media URL → still a normal hyperlink; speak the label.
        let (spoken, links) = split_speech_and_links(
            "See [the Wikipedia article](https://en.wikipedia.org/wiki/Foo) for more.",
        );
        assert!(spoken.contains("the Wikipedia article"));
        assert_eq!(links, vec!["https://en.wikipedia.org/wiki/Foo"]);
    }

    #[test]
    fn media_url_query_string_and_data_uri() {
        // `.jpg?w=200` and `data:image/...` both count as media.
        assert!(looks_like_media_url("https://x.com/cat.jpg?w=200&h=200"));
        assert!(looks_like_media_url("data:image/jpeg;base64,/9j/4AAQ..."));
        assert!(!looks_like_media_url("https://en.wikipedia.org/wiki/Cat"));
    }

    #[test]
    fn markdown_link_to_a_non_web_target_yields_no_link() {
        // The label is still spoken, but a non-http(s)/www target is not
        // surfaced as a postable link.
        let (spoken, links) = split_speech_and_links("[footnote](#section-3) explains it");
        assert_eq!(spoken, "footnote explains it");
        assert!(links.is_empty());
    }

    #[test]
    fn multiple_links_are_all_collected() {
        let (spoken, links) = split_speech_and_links(
            "both [one](https://a.example) and https://b.example matter",
        );
        assert_eq!(spoken, "both one and matter");
        assert_eq!(links, vec!["https://a.example", "https://b.example"]);
    }

    #[test]
    fn unclosed_markdown_bracket_is_left_as_literal_text() {
        // A stray `[` must not panic or eat the rest of the string.
        let (spoken, links) = split_speech_and_links("a [broken link here");
        assert_eq!(spoken, "a [broken link here");
        assert!(links.is_empty());
    }

    #[test]
    fn handles_multibyte_text_without_panicking() {
        let (spoken, links) = split_speech_and_links("café — naïve résumé 日本語");
        assert_eq!(spoken, "café — naïve résumé 日本語");
        assert!(links.is_empty());
    }
}
