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
        // HTML <img …> tag — drop the whole tag from speech. Otherwise
        // TTS reads "img src equals http alt equals …" literally.
        if rest.starts_with("<img") {
            if let Some(end) = rest.find('>') {
                rest = &rest[end + 1..];
                continue;
            }
        }
        // Markdown link: [label](url) — speak the label, surface the url.
        if let Some(stripped) = rest.strip_prefix('[') {
            if let Some(mid) = stripped.find("](") {
                if let Some(close) = stripped[mid + 2..].find(')') {
                    let label = &stripped[..mid];
                    let url = stripped[mid + 2..mid + 2 + close].trim();
                    spoken.push_str(label);
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
