//! Decision log — extracts commitment language from speech and
//! accumulates structured `Decision` records so a session-end
//! read-back surfaces "things this room decided" without anyone
//! taking notes. Live demo of *conversation as the source of
//! knowledge work*: nobody types decisions, they just say them.
//!
//! Extraction is intentionally simple + deterministic so the
//! functional core is unit-testable. The patterns catch the
//! highest-signal commitment language ("let's X", "we should X",
//! "I'll X", "by [date] we'll X"); an LLM enrichment pass is a
//! reasonable future addition that wraps the deterministic core
//! rather than replaces it.

/// One captured commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Speaker nick who proposed it.
    pub who: String,
    /// The commitment text itself, trimmed.
    pub what: String,
    /// Optional time / deadline phrase ("by Friday", "tomorrow",
    /// "next sprint"). Best-effort, not normalised — humans say
    /// time fuzzily.
    pub when: Option<String>,
}

impl Decision {
    /// Scan `text` for commitment language and return any decisions
    /// it contains. Multiple per input are possible — long speech
    /// often chains commitments.
    pub fn extract(who: &str, text: &str) -> Vec<Decision> {
        let mut out = Vec::new();
        for sentence in split_sentences(text) {
            if let Some(body) = match_commitment(sentence) {
                let (what, when) = split_deadline(&body);
                out.push(Decision {
                    who: who.to_string(),
                    what,
                    when,
                });
            }
        }
        out
    }
}

/// Naïve sentence splitter — period/question/exclamation. Captures
/// the speech-rhythm we want without dragging in a full NLP tokenizer.
fn split_sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == '.' || c == '?' || c == '!')
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Strip a leading filler word like "then", "so", "and" so the
/// commitment prefixes (`let's`, `we should`, `i'll`) can still match
/// when a speaker chains sentences ("...then i'll write the post").
fn strip_filler_prefix(sentence: &str) -> &str {
    const FILLERS: &[&str] = &["then ", "so ", "and ", "also ", "ok ", "okay "];
    let mut s = sentence;
    loop {
        let mut hit = false;
        let lower = s.to_ascii_lowercase();
        for f in FILLERS {
            if lower.starts_with(f) {
                s = &s[f.len()..];
                hit = true;
                break;
            }
        }
        if !hit {
            break;
        }
    }
    s.trim_start()
}

/// Match any of the supported commitment prefixes and return the
/// body of the commitment (everything after the prefix, lowercased,
/// trimmed). Returns `None` when no prefix fires — the sentence is
/// idle chat, not a commitment.
///
/// Patterns:
///   * `let's X` / `lets X`
///   * `we should X` / `we'll X` / `we will X`
///   * `i'll X` / `i will X`
///   * `i'm going to X`
fn match_commitment(sentence: &str) -> Option<String> {
    let trimmed = strip_filler_prefix(sentence);
    // Strip leading non-alphanumeric (commas etc.) but preserve `'`.
    let trimmed = trimmed.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '\'');
    let lower = trimmed.to_ascii_lowercase();
    // Ordered longest-first so "we'll" doesn't shadow "we will" and
    // vice versa.
    const PREFIXES: &[&str] = &[
        "let's ",
        "lets ",
        "we should ",
        "we'll ",
        "we will ",
        "i'll ",
        "i will ",
        "i'm going to ",
    ];
    for p in PREFIXES {
        if let Some(rest) = lower.strip_prefix(p) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Split a commitment body into `(what, when)` by pulling out a
/// trailing time/date phrase. Recognises a small but high-signal
/// vocabulary — enough to land on the demo, not enough to claim
/// general NLP. Anything else stays in `what` with `when = None`.
fn split_deadline(body: &str) -> (String, Option<String>) {
    // The most common shape: "X by <when>" or "X tomorrow"
    // or "X tonight". Look for the LAST occurrence so "by friday"
    // wins over "by the book" if both happened to appear.
    const BY_MARKER: &str = " by ";
    if let Some(pos) = body.rfind(BY_MARKER) {
        let what = body[..pos].trim().to_string();
        let when = body[pos + BY_MARKER.len()..].trim().to_string();
        if !what.is_empty() && !when.is_empty() {
            return (what, Some(when));
        }
    }
    // Trailing single-word time markers — strip them off the end.
    const TRAILING_WHEN: &[&str] = &[
        "tonight",
        "tomorrow",
        "today",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    for w in TRAILING_WHEN {
        let suffix = format!(" {w}");
        if let Some(stripped) = body.strip_suffix(&suffix) {
            let what = stripped.trim().to_string();
            if !what.is_empty() {
                return (what, Some((*w).to_string()));
            }
        }
    }
    (body.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_no_decisions() {
        assert!(Decision::extract("chad", "").is_empty());
    }

    #[test]
    fn small_talk_no_decisions() {
        assert!(Decision::extract("chad", "hey how are you").is_empty());
    }

    #[test]
    fn lets_clause_captures_decision() {
        let d = Decision::extract("chad", "let's ship the build tonight");
        assert_eq!(d.len(), 1, "expected one decision, got {d:?}");
        assert_eq!(d[0].who, "chad");
        assert!(d[0].what.contains("ship"), "what was {:?}", d[0].what);
    }

    #[test]
    fn we_should_clause_captures_decision() {
        let d = Decision::extract("jess", "we should rebuild the server");
        assert_eq!(d.len(), 1, "expected one decision, got {d:?}");
        assert!(d[0].what.contains("rebuild"));
    }

    #[test]
    fn ill_clause_captures_decision() {
        let d = Decision::extract("chad", "I'll handle the deploy");
        assert_eq!(d.len(), 1, "expected one decision, got {d:?}");
        assert!(d[0].what.contains("handle the deploy"));
    }

    #[test]
    fn deadline_phrase_attaches_to_when() {
        let d = Decision::extract("chad", "let's ship the build by friday");
        assert_eq!(d.len(), 1, "got {d:?}");
        assert_eq!(d[0].when.as_deref(), Some("friday"), "decision {:?}", d[0]);
        // `what` no longer carries the deadline — it's been pulled out.
        assert!(!d[0].what.contains("friday"), "what was {:?}", d[0].what);
    }

    #[test]
    fn multiple_clauses_in_one_input() {
        let d = Decision::extract(
            "chad",
            "let's ship tonight. then i'll write the post.",
        );
        assert_eq!(d.len(), 2, "got {d:?}");
        assert!(d[0].what.contains("ship"));
        assert!(d[1].what.contains("write the post"));
    }

    #[test]
    fn idle_speculation_not_a_decision() {
        // No commitment markers, just musing.
        let d = Decision::extract(
            "chad",
            "I think the weather might turn tomorrow",
        );
        assert!(d.is_empty(), "got {d:?}");
    }
}

