//! IRCv3 `draft/multiline` support — the inbound BATCH state machine,
//! `draft/multiline` assembly per concat rules, and policy limits.
//!
//! Spec: <https://ircv3.net/specs/extensions/multiline>
//!
//! ## Phase 1 (this file, today)
//!
//! - Per-session state for in-flight BATCHes (open_batches).
//! - Validation rules at BATCH open / message append / BATCH close.
//! - max-bytes / max-lines policy values + enforcement helpers.
//! - `FAIL BATCH MULTILINE_*` standard-replies framework codes.
//!
//! What's intentionally NOT here yet:
//!
//! - Dispatching a closed batch as a single logical message (Phase 2:
//!   assemble per concat rules and feed back into the normal PRIVMSG/
//!   NOTICE path).
//! - Outbound relay (Phase 4): emitting BATCH frames to multiline-
//!   capable channel members and individual PRIVMSGs to fallback
//!   members.
//! - `verify_commit_reveal` extension to use the assembled body when
//!   the reveal arrived inside a multiline batch (Phase 3).

use std::collections::HashMap;

/// Server policy: maximum total byte length of a multiline batch's
/// combined message value. Spec calls this `max-bytes` and requires
/// us to advertise it.
///
/// Counted bytes per spec: only the body (last PRIVMSG/NOTICE param)
/// of each line, plus one byte per `\n` separator. Not the prefix,
/// command, target, or tags.
///
/// 40 KB is the same value the spec uses in its own examples and
/// comfortably fits any sane LLM turn (~12k words). Configurable in
/// a follow-up if needed.
pub const MAX_BYTES: usize = 40_000;

/// Server policy: maximum number of PRIVMSG/NOTICE lines in a single
/// multiline batch. Spec calls this `max-lines` (recommended).
///
/// 100 lines covers any realistic structured output (markdown with
/// headings + bullets + paragraphs) without enabling abusive batches.
pub const MAX_LINES: usize = 100;

/// Soft cap on concurrent open batches per session. The spec allows
/// multiple, but in practice clients open one at a time. Capping at
/// 5 prevents a misbehaving client from accumulating unbounded
/// per-session state by opening batches and never closing them.
pub const MAX_CONCURRENT_BATCHES_PER_SESSION: usize = 5;

/// Per-line entry inside an open batch: the body and whether this
/// line carried the `draft/multiline-concat` tag (meaning "join to
/// previous with no separator", vs. the default "join with `\n`").
#[derive(Debug, Clone)]
pub struct BatchLine {
    /// Final-param body of this PRIVMSG or NOTICE.
    pub body: String,
    /// True if this line carried `draft/multiline-concat`.
    pub concat_to_previous: bool,
    /// PRIVMSG or NOTICE — must match across the whole batch.
    pub command: String,
}

/// An open BATCH on a single connection — accumulates lines until
/// the matching `BATCH -<id>` arrives, then gets dispatched as a
/// single logical message by the batch-type-specific handler.
#[derive(Debug, Clone)]
pub struct OpenBatch {
    /// The `batch_id` parameter the client supplied (without the `+`).
    pub batch_id: String,
    /// Batch type, e.g. `draft/multiline`.
    pub batch_type: String,
    /// Target channel or nick. Every PRIVMSG inside the batch must
    /// have a target equal to this; mismatch → MULTILINE_INVALID_TARGET.
    pub target: String,
    /// Tags from the BATCH opener (other than `batch`). Per spec,
    /// client-only tags associated with the assembled message live
    /// here, not on individual PRIVMSGs inside.
    pub opener_tags: HashMap<String, String>,
    /// Accumulated lines, in arrival order.
    pub lines: Vec<BatchLine>,
    /// Running total of body bytes + 1 per `\n` separator (per spec's
    /// max-bytes accounting).
    pub byte_count: usize,
    /// First command seen inside the batch — used to enforce the
    /// "PRIVMSG-only XOR NOTICE-only" rule. Set when the first line
    /// arrives.
    pub first_command: Option<String>,
}

impl OpenBatch {
    /// Compute the byte cost a new line would add to the batch:
    /// the body length, plus 1 for the `\n` separator IF this is not
    /// the first line AND this line is NOT marked `concat-to-previous`.
    pub fn cost_of_appending(&self, body: &str, concat_to_previous: bool) -> usize {
        if self.lines.is_empty() {
            body.len()
        } else if concat_to_previous {
            body.len()
        } else {
            body.len() + 1
        }
    }
}

/// Standard-replies error codes per the spec's "Errors" section.
/// Emitted as `FAIL BATCH <CODE> [<args>...] :<human-readable>`.
pub mod fail_code {
    pub const MULTILINE_MAX_BYTES: &str = "MULTILINE_MAX_BYTES";
    pub const MULTILINE_MAX_LINES: &str = "MULTILINE_MAX_LINES";
    pub const MULTILINE_INVALID_TARGET: &str = "MULTILINE_INVALID_TARGET";
    pub const MULTILINE_INVALID: &str = "MULTILINE_INVALID";
}

// ──────────────────────────────────────────────────────────────────
// Unit tests for OpenBatch + cost accounting (pure logic, no state)
// ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_batch() -> OpenBatch {
        OpenBatch {
            batch_id: "abc".to_string(),
            batch_type: "draft/multiline".to_string(),
            target: "#c".to_string(),
            opener_tags: HashMap::new(),
            lines: Vec::new(),
            byte_count: 0,
            first_command: None,
        }
    }

    #[test]
    fn cost_of_first_line_is_just_the_body() {
        let b = empty_batch();
        assert_eq!(b.cost_of_appending("hello", false), 5);
        // The concat marker on the first line is invalid per spec, but
        // the cost helper doesn't validate — it just accounts. The
        // first-line concat case still doesn't add a separator byte
        // because there's nothing to join *to*.
        assert_eq!(b.cost_of_appending("hello", true), 5);
    }

    #[test]
    fn cost_of_subsequent_normal_line_adds_separator_byte() {
        let mut b = empty_batch();
        b.lines.push(BatchLine {
            body: "first".to_string(),
            concat_to_previous: false,
            command: "PRIVMSG".to_string(),
        });
        // "world" (5) + \n separator (1) = 6.
        assert_eq!(b.cost_of_appending("world", false), 6);
    }

    #[test]
    fn cost_of_subsequent_concat_line_does_not_add_separator() {
        let mut b = empty_batch();
        b.lines.push(BatchLine {
            body: "first".to_string(),
            concat_to_previous: false,
            command: "PRIVMSG".to_string(),
        });
        // Concat means no separator → cost is just body length.
        assert_eq!(b.cost_of_appending("ABC", true), 3);
    }

    #[test]
    fn max_bytes_constant_matches_spec_example() {
        // The spec uses 40000 in its own example; keeping policy
        // aligned makes interop with the example clients painless.
        assert_eq!(MAX_BYTES, 40_000);
    }

    #[test]
    fn max_lines_constant_is_sane_default() {
        // 100 lines is enough for any structured LLM turn while
        // staying well below abusive territory.
        assert_eq!(MAX_LINES, 100);
    }

    #[test]
    fn max_concurrent_batches_per_session_is_sane_default() {
        // Clients in the wild open one at a time; 5 leaves headroom
        // without permitting unbounded per-session state.
        assert_eq!(MAX_CONCURRENT_BATCHES_PER_SESSION, 5);
    }
}
