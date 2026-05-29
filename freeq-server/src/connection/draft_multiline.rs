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
// BATCH state machine handlers (Phase 1b)
//
// These functions are intentionally pure-state — they take a
// `SharedState` and the raw inputs, mutate `state.open_batches`, and
// return success or one of the spec's MULTILINE_* error codes. The
// caller in connection/mod.rs is responsible for parsing the wire
// message and emitting `FAIL BATCH <code>` replies. Keeping the
// state mutations here makes the logic testable without any wire
// plumbing.
// ──────────────────────────────────────────────────────────────────

use std::sync::Arc;

use crate::server::SharedState;

/// Outcome of a batch operation. The `Err` variant carries the
/// `MULTILINE_*` code the caller should emit as `FAIL BATCH <code>
/// [<args>] :<reason>`. Static strings so they can be matched on by
/// tests without allocation.
pub type BatchResult<T> = Result<T, BatchError>;

#[derive(Debug, PartialEq, Eq)]
pub enum BatchError {
    /// `MULTILINE_INVALID` — anything that doesn't fit the more
    /// specific buckets (unknown batch id, blank-line rules, type
    /// mismatch within a batch, etc.).
    Invalid(&'static str),
    /// `MULTILINE_MAX_BYTES <limit>` — appending this line would
    /// blow the per-batch byte budget.
    MaxBytes,
    /// `MULTILINE_MAX_LINES <limit>` — appending this line would
    /// exceed the per-batch line count.
    MaxLines,
    /// `MULTILINE_INVALID_TARGET <batch-target> <line-target>` —
    /// a PRIVMSG within the batch had a target that doesn't match
    /// the batch's declared target.
    InvalidTarget { batch_target: String, line_target: String },
}

/// Open a new BATCH on the given session. Validates the batch type
/// (only `draft/multiline` for now), checks the per-session concurrent
/// cap, and rejects duplicates of an already-open batch id.
///
/// Caller passes the tags from the BATCH opener (sans `batch`) —
/// those tags carry the client-only metadata for the assembled
/// logical message (per spec § "Message tags").
pub fn handle_batch_open(
    state: &Arc<SharedState>,
    session_id: &str,
    batch_id: &str,
    batch_type: &str,
    target: &str,
    opener_tags: HashMap<String, String>,
) -> BatchResult<()> {
    if batch_id.is_empty() {
        return Err(BatchError::Invalid("empty batch id"));
    }
    // Phase 1: we only recognise `draft/multiline`. Other types (e.g.
    // future `draft/labeled` batches) would land their own validators
    // here.
    if batch_type != "draft/multiline" {
        return Err(BatchError::Invalid("unknown batch type"));
    }
    if target.is_empty() {
        return Err(BatchError::Invalid("missing batch target"));
    }

    let key = (session_id.to_string(), batch_id.to_string());
    let mut open = state.open_batches.lock();

    // Reject duplicate batch id on the same session.
    if open.contains_key(&key) {
        return Err(BatchError::Invalid("batch id already open"));
    }

    // Cap concurrent open batches per session — prevents a misbehaving
    // client from accumulating unbounded per-session state.
    let session_open_count = open.keys().filter(|(sid, _)| sid == session_id).count();
    if session_open_count >= MAX_CONCURRENT_BATCHES_PER_SESSION {
        return Err(BatchError::Invalid("too many concurrent batches"));
    }

    open.insert(
        key,
        OpenBatch {
            batch_id: batch_id.to_string(),
            batch_type: batch_type.to_string(),
            target: target.to_string(),
            opener_tags,
            lines: Vec::new(),
            byte_count: 0,
            first_command: None,
        },
    );
    Ok(())
}

/// Outcome of a PRIVMSG/NOTICE arriving on a session: was it part of
/// an open batch (and absorbed), or should the caller dispatch it as
/// a normal standalone message?
#[derive(Debug)]
pub enum RouteOutcome {
    /// Message had a `batch=<id>` tag matching an open batch on this
    /// session, and was appended successfully. Caller should NOT
    /// dispatch it as a standalone PRIVMSG/NOTICE.
    Absorbed,
    /// No `batch` tag, or the tag pointed at an unknown batch id.
    /// Caller dispatches as a normal standalone message.
    NotInBatch,
    /// Message claimed membership in an open batch but failed
    /// validation (target mismatch, byte/line cap exceeded, blank-
    /// line concat, type mixing). Caller should emit FAIL with the
    /// supplied error and SHOULD NOT dispatch the message.
    Error(BatchError),
}

/// Route an inbound PRIVMSG/NOTICE: if it carries a `batch=<id>` tag
/// matching an open batch on this session, append it; otherwise
/// signal NotInBatch so the caller dispatches it normally.
///
/// `concat_to_previous` reflects whether the line carried the
/// `draft/multiline-concat` tag.
pub fn try_route_to_batch(
    state: &Arc<SharedState>,
    session_id: &str,
    msg_tags: &HashMap<String, String>,
    command: &str,
    target: &str,
    body: &str,
    concat_to_previous: bool,
) -> RouteOutcome {
    let batch_id = match msg_tags.get("batch") {
        Some(b) => b,
        None => return RouteOutcome::NotInBatch,
    };

    let key = (session_id.to_string(), batch_id.to_string());
    let mut open = state.open_batches.lock();
    let entry = match open.get_mut(&key) {
        Some(e) => e,
        // Per the base BATCH spec, a `batch=<id>` tag pointing at no
        // open batch is silently treated as a normal standalone
        // message (the receiver simply doesn't know about the batch).
        None => return RouteOutcome::NotInBatch,
    };

    // Validation per spec:
    // - Target must match the batch target.
    if target != entry.target {
        return RouteOutcome::Error(BatchError::InvalidTarget {
            batch_target: entry.target.clone(),
            line_target: target.to_string(),
        });
    }
    // - Batch contains only PRIVMSG or only NOTICE — first line sets
    //   the type; subsequent lines must match.
    match &entry.first_command {
        Some(seen) if seen != command => {
            return RouteOutcome::Error(BatchError::Invalid(
                "cannot mix PRIVMSG and NOTICE in a draft/multiline batch",
            ));
        }
        None => {
            entry.first_command = Some(command.to_string());
        }
        _ => {}
    }
    // - Blank line with concat tag is invalid; entirely-blank message
    //   guard is enforced at close-time.
    if concat_to_previous && body.is_empty() {
        return RouteOutcome::Error(BatchError::Invalid(
            "cannot send a blank line with the multiline concat tag",
        ));
    }
    // - First-line concat is invalid (nothing to join to).
    if entry.lines.is_empty() && concat_to_previous {
        return RouteOutcome::Error(BatchError::Invalid(
            "first line cannot carry draft/multiline-concat",
        ));
    }
    // - Byte budget.
    let cost = entry.cost_of_appending(body, concat_to_previous);
    if entry.byte_count + cost > MAX_BYTES {
        return RouteOutcome::Error(BatchError::MaxBytes);
    }
    // - Line budget (each line counts, including concat'd ones, per
    //   spec § "Batch types").
    if entry.lines.len() + 1 > MAX_LINES {
        return RouteOutcome::Error(BatchError::MaxLines);
    }

    entry.byte_count += cost;
    entry.lines.push(BatchLine {
        body: body.to_string(),
        concat_to_previous,
        command: command.to_string(),
    });
    RouteOutcome::Absorbed
}

/// Close an open BATCH and return its accumulated state to the
/// caller. The caller is responsible for dispatching the closed
/// batch — Phase 2 plugs in `draft/multiline` assembly + dispatch as
/// a single logical PRIVMSG/NOTICE. For now (Phase 1), the caller
/// just drops the returned batch.
///
/// Final validation: an entirely-blank batch (no lines OR every line
/// has an empty body) is rejected per spec.
pub fn handle_batch_close(
    state: &Arc<SharedState>,
    session_id: &str,
    batch_id: &str,
) -> BatchResult<OpenBatch> {
    let key = (session_id.to_string(), batch_id.to_string());
    let batch = state
        .open_batches
        .lock()
        .remove(&key)
        .ok_or(BatchError::Invalid("unknown batch id"))?;

    // Spec: "Clients MUST NOT send messages consisting entirely of
    // blank lines." Reject at close — we couldn't tell the message
    // was all-blank until now.
    let all_blank = batch.lines.iter().all(|l| l.body.is_empty());
    if !batch.lines.is_empty() && all_blank {
        return Err(BatchError::Invalid(
            "batch consists entirely of blank lines",
        ));
    }
    Ok(batch)
}

/// Concatenate the lines of a closed `draft/multiline` batch into a
/// single body, per the spec's join rules:
///
/// > The combined message value of a multiline batch is defined as
/// > the concatenation of the messages from each individual line
/// > within the batch. Line messages are joined by a single line
/// > feed (\n) byte unless the draft/multiline-concat message tag is
/// > sent, in which case that line's message is directly joined with
/// > the previous line's message with no separation.
///
/// — <https://ircv3.net/specs/extensions/multiline> § Batch types
pub fn assemble_body(batch: &OpenBatch) -> String {
    let mut out = String::with_capacity(batch.byte_count);
    for (i, line) in batch.lines.iter().enumerate() {
        if i > 0 && !line.concat_to_previous {
            out.push('\n');
        }
        out.push_str(&line.body);
    }
    out
}

/// Snapshot the number of open batches for a session — used by the
/// per-session concurrent-cap check and by tests.
#[cfg(test)]
fn count_open_batches(state: &Arc<SharedState>, session_id: &str) -> usize {
    state
        .open_batches
        .lock()
        .keys()
        .filter(|(sid, _)| sid == session_id)
        .count()
}

/// Top-level entry point for the inbound `BATCH` command. Parses the
/// reference-tag, dispatches to `handle_batch_open` (`+`) or
/// `handle_batch_close` (`-`), and emits a `FAIL BATCH <code>` reply
/// on validation errors.
///
/// On successful close of a `draft/multiline` batch, assembles the
/// accumulated lines per the spec's concat rules and re-feeds the
/// result through the normal `handle_privmsg` path — so the rest of
/// the server (history, broadcast, signing, commit-reveal verification,
/// etc.) sees one logical PRIVMSG/NOTICE regardless of how the sender
/// chunked it on the wire.
pub fn handle_batch_command(
    conn: &super::Connection,
    msg: &crate::irc::Message,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let reference = match msg.params.first() {
        Some(r) if !r.is_empty() => r,
        _ => {
            send_fail(
                state,
                server_name,
                session_id,
                send,
                fail_code::MULTILINE_INVALID,
                &[],
                "BATCH reference-tag required",
            );
            return;
        }
    };

    let (sign, batch_id) = reference.split_at(1);
    match sign {
        "+" => {
            // BATCH +<id> <type> [<params>...]
            let batch_type = msg.params.get(1).map(String::as_str).unwrap_or("");
            // For `draft/multiline` the third parameter is the target.
            let target = msg.params.get(2).map(String::as_str).unwrap_or("");

            // Per spec: client-only tags (`+`-prefixed) on the opener
            // carry the assembled message's metadata. Strip the `batch`
            // tag itself (won't be on an opener anyway) and store the rest.
            let opener_tags: HashMap<String, String> = msg
                .tags
                .iter()
                .filter(|(k, _)| k.as_str() != "batch")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            if let Err(err) =
                handle_batch_open(state, session_id, batch_id, batch_type, target, opener_tags)
            {
                send_batch_error(state, server_name, session_id, send, &err);
            }
        }
        "-" => {
            // BATCH -<id>
            match handle_batch_close(state, session_id, batch_id) {
                Ok(closed_batch) => {
                    dispatch_assembled_batch(conn, &closed_batch, state);
                }
                Err(err) => {
                    send_batch_error(state, server_name, session_id, send, &err);
                }
            }
        }
        _ => {
            send_fail(
                state,
                server_name,
                session_id,
                send,
                fail_code::MULTILINE_INVALID,
                &[],
                "BATCH reference-tag must start with + or -",
            );
        }
    }
}

/// Emit a `FAIL BATCH <code> [<args>] :<reason>` reply for a
/// `BatchError`, picking the correct spec code + arguments.
pub fn send_batch_error(
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
    err: &BatchError,
) {
    match err {
        BatchError::MaxBytes => {
            let limit = MAX_BYTES.to_string();
            send_fail(
                state,
                server_name,
                session_id,
                send,
                fail_code::MULTILINE_MAX_BYTES,
                &[&limit],
                "Multiline batch byte limit exceeded",
            );
        }
        BatchError::MaxLines => {
            let limit = MAX_LINES.to_string();
            send_fail(
                state,
                server_name,
                session_id,
                send,
                fail_code::MULTILINE_MAX_LINES,
                &[&limit],
                "Multiline batch line limit exceeded",
            );
        }
        BatchError::InvalidTarget {
            batch_target,
            line_target,
        } => {
            send_fail(
                state,
                server_name,
                session_id,
                send,
                fail_code::MULTILINE_INVALID_TARGET,
                &[batch_target.as_str(), line_target.as_str()],
                "Invalid multiline target",
            );
        }
        BatchError::Invalid(reason) => {
            send_fail(
                state,
                server_name,
                session_id,
                send,
                fail_code::MULTILINE_INVALID,
                &[],
                reason,
            );
        }
    }
}

fn send_fail(
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
    code: &str,
    extra_args: &[&str],
    human_reason: &str,
) {
    // Standard-replies frame: `FAIL <command> <code> [<args>...] :<reason>`.
    let mut params: Vec<&str> = vec!["BATCH", code];
    params.extend_from_slice(extra_args);
    params.push(human_reason);
    let reply = crate::irc::Message::from_server(server_name, "FAIL", params);
    send(state, session_id, format!("{reply}\r\n"));
}

/// Take a closed batch and re-dispatch it as a single logical
/// PRIVMSG/NOTICE. The body is assembled per concat rules; the
/// client-only tags from the BATCH opener carry through; the
/// command (PRIVMSG vs NOTICE) is whatever the first line inside the
/// batch used. The downstream `handle_privmsg` does its normal thing
/// — flood protection, history persistence, signing, broadcast — so
/// the rest of the server treats the assembled message identically
/// to a single-PRIVMSG send.
fn dispatch_assembled_batch(
    conn: &super::Connection,
    batch: &OpenBatch,
    state: &Arc<SharedState>,
) {
    // first_command is set on first append (try_route_to_batch). A
    // batch with zero lines wouldn't have one, but the close-time
    // entirely-blank-batch guard rejects that case before we get
    // here, so first_command is always Some when we reach dispatch.
    let command = batch
        .first_command
        .as_deref()
        .unwrap_or("PRIVMSG");
    let body = assemble_body(batch);
    super::messaging::handle_privmsg(
        conn,
        command,
        &batch.target,
        &body,
        &batch.opener_tags,
        state,
    );
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

    // ──────────────────────────────────────────────────────────────
    // State machine tests (against a real SharedState)
    // ──────────────────────────────────────────────────────────────

    use crate::server::SharedState;
    use parking_lot::Mutex;
    use std::collections::HashSet;
    use std::sync::Arc;

    /// Minimal SharedState for these state-machine tests. Mirrors the
    /// `test_state()` helper in s2s_adversarial_tests but with only
    /// the fields the batch handlers actually touch initialised; the
    /// rest are placeholder defaults.
    fn test_state() -> Arc<SharedState> {
        let config = crate::config::ServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            server_name: "test-multiline".to_string(),
            challenge_timeout_secs: 60,
            ..Default::default()
        };
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        Arc::new(SharedState {
            server_name: config.server_name.clone(),
            challenge_store: crate::sasl::ChallengeStore::new(60),
            did_resolver: freeq_sdk::did::DidResolver::static_map(HashMap::new()),
            connections: Mutex::new(HashMap::new()),
            nick_to_session: Mutex::new(crate::server::NickMap::new()),
            session_dids: Mutex::new(HashMap::new()),
            did_sessions: Mutex::new(HashMap::new()),
            did_nicks: Mutex::new(HashMap::new()),
            nick_owners: Mutex::new(HashMap::new()),
            session_handles: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            cap_message_tags: Mutex::new(HashSet::new()),
            cap_multi_prefix: Mutex::new(HashSet::new()),
            cap_echo_message: Mutex::new(HashSet::new()),
            cap_server_time: Mutex::new(HashSet::new()),
            cap_batch: Mutex::new(HashSet::new()),
            cap_draft_multiline: Mutex::new(HashSet::new()),
            open_batches: Mutex::new(HashMap::new()),
            cap_account_notify: Mutex::new(HashSet::new()),
            cap_extended_join: Mutex::new(HashSet::new()),
            cap_away_notify: Mutex::new(HashSet::new()),
            cap_account_tag: Mutex::new(HashSet::new()),
            server_opers: Mutex::new(HashSet::new()),
            session_actor_class: Mutex::new(HashMap::new()),
            provenance_declarations: Mutex::new(HashMap::new()),
            agent_presence: Mutex::new(HashMap::new()),
            agent_heartbeats: Mutex::new(HashMap::new()),
            av_instances_per_conn: Mutex::new(HashMap::new()),
            oauth_pending: Mutex::new(HashMap::new()),
            oauth_complete: Mutex::new(HashMap::new()),
            web_auth_tokens: Mutex::new(HashMap::new()),
            web_sessions: Mutex::new(HashMap::new()),
            login_pending: Mutex::new(HashMap::new()),
            linked_identities: Mutex::new(HashMap::new()),
            login_completions: Mutex::new(HashMap::new()),
            session_iroh_ids: Mutex::new(HashMap::new()),
            session_away: Mutex::new(HashMap::new()),
            server_iroh_id: Mutex::new(Some("test-server-id".to_string())),
            iroh_endpoint: Mutex::new(None),
            iroh_router: Mutex::new(None),
            av_sessions: Mutex::new(crate::av::AvSessionManager::new()),
            av_media: Mutex::new(None),
            s2s_manager: Mutex::new(None),
            cluster_doc: crate::crdt::ClusterDoc::new("test-server-id"),
            db: None,
            config,
            plugin_manager: crate::plugin::PluginManager::new(),
            policy_engine: None,
            prekey_bundles: Mutex::new(HashMap::new()),
            msg_timestamps: Mutex::new(HashMap::new()),
            ip_connections: Mutex::new(HashMap::new()),
            msg_signing_key: signing_key,
            boot_time: std::time::Instant::now(),
            boot_timestamp: chrono::Utc::now(),
            session_msg_keys: Mutex::new(HashMap::new()),
            did_msg_keys: Mutex::new(HashMap::new()),
            session_client_info: Mutex::new(HashMap::new()),
            upload_tokens: Mutex::new(HashMap::new()),
            ghost_sessions: Mutex::new(HashMap::new()),
            spawned_agents: Mutex::new(HashMap::new()),
            rest_rate_limiter: crate::web::IpRateLimiter::new(30, 60),
        })
    }

    fn empty_tags() -> HashMap<String, String> {
        HashMap::new()
    }

    fn tag(key: &str, value: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(key.to_string(), value.to_string());
        m
    }

    // ── open / close lifecycle ────────────────────────────────────

    #[test]
    fn batch_open_creates_state() {
        let state = test_state();
        assert!(handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#cloudcity",
            empty_tags()
        )
        .is_ok());
        assert_eq!(count_open_batches(&state, "sess-A"), 1);
    }

    #[test]
    fn batch_close_returns_and_removes_state() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        // Need at least one non-blank line for close to succeed.
        let routed = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "hi",
            false,
        );
        assert!(matches!(routed, RouteOutcome::Absorbed));

        let closed = handle_batch_close(&state, "sess-A", "abc").unwrap();
        assert_eq!(closed.lines.len(), 1);
        assert_eq!(closed.lines[0].body, "hi");
        assert_eq!(count_open_batches(&state, "sess-A"), 0);
    }

    #[test]
    fn batch_close_unknown_id_is_invalid() {
        let state = test_state();
        let err = handle_batch_close(&state, "sess-A", "nope").unwrap_err();
        assert!(matches!(err, BatchError::Invalid(_)));
    }

    #[test]
    fn duplicate_batch_open_rejected() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        let err = handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap_err();
        assert!(matches!(err, BatchError::Invalid(_)));
    }

    #[test]
    fn unknown_batch_type_rejected() {
        let state = test_state();
        let err = handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "chathistory",
            "#c",
            empty_tags(),
        )
        .unwrap_err();
        assert!(matches!(err, BatchError::Invalid(_)));
    }

    #[test]
    fn max_concurrent_batches_per_session_enforced() {
        let state = test_state();
        for i in 0..MAX_CONCURRENT_BATCHES_PER_SESSION {
            handle_batch_open(
                &state,
                "sess-A",
                &format!("batch-{i}"),
                "draft/multiline",
                "#c",
                empty_tags(),
            )
            .unwrap();
        }
        let err = handle_batch_open(
            &state,
            "sess-A",
            "one-too-many",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap_err();
        assert!(matches!(err, BatchError::Invalid(_)));
    }

    #[test]
    fn batches_isolated_per_session() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        // Same batch id on a different session is fine — keys are
        // `(session_id, batch_id)`.
        handle_batch_open(
            &state,
            "sess-B",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        assert_eq!(count_open_batches(&state, "sess-A"), 1);
        assert_eq!(count_open_batches(&state, "sess-B"), 1);
    }

    // ── routing (PRIVMSG / NOTICE with batch tag) ──────────────────

    #[test]
    fn message_without_batch_tag_is_not_in_batch() {
        let state = test_state();
        let outcome = try_route_to_batch(
            &state,
            "sess-A",
            &empty_tags(),
            "PRIVMSG",
            "#c",
            "hi",
            false,
        );
        assert!(matches!(outcome, RouteOutcome::NotInBatch));
    }

    #[test]
    fn message_with_batch_tag_for_unknown_id_is_not_in_batch() {
        // Per spec, a `batch=<id>` pointing at no open batch is
        // treated as a normal standalone message.
        let state = test_state();
        let outcome = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "nope"),
            "PRIVMSG",
            "#c",
            "hi",
            false,
        );
        assert!(matches!(outcome, RouteOutcome::NotInBatch));
    }

    #[test]
    fn line_target_mismatch_is_invalid_target() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#cloudcity",
            empty_tags(),
        )
        .unwrap();
        let outcome = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#wrongchan",
            "hi",
            false,
        );
        match outcome {
            RouteOutcome::Error(BatchError::InvalidTarget {
                batch_target,
                line_target,
            }) => {
                assert_eq!(batch_target, "#cloudcity");
                assert_eq!(line_target, "#wrongchan");
            }
            other => panic!("expected InvalidTarget, got {other:?}"),
        }
    }

    #[test]
    fn mixing_privmsg_and_notice_in_one_batch_is_invalid() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "first",
            false,
        );
        let outcome = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "NOTICE",
            "#c",
            "uh oh",
            false,
        );
        assert!(matches!(outcome, RouteOutcome::Error(BatchError::Invalid(_))));
    }

    #[test]
    fn first_line_with_concat_tag_is_invalid() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        let mut tags = tag("batch", "abc");
        tags.insert("draft/multiline-concat".to_string(), String::new());
        let outcome = try_route_to_batch(
            &state,
            "sess-A",
            &tags,
            "PRIVMSG",
            "#c",
            "hello",
            true,
        );
        assert!(matches!(outcome, RouteOutcome::Error(BatchError::Invalid(_))));
    }

    #[test]
    fn blank_line_with_concat_tag_is_invalid() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        // First line: normal, non-blank.
        try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "hi",
            false,
        );
        // Second line: blank + concat — rejected.
        let outcome = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "",
            true,
        );
        assert!(matches!(outcome, RouteOutcome::Error(BatchError::Invalid(_))));
    }

    #[test]
    fn entirely_blank_batch_rejected_at_close() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        // Two blank lines, no concat (so each is "allowed" mid-batch).
        try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "",
            false,
        );
        try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "",
            false,
        );
        let err = handle_batch_close(&state, "sess-A", "abc").unwrap_err();
        assert!(matches!(err, BatchError::Invalid(_)));
    }

    #[test]
    fn byte_budget_enforced_at_append() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        // First line fills the budget exactly.
        let big = "x".repeat(MAX_BYTES);
        let r1 = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            &big,
            false,
        );
        assert!(matches!(r1, RouteOutcome::Absorbed));
        // Any non-concat second line adds the body + 1 separator
        // byte, so even a 0-byte line should push us over.
        let r2 = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "",
            false,
        );
        assert!(matches!(r2, RouteOutcome::Error(BatchError::MaxBytes)));
    }

    // ── assemble_body (concat rules) ──────────────────────────────

    fn line(body: &str, concat: bool) -> BatchLine {
        BatchLine {
            body: body.to_string(),
            concat_to_previous: concat,
            command: "PRIVMSG".to_string(),
        }
    }

    fn batch_with_lines(lines: Vec<BatchLine>) -> OpenBatch {
        OpenBatch {
            batch_id: "x".to_string(),
            batch_type: "draft/multiline".to_string(),
            target: "#c".to_string(),
            opener_tags: HashMap::new(),
            lines,
            byte_count: 0,
            first_command: Some("PRIVMSG".to_string()),
        }
    }

    #[test]
    fn assemble_single_line_yields_just_that_line() {
        let b = batch_with_lines(vec![line("only", false)]);
        assert_eq!(assemble_body(&b), "only");
    }

    #[test]
    fn assemble_joins_normal_lines_with_newline() {
        let b = batch_with_lines(vec![
            line("first", false),
            line("second", false),
            line("third", false),
        ]);
        assert_eq!(assemble_body(&b), "first\nsecond\nthird");
    }

    #[test]
    fn assemble_concat_line_joins_without_separator() {
        // Splitting one long word across two chunks should rejoin
        // seamlessly — this is the spec's "splitting long lines" case.
        let b = batch_with_lines(vec![
            line("hello ", false),
            line("everyone", true), // concat-to-previous
        ]);
        assert_eq!(assemble_body(&b), "hello everyone");
    }

    #[test]
    fn assemble_mixed_concat_and_newline_lines() {
        // The spec's own example:
        //   hello
        //
        //   how is everyone?
        // sent as: "hello", "", "how is ", concat:"everyone?"
        let b = batch_with_lines(vec![
            line("hello", false),
            line("", false),
            line("how is ", false),
            line("everyone?", true),
        ]);
        assert_eq!(assemble_body(&b), "hello\n\nhow is everyone?");
    }

    #[test]
    fn assemble_appends_no_trailing_newline() {
        // Spec: "No line feed is appended to the final line message
        // of a batch." Belt-and-suspenders test.
        let b = batch_with_lines(vec![
            line("first", false),
            line("second", false),
        ]);
        let assembled = assemble_body(&b);
        assert!(
            !assembled.ends_with('\n'),
            "trailing newline leaked: {assembled:?}"
        );
    }

    #[test]
    fn line_budget_enforced_at_append() {
        let state = test_state();
        handle_batch_open(
            &state,
            "sess-A",
            "abc",
            "draft/multiline",
            "#c",
            empty_tags(),
        )
        .unwrap();
        for _ in 0..MAX_LINES {
            let r = try_route_to_batch(
                &state,
                "sess-A",
                &tag("batch", "abc"),
                "PRIVMSG",
                "#c",
                "ok",
                false,
            );
            assert!(matches!(r, RouteOutcome::Absorbed));
        }
        let r_over = try_route_to_batch(
            &state,
            "sess-A",
            &tag("batch", "abc"),
            "PRIVMSG",
            "#c",
            "ok",
            false,
        );
        assert!(matches!(r_over, RouteOutcome::Error(BatchError::MaxLines)));
    }
}
