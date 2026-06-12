//! Deciding whether a line of chat addresses the agent by name.
//!
//! In a voice call people address the agent by *talking* — "eliza, what
//! did we decide?" — and the speech-to-text pass rarely spells the name
//! the same way twice. [`extract_addressed`] is the tolerant matcher: it
//! accepts the name as word 0 (or word 1 after one filler word), allows
//! the name to be split across two words, and forgives small edit-distance
//! mishearings.

/// Filler / speech-to-text-noise words that may precede the name —
/// "hey eliza", "um, eliza", or whisper rendering "Eliza" as "in miza".
/// Two of these may stack ("OK so eliza", "yeah hey eliza") — the
/// matcher tries up to two leading fillers before giving up.
const LEADING_FILLERS: &[&str] = &[
    "hey", "hi", "hello", "ok", "okay", "so", "um", "uh", "well", "yo", "and", "but", "the", "a",
    "in", "at", "to", "now", "oh", "yeah", "yes", "alright", "right", "listen", "sorry", "look",
    "say", "wait", "actually", "anyway", "still", "then", "ah", "mm", "mmm", "hmm", "huh", "like",
];

/// Levenshtein edit distance between two char sequences.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Lowercase a word, keeping only its letters and digits.
fn normalize_word(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Whether `cand` is the agent's name, allowing for STT mishearings —
/// whisper rarely spells a name the same way twice.
fn name_matches(cand: &str, nick: &str) -> bool {
    if cand.chars().count() < 3 {
        return false;
    }
    if cand == nick {
        return true;
    }
    // Containment branch: STT often hallucinates extra letters around
    // a name ("u-topia" → "eutopia", "utopia" → "zootopia") rather
    // than mis-spelling the name itself. If the nick appears
    // contiguously inside the candidate, treat as a match — provided
    // the nick is long enough (≥5 chars) that random words won't
    // collide. This unblocks the live demo: "Zootopia" /
    // "Eutopia" / "you-topia" all reach Utopia.
    if nick.chars().count() >= 5 && cand.len() >= nick.len() && cand.contains(nick) {
        return true;
    }
    // Edit-distance tolerance scales with name length. Long names
    // (≥7 chars) tolerate up to 3 edits — enough to land
    // "narator"→"narrator", "obliviion"→"oblivion", and similar STT
    // duplications.
    let tol = match nick.chars().count() {
        0..=3 => 0,
        4 => 1,
        5..=6 => 2,
        _ => 3,
    };
    edit_distance(cand, nick) <= tol
}

/// If `text` addresses the agent (named `nick`) at the start, return the
/// remainder — the actual question. Tolerant of speech-to-text
/// mishearings of the name and of one leading filler word ("hey eliza",
/// "in miza" → addressed); the name may also be split across two words.
/// Case-insensitive. `None` when the message isn't addressed to the agent
/// or has nothing after the name.
pub fn extract_addressed(text: &str, nick: &str) -> Option<String> {
    let nick = normalize_word(nick);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    // The name may be word 0, or words 1-2 preceded by up to two filler
    // words ("OK so eliza", "yeah hey eliza"); STT sometimes splits it
    // across two words ("a lisa" → "alisa"). Walking the skip range
    // top-down (0, 1, 2) means the earliest match wins, which is what
    // a human would intuit ("the eliza" is a mention, not an address).
    for skip in [0usize, 1, 2] {
        if skip >= words.len() {
            break;
        }
        // Every word before the candidate must itself be a known filler;
        // otherwise we're in mid-sentence territory and shouldn't match.
        let preceding_ok = words[..skip]
            .iter()
            .all(|w| LEADING_FILLERS.contains(&normalize_word(w).as_str()));
        if !preceding_ok {
            continue;
        }
        for take in [1usize, 2] {
            if skip + take > words.len() {
                continue;
            }
            let cand: String = words[skip..skip + take]
                .iter()
                .map(|w| normalize_word(w))
                .collect();
            if name_matches(&cand, &nick) {
                let rest = words[skip + take..].join(" ");
                let rest = rest
                    .trim_start_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
                    .trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// Strict variant of [`extract_addressed`]: the candidate words must
/// EQUAL the normalized nick — no edit-distance tolerance, no
/// containment. Same skip/take walk (fillers, split names), so
/// "hey o live, ..." still reaches "olive", but "a live" (distance 1)
/// does not.
///
/// Use this when matching OTHER agents' names to decide a line is *not*
/// for you: a fuzzy match there suppresses your own answer, so a false
/// positive silences the bot. (Matching your OWN name stays fuzzy —
/// there a false negative is the expensive error.)
pub fn extract_addressed_exact(text: &str, nick: &str) -> Option<String> {
    let nick = normalize_word(nick);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    for skip in [0usize, 1, 2] {
        if skip >= words.len() {
            break;
        }
        let preceding_ok = words[..skip]
            .iter()
            .all(|w| LEADING_FILLERS.contains(&normalize_word(w).as_str()));
        if !preceding_ok {
            continue;
        }
        for take in [1usize, 2] {
            if skip + take > words.len() {
                continue;
            }
            let cand: String = words[skip..skip + take]
                .iter()
                .map(|w| normalize_word(w))
                .collect();
            if cand == nick {
                let rest = words[skip + take..].join(" ");
                let rest = rest
                    .trim_start_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
                    .trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
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
            extract_addressed("eliza: what did we decide?", "eliza").as_deref(),
            Some("what did we decide?")
        );
    }

    #[test]
    fn extracts_comma_and_at_and_bare_forms() {
        assert_eq!(
            extract_addressed("eliza, summarize", "eliza").as_deref(),
            Some("summarize")
        );
        assert_eq!(
            extract_addressed("@eliza who is talking", "eliza").as_deref(),
            Some("who is talking")
        );
        assert_eq!(
            extract_addressed("eliza recap please", "eliza").as_deref(),
            Some("recap please")
        );
    }

    #[test]
    fn case_insensitive_on_nick() {
        assert_eq!(
            extract_addressed("Eliza: hi there", "eliza").as_deref(),
            Some("hi there")
        );
        assert_eq!(
            extract_addressed("eliza: hi", "ELIZA").as_deref(),
            Some("hi")
        );
    }

    #[test]
    fn tolerates_stt_mishearings_of_the_name() {
        // Whisper rarely spells "Eliza" the same way twice; small
        // mishearings and one leading filler word must still register.
        for heard in [
            "eliza hello",
            "elisa hello",
            "aliza hello",
            "in miza hello",
            "hey eliza hello",
        ] {
            assert_eq!(
                extract_addressed(heard, "eliza").as_deref(),
                Some("hello"),
                "should detect address in {heard:?}"
            );
        }
    }

    #[test]
    fn two_leading_filler_words_still_match() {
        // People in voice calls trail off and hedge — "OK so Eliza,",
        // "Yeah hey Eliza,", "Um well Eliza". Two leading fillers must
        // still register; three is too far (probably a real sentence).
        for heard in [
            "ok so eliza what time is it",
            "yeah hey eliza what time is it",
            "um well eliza what time is it",
            "alright now eliza what time is it",
        ] {
            assert_eq!(
                extract_addressed(heard, "eliza").as_deref(),
                Some("what time is it"),
                "should detect address in {heard:?}"
            );
        }
    }

    #[test]
    fn ignores_unaddressed_or_mid_sentence_mentions() {
        assert_eq!(extract_addressed("hello everyone", "eliza"), None);
        assert_eq!(
            extract_addressed("ask the eliza later", "eliza"),
            None,
            "a mention mid-sentence is not an address"
        );
    }

    #[test]
    fn ignores_bare_nick_with_no_question() {
        // Just the nick, nothing after → not a question.
        assert_eq!(extract_addressed("eliza", "eliza"), None);
        assert_eq!(extract_addressed("eliza: ", "eliza"), None);
        assert_eq!(extract_addressed("", "eliza"), None);
    }

    #[test]
    fn short_nick_tolerates_no_mishearing() {
        // A 3-char nick has zero edit-distance tolerance — a one-letter
        // slip must not register as an address.
        assert_eq!(
            extract_addressed("bot status", "bot").as_deref(),
            Some("status")
        );
        assert_eq!(extract_addressed("bat status", "bot"), None);
    }

    #[test]
    fn edit_distance_is_symmetric_and_zero_on_equal() {
        assert_eq!(edit_distance("eliza", "eliza"), 0);
        assert_eq!(edit_distance("eliza", "elisa"), 1);
        assert_eq!(edit_distance("eliza", "aliza"), 1);
        assert_eq!(
            edit_distance("kitten", "sitting"),
            edit_distance("sitting", "kitten")
        );
    }

    #[test]
    fn exact_variant_rejects_fuzzy_matches() {
        // The live misfire: "A live" normalize-joins to "alive", edit
        // distance 1 from "olive" — the fuzzy matcher takes it, the
        // exact one must not.
        assert_eq!(
            extract_addressed_exact("A live voice call with the assistant", "olive"),
            None
        );
        assert_eq!(
            extract_addressed_exact("elisa hello", "eliza"),
            None,
            "no edit tolerance"
        );
        assert_eq!(
            extract_addressed_exact("zootopia what's up", "utopia"),
            None,
            "no containment"
        );
        // Exact spellings still work, including fillers and split names.
        assert_eq!(
            extract_addressed_exact("olive, what's 2+2", "olive").as_deref(),
            Some("what's 2+2")
        );
        assert_eq!(
            extract_addressed_exact("hey olive what's 2+2", "olive").as_deref(),
            Some("what's 2+2")
        );
        assert_eq!(
            extract_addressed_exact("o live what's 2+2", "olive").as_deref(),
            Some("what's 2+2"),
            "split-name exact join still matches"
        );
    }

    #[test]
    fn normalize_word_strips_punctuation_and_case() {
        assert_eq!(normalize_word("@Eliza,"), "eliza");
        assert_eq!(normalize_word("HELLO!"), "hello");
        assert_eq!(normalize_word("...---"), "");
    }
}
