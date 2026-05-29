//! Social-presence helpers — small deterministic pieces that turn
//! the multi-agent rig from a Q&A queue into something that *feels*
//! like a meeting. Five features land here, each behind its own
//! adversarial unit tests so the parsers / mergers / pickers stay
//! pinned even as the IRC orchestration around them evolves:
//!
//!   1. [`extract_addressee`] — when a human says "Narrator, …",
//!      this returns `"narrator"` so all OTHER agents can swing
//!      their gaze toward the addressee (peer-aware attention).
//!
//!   2. [`mention_without_address`] — was *this* bot's name name-
//!      dropped without being directly asked? Drives the silent
//!      hand-raise: "I have something to add."
//!
//!   3. [`pick_backchannel`] — a tiny audible "mm" / "hmm" / "right"
//!      that listening bots emit while another agent answers. Rate-
//!      limited per bot so it does not become noise.
//!
//!   4. [`merge_diagrams`] — fold N per-bot diagrams into one shared
//!      graph so every tile draws the same whiteboard.
//!
//!   5. [`format_session_recall`] — a one-line "I remember…" string
//!      a bot says at session open, sourced from the FTS5 memory's
//!      most-recent exchanges.

use crate::diagram::Diagram;
use crate::memory::Recollection;

/// Phrases a human can say to flip the room into peer-conversation
/// mode. Each agent watches its own STT stream for any of these; on
/// a match the global `discussion_until` deadline extends 90 s into
/// the future. While the deadline is active, the strict
/// human-only-address policy relaxes and bots may answer each other.
/// A new human utterance that does NOT contain one of these phrases
/// is the natural off-switch — humans speaking resets the chain
/// already, and the deadline expires on its own.
pub const DISCUSSION_TRIGGER_PHRASES: &[&str] = &[
    "discuss it",
    "discuss this",
    "discuss that",
    "discuss amongst yourselves",
    "discuss among yourselves",
    "talk amongst yourselves",
    "talk among yourselves",
    "talk it out",
    "debate it",
    "debate this",
    "go ahead and discuss",
    "you decide",
    "you all discuss",
];

/// True if `text` is a human cue to enter peer-conversation mode.
/// Case-insensitive whole-phrase substring match.
pub fn is_discussion_trigger(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    DISCUSSION_TRIGGER_PHRASES
        .iter()
        .any(|p| lower.contains(p))
}

/// Scan `answer_text` for a trailing peer hand-off and return
/// `(peer_name, question_body)`. Looks for the last occurrence of a
/// peer name followed by `,` / `:` / ` -` and at least one more
/// word. Falls back to detecting a bare peer name at the end of a
/// question ("…Utopia?") with an empty body — caller can synthesise
/// a generic continuation in that case.
///
/// Used to bypass TTS/STT chunking: the bot's LLM answer often
/// addresses a peer in its last sentence, but the resulting TTS gets
/// split into multiple STT utterances on the receiving side and the
/// peer-name fragment lands without context. We extract the hand-off
/// directly from the model's text and send it as a deterministic IRC
/// privmsg the peer parses without going through audio at all.
pub fn extract_peer_handoff(
    answer_text: &str,
    peers: &std::collections::HashSet<String>,
) -> Option<(String, String)> {
    let lower = answer_text.to_ascii_lowercase();
    // Find every peer occurrence; pick the latest one.
    let mut best: Option<(usize, &str)> = None;
    for peer in peers {
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(peer.as_str()) {
            let pos = from + rel;
            // Word boundary check on both sides.
            let left_ok =
                pos == 0 || !lower.as_bytes()[pos - 1].is_ascii_alphanumeric();
            let right_ok = pos + peer.len() == lower.len()
                || !lower.as_bytes()[pos + peer.len()].is_ascii_alphanumeric();
            if left_ok && right_ok {
                best = match best {
                    Some((bp, _)) if bp >= pos => best,
                    _ => Some((pos, peer.as_str())),
                };
            }
            from = pos + 1;
        }
    }
    let (pos, peer) = best?;
    // Take everything after the peer name, strip leading punctuation
    // and whitespace.
    let after = &answer_text[pos + peer.len()..];
    let body: String = after
        .trim_start_matches(|c: char| {
            c.is_ascii_punctuation() || c.is_whitespace()
        })
        .trim_end()
        .to_string();
    Some((peer.to_string(), body))
}

/// Strict direct-address check: only the punctuated forms count. The
/// permissive parser in `freeq_agent_kit::addressing` treats any
/// sentence beginning with the name as an address, which conflates
/// "Narrator, what about X?" with "narrator was right earlier" —
/// fine for the answer dispatcher (which prefers false positives
/// over silent bots) but too loose for distinguishing addresses from
/// mentions.
fn is_directly_addressed(text: &str, nick: &str) -> bool {
    let canonical = nick
        .split_once('-')
        .map(|(p, _)| p)
        .unwrap_or(nick)
        .to_ascii_lowercase();
    if canonical.len() < 4 {
        return false;
    }
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    // Forms we accept as a direct address (case-insensitive, leading
    // filler words stripped by the agent-kit parser elsewhere — here
    // we only see the raw text):
    //   name,
    //   name:
    //   name -
    //   @name
    for candidate in [&canonical, &nick.to_ascii_lowercase()] {
        for suffix in &[",", ":", " -", " —", "?", "!"] {
            let pattern = format!("{candidate}{suffix}");
            if lower.starts_with(&pattern) {
                return true;
            }
        }
        let at_pattern = format!("@{candidate}");
        if lower.starts_with(&at_pattern) {
            return true;
        }
    }
    false
}

/// If `text` is addressed to one of `candidates` (a list of known
/// nicks in the room — humans + peer agents), return the matched
/// canonical lowercase name. Used so a bot can swing its gaze toward
/// whoever the human just called on.
///
/// The matcher is intentionally permissive on the leading word — STT
/// frequently mishears names — but rejects anything that doesn't
/// land on a candidate. Returns `None` for idle chatter or addresses
/// to nobody we know.
pub fn extract_addressee(text: &str, candidates: &[&str]) -> Option<String> {
    // Re-use the avatar-kit address parser by trying each candidate
    // in turn — first match wins. The parser already handles colon,
    // comma, @ prefix, and slight STT mishearings via edit distance.
    for cand in candidates {
        if freeq_agent_kit::addressing::extract_addressed(text, cand).is_some() {
            return Some(cand.to_ascii_lowercase());
        }
    }
    None
}

/// True if `my_nick` is *mentioned* anywhere in `text` but is not the
/// thing being directly addressed at the start. Drives the hand-
/// raise: "they're talking ABOUT me, I'd like to chime in."
///
/// Distinguishing a direct address ("Narrator, what about that?")
/// from a third-person mention ("narrator was right earlier") is the
/// whole point of this function. The address matcher is deliberately
/// permissive — it treats any sentence starting with the name as an
/// address — so we tighten the test here: only the punctuated forms
/// (`name,` / `name:` / `@name` / `name -`) count as direct
/// addresses, and everything else where the name appears at a word
/// boundary is a mention.
pub fn mention_without_address(text: &str, my_nick: &str) -> bool {
    if my_nick.is_empty() {
        return false;
    }
    // First: is this a *direct* address to me (the punctuated form)?
    // If so, it's a normal Q, not a hand-raise.
    if is_directly_addressed(text, my_nick) {
        return false;
    }
    // Allow the bot's character-name prefix (e.g. "oblivion" matches
    // server-suffixed "oblivion-z6mkfa8x") — the user always says the
    // character name, not the suffixed nick.
    let canonical = my_nick
        .split_once('-')
        .map(|(p, _)| p)
        .unwrap_or(my_nick)
        .to_ascii_lowercase();
    if canonical.len() < 4 {
        return false;
    }
    // Word-boundary substring match (case-insensitive) — avoids
    // matching "narrator" inside "narratorial" while still catching
    // "Narrator's" / "narrator." / "narrator,".
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let needle = canonical.as_bytes();
    let nlen = needle.len();
    if bytes.len() < nlen {
        return false;
    }
    for i in 0..=bytes.len() - nlen {
        if &bytes[i..i + nlen] == needle {
            let left_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let right_ok =
                i + nlen == bytes.len() || !bytes[i + nlen].is_ascii_alphanumeric();
            if left_ok && right_ok {
                return true;
            }
        }
    }
    false
}

/// Pick a short character-flavoured backchannel utterance — or
/// `None` to stay silent — given the time since the last one this
/// bot emitted. The picker rotates through three candidates per
/// character so consecutive backchannels do not repeat verbatim.
///
/// Returns `None` if it's been less than `min_gap_secs` since the
/// last backchannel from this bot (per-bot rate limit).
pub fn pick_backchannel(
    character: &str,
    seconds_since_last: f32,
    min_gap_secs: f32,
    counter: u32,
) -> Option<&'static str> {
    if seconds_since_last < min_gap_secs {
        return None;
    }
    let pool: &[&'static str] = match character.to_ascii_lowercase().as_str() {
        "oblivion" => &["mm.", "hm.", "right."],
        "utopia" => &["mhm.", "yeah.", "okay."],
        "narrator" => &["mm.", "go on.", "yes."],
        // Default neutral pool for any other character.
        _ => &["mm.", "hm.", "yeah."],
    };
    Some(pool[(counter as usize) % pool.len()])
}

/// Merge several per-bot diagrams into one. The result contains every
/// node and edge from every input, deduplicated by the same canonical
/// form `Diagram::ingest` uses. Provenance is lost — that is the
/// point; the rendered whiteboard is the room's diagram, not any one
/// bot's.
pub fn merge_diagrams<'a, I>(diagrams: I) -> Diagram
where
    I: IntoIterator<Item = &'a Diagram>,
{
    let mut out = Diagram::new();
    for d in diagrams {
        for edge in d.edges() {
            // `ingest` is the public path, but it parses raw text;
            // for direct edge-replay we want the structured form.
            // The diagram module exposes the underlying triple
            // through `Edge`; call insert via a synthesised
            // sentence the diagram parser is guaranteed to round-
            // trip ("FROM RELATION TO" matches the SVO pattern,
            // and the relation is a verb we recognise).
            let sentence = format!("{} {} {}", edge.from, edge.relation, edge.to);
            out.ingest(&sentence);
        }
    }
    out
}

/// Pull the K most recent recollections from a `recall` result and
/// format them as a single spoken line a bot can open the session
/// with — "Welcome back. Last time we landed on X." Returns `None`
/// when the input is empty so the caller can skip the recall step
/// entirely.
pub fn format_session_recall(recs: &[Recollection]) -> Option<String> {
    if recs.is_empty() {
        return None;
    }
    // Take the most recent (highest ts) — the operator does not want
    // an exhaustive readback, just one continuity hook.
    let mut sorted: Vec<&Recollection> = recs.iter().collect();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.ts));
    let top = sorted[0];
    // Strip trailing punctuation + truncate to one breath worth.
    let q = top.question.trim_end_matches(|c: char| c == '?' || c == '.').trim();
    let q_short: String = q.chars().take(80).collect();
    Some(format!("Last time, you asked about {q_short}. I remember."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::Diagram;
    use crate::memory::Recollection;

    // ── 1. extract_addressee ──────────────────────────────────────

    #[test]
    fn addressee_direct_match_returns_canonical_nick() {
        let r = extract_addressee("Narrator, what's your take?", &["narrator", "utopia"]);
        assert_eq!(r.as_deref(), Some("narrator"));
    }

    #[test]
    fn addressee_no_address_returns_none() {
        let r = extract_addressee("The weather looks great today", &["narrator", "utopia"]);
        assert!(r.is_none());
    }

    #[test]
    fn addressee_mention_mid_sentence_does_not_count() {
        // Names dropped mid-sentence are NOT addresses — that's the
        // hand-raise case, handled separately.
        let r = extract_addressee(
            "I think narrator's earlier point was sharper",
            &["narrator", "utopia"],
        );
        assert!(r.is_none(), "mid-sentence mentions should not be addresses");
    }

    #[test]
    fn addressee_first_candidate_in_list_does_not_short_circuit_wrong_one() {
        // "Utopia, …" should match utopia even though narrator is
        // listed first in the candidates array.
        let r = extract_addressee("Utopia, counter that", &["narrator", "utopia"]);
        assert_eq!(r.as_deref(), Some("utopia"));
    }

    // ── 2. mention_without_address ───────────────────────────────

    #[test]
    fn mention_without_address_fires_on_namedrop() {
        assert!(mention_without_address(
            "narrator was making a useful point about timing",
            "narrator",
        ));
    }

    #[test]
    fn mention_without_address_does_not_fire_on_direct_address() {
        // Direct address is a question to answer, not a hand-raise.
        assert!(!mention_without_address(
            "narrator, what about that?",
            "narrator",
        ));
    }

    #[test]
    fn mention_without_address_handles_suffixed_nick() {
        // The bot's actual IRC nick is suffixed; humans say the
        // character name. The matcher checks the prefix.
        assert!(mention_without_address(
            "I disagree with oblivion's framing",
            "oblivion-z6mkfa8x",
        ));
    }

    #[test]
    fn mention_without_address_word_boundary_only() {
        // "narrator" must not match "narratorial".
        assert!(!mention_without_address(
            "that was a narratorial flourish",
            "narrator",
        ));
    }

    // ── 3. pick_backchannel ───────────────────────────────────────

    #[test]
    fn backchannel_returns_a_phrase_when_gap_elapsed() {
        let r = pick_backchannel("oblivion", 8.0, 5.0, 0);
        assert!(r.is_some(), "expected a backchannel after the gap");
    }

    #[test]
    fn backchannel_returns_none_inside_rate_limit() {
        let r = pick_backchannel("oblivion", 2.0, 5.0, 0);
        assert!(r.is_none(), "should be rate-limited under min_gap");
    }

    #[test]
    fn backchannel_rotates_with_counter() {
        let a = pick_backchannel("utopia", 10.0, 5.0, 0).unwrap();
        let b = pick_backchannel("utopia", 10.0, 5.0, 1).unwrap();
        assert_ne!(a, b, "counter should rotate the phrase");
    }

    #[test]
    fn backchannel_unknown_character_uses_default_pool() {
        let r = pick_backchannel("eliza", 10.0, 5.0, 0);
        assert!(r.is_some(), "should still return something for unknown chars");
    }

    // ── 4. merge_diagrams ─────────────────────────────────────────

    #[test]
    fn merge_empty_inputs_yields_empty_diagram() {
        let merged = merge_diagrams::<Vec<&Diagram>>(vec![]);
        assert_eq!(merged.edge_count(), 0);
    }

    #[test]
    fn merge_unions_distinct_edges() {
        let mut a = Diagram::new();
        a.ingest("the api calls the database");
        let mut b = Diagram::new();
        b.ingest("the worker reads from the queue");
        let merged = merge_diagrams(vec![&a, &b]);
        assert!(merged.has_edge("api", "calls", "database"));
        assert!(merged.has_edge("worker", "reads from", "queue"));
        assert_eq!(merged.edge_count(), 2);
    }

    #[test]
    fn merge_deduplicates_overlapping_edges() {
        let mut a = Diagram::new();
        a.ingest("the api calls the database");
        let mut b = Diagram::new();
        b.ingest("the api calls the database. the bot uses the api.");
        let merged = merge_diagrams(vec![&a, &b]);
        assert_eq!(merged.edge_count(), 2, "duplicate edge should appear once");
    }

    // ── 5. format_session_recall ──────────────────────────────────

    fn rec(asker: &str, q: &str, a: &str, ts: i64) -> Recollection {
        Recollection {
            asker: asker.into(),
            question: q.into(),
            answer: a.into(),
            ts,
        }
    }

    #[test]
    fn session_recall_empty_returns_none() {
        assert!(format_session_recall(&[]).is_none());
    }

    #[test]
    fn session_recall_uses_most_recent_question() {
        let recs = vec![
            rec("chad", "what is voronoi", "...", 100),
            rec("chad", "how does fts5 rank", "...", 200),
        ];
        let s = format_session_recall(&recs).unwrap();
        assert!(s.contains("fts5"), "expected most recent question, got {s:?}");
        assert!(!s.contains("voronoi"));
    }

    // ── discussion mode trigger ──────────────────────────────────

    #[test]
    fn discussion_trigger_matches_exact_phrase() {
        assert!(is_discussion_trigger("Discuss it for a minute"));
        assert!(is_discussion_trigger("talk it out among yourselves"));
    }

    #[test]
    fn discussion_trigger_case_insensitive() {
        assert!(is_discussion_trigger("DEBATE THIS for me"));
    }

    // ── extract_peer_handoff ─────────────────────────────────────

    fn peers(items: &[&str]) -> std::collections::HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn handoff_finds_trailing_peer_with_question() {
        let p = peers(&["utopia", "narrator"]);
        let r = extract_peer_handoff(
            "I see the pattern. Utopia, do you have the counter-evidence?",
            &p,
        )
        .unwrap();
        assert_eq!(r.0, "utopia");
        assert!(r.1.contains("counter-evidence"));
    }

    #[test]
    fn handoff_returns_none_when_no_peer_mentioned() {
        let p = peers(&["utopia", "narrator"]);
        assert!(extract_peer_handoff("That's the whole story.", &p).is_none());
    }

    #[test]
    fn handoff_prefers_last_peer_when_multiple() {
        let p = peers(&["utopia", "narrator"]);
        let r = extract_peer_handoff(
            "Utopia made a point earlier. Narrator, your read?",
            &p,
        )
        .unwrap();
        assert_eq!(r.0, "narrator");
    }

    #[test]
    fn handoff_handles_bare_name_at_end() {
        // "...Utopia?" — body is empty. Caller will synthesise a
        // generic continuation.
        let p = peers(&["utopia", "narrator"]);
        let r =
            extract_peer_handoff("The disagreement is legible. Utopia?", &p).unwrap();
        assert_eq!(r.0, "utopia");
        assert!(r.1.is_empty(), "got body {:?}", r.1);
    }

    #[test]
    fn handoff_word_boundary_does_not_match_substring() {
        let p = peers(&["utopia"]);
        // "utopian" should NOT match the peer "utopia".
        assert!(
            extract_peer_handoff("That sounds utopian.", &p).is_none(),
            "substring match would be a false positive"
        );
    }

    #[test]
    fn discussion_trigger_rejects_unrelated_speech() {
        assert!(!is_discussion_trigger("oblivion, what about the schedule"));
        assert!(!is_discussion_trigger("the weather looks great today"));
        assert!(!is_discussion_trigger("I'm undecided about it"));
    }

    #[test]
    fn session_recall_trims_trailing_question_mark() {
        let recs = vec![rec("chad", "what is voronoi?", "...", 100)];
        let s = format_session_recall(&recs).unwrap();
        assert!(!s.contains("voronoi?"), "should strip the ?");
        assert!(s.contains("voronoi"));
    }
}
