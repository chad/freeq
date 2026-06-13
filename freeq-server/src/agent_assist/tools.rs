//! Deterministic diagnostic tools.
//!
//! Each tool takes a typed input + a [`Caller`] + the shared state,
//! reads from authoritative server state (DB, in-memory channel maps),
//! applies the disclosure filter for the bundle's required level, and
//! returns a [`FactBundle`].
//!
//! The tools are pure with respect to the request — they do not record
//! into the diagnostic recorder, do not mutate state, and do not write
//! to the network. That property is what lets the LLM PR safely call
//! them as fact-gathering primitives without worrying about side
//! effects.
//!
//! ### Adding a tool
//!
//! 1. Add an input struct in [`super::types`].
//! 2. Add a `pub fn tool_xyz(input, caller, state) -> FactBundle` here.
//! 3. Wire an HTTP handler in [`super::api`].
//! 4. Add a row to the discovery `capabilities` array.
//!
//! The LLM router (next PR) will pick a tool based on a free-form
//! `/agent/session` request, call it, then optionally summarise the
//! returned bundle's `safe_facts`. None of that affects this file.

use super::caller;
use super::types::*;
use crate::server::SharedState;
use std::sync::Arc;

// ─── Tool: validate_client_config ────────────────────────────────────────

/// Pure-function validator. No state read, no DB hit. Public.
///
/// Rules implemented for MVP cover the most common foot-guns we have
/// observed when someone bolts together a fresh client. Add to this
/// list as new client failure modes show up in the wild — every entry
/// should pair a missing/unsupported feature with a concrete fix.
pub fn validate_client_config(input: &ValidateClientConfigInput) -> FactBundle {
    let mut warnings: Vec<String> = Vec::new();
    let mut fixes: Vec<SuggestedFix> = Vec::new();

    let safe_name = sanitize_label(&input.client_name);

    if !input.supports.message_tags {
        warnings.push(
            "Client does not advertise `message-tags`. msgid, time, and reply tags will be \
             missing on every PRIVMSG, breaking edits, deletes, replies, and reactions."
                .into(),
        );
        fixes.push(SuggestedFix {
            summary: "Negotiate the `message-tags` IRCv3 capability before joining channels."
                .into(),
            details: Some(
                "Send `CAP LS 302`, then `CAP REQ :message-tags server-time batch` and wait for \
                 `CAP ACK` before sending USER/NICK or any JOIN."
                    .into(),
            ),
        });
    }

    if !input.supports.server_time {
        warnings.push(
            "Client does not advertise `server-time`. Timestamps on history and live messages \
             will fall back to local receive time, which causes ordering surprises after \
             reconnect."
                .into(),
        );
        fixes.push(SuggestedFix {
            summary: "Request `server-time` and render messages by the server `time` tag.".into(),
            details: None,
        });
    }

    if !input.supports.batch {
        warnings.push(
            "Client does not advertise `batch`. CHATHISTORY replies will arrive as a flat \
             interleaved stream and may be ordered incorrectly relative to live messages."
                .into(),
        );
        fixes.push(SuggestedFix {
            summary: "Request `batch` to receive CHATHISTORY as a delimited group.".into(),
            details: None,
        });
    }

    if !input.supports.echo_message {
        warnings.push(
            "Client does not advertise `echo-message`. Self-sent messages will not be echoed \
             back, which makes optimistic local rendering racy with edits and deletes."
                .into(),
        );
    }

    if input.supports.e2ee && !input.supports.message_tags {
        warnings.push(
            "Client claims E2EE support but not message-tags. E2EE relies on tag-based metadata \
             (msgid, sig, encrypted) — this combination will not work."
                .into(),
        );
    }

    let wants_multi_device = input
        .desired_features
        .iter()
        .any(|f| f.eq_ignore_ascii_case("multi_device") || f.eq_ignore_ascii_case("multi-device"));

    if wants_multi_device && !input.supports.resume {
        warnings.push(
            "Client wants multi_device but does not advertise resume support. After a reconnect, \
             other devices will see a transient quit/join cycle and history may be replayed in \
             a different order than canonical."
                .into(),
        );
        fixes.push(SuggestedFix {
            summary: "Implement session resume so reconnects don't churn presence and replay."
                .into(),
            details: Some(
                "Persist the last server `msgid` you observed per channel and request \
                 `CHATHISTORY AFTER #ch msgid=<...>` on reconnect."
                    .into(),
            ),
        });
    }

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!("Validated configuration for client `{safe_name}`."));
    safe_facts.push(format!(
        "Capability bitmap observed: message-tags={mt}, server-time={st}, batch={ba}, \
         sasl={sa}, resume={re}, e2ee={e2}, echo-message={em}, away-notify={aw}.",
        mt = input.supports.message_tags,
        st = input.supports.server_time,
        ba = input.supports.batch,
        sa = input.supports.sasl,
        re = input.supports.resume,
        e2 = input.supports.e2ee,
        em = input.supports.echo_message,
        aw = input.supports.away_notify,
    ));
    safe_facts.extend(warnings.iter().cloned());

    let ok = warnings.is_empty();
    let (code, summary, confidence) = if ok {
        (
            "CONFIG_OK".to_string(),
            "Client configuration looks compatible with current server expectations.".to_string(),
            Confidence::High,
        )
    } else {
        (
            "CONFIG_HAS_WARNINGS".to_string(),
            format!(
                "Client configuration has {} compatibility warning(s).",
                warnings.len()
            ),
            Confidence::High,
        )
    };

    FactBundle {
        ok,
        code,
        summary,
        confidence,
        safe_facts,
        suggested_fixes: fixes,
        redactions: vec![],
        followups: vec![],
        // Validator only inspects caller-supplied JSON, no server state.
        min_disclosure: DisclosureLevel::Public,
    }
}

// ─── Tool: diagnose_message_ordering ─────────────────────────────────────

/// Compare the caller's observed display order against the canonical
/// server order for the listed msgids in a channel. Reads from the
/// `messages` table directly.
///
/// Disclosure: requires the caller to be a member of the channel, or
/// a server operator. Message bodies are never returned; only the
/// canonical sequence (autoincrement row id) and server timestamp.
pub fn diagnose_message_ordering(
    input: &DiagnoseMessageOrderingInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    let channel = normalize_channel(&input.channel);
    let safe_channel = sanitize_label(&channel);

    let effective = caller::effective_level(caller, state, &channel);
    if !effective.satisfies(DisclosureLevel::ChannelMember) {
        return permission_denied(
            "DIAGNOSE_MESSAGE_ORDERING_REQUIRES_MEMBERSHIP",
            "You must be a member of the channel to inspect its message ordering.",
            DisclosureLevel::ChannelMember,
        );
    }

    if input.message_ids.is_empty() {
        return FactBundle {
            ok: false,
            code: "INVALID_INPUT".into(),
            summary: "Provide at least one msgid to diagnose ordering.".into(),
            confidence: Confidence::High,
            safe_facts: vec![],
            suggested_fixes: vec![SuggestedFix {
                summary: "Pass `message_ids` as a non-empty array of ULID msgids.".into(),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::ChannelMember,
        };
    }
    if input.message_ids.len() > 50 {
        return FactBundle {
            ok: false,
            code: "INVALID_INPUT".into(),
            summary: "Diagnose at most 50 messages per request.".into(),
            confidence: Confidence::High,
            safe_facts: vec![],
            suggested_fixes: vec![],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::ChannelMember,
        };
    }

    // Resolve each msgid to (server_sequence, server_time). Missing
    // ones are reported but not fatal.
    //
    // SECURITY: lookup is **channel-scoped** via `get_message_by_msgid`
    // — NOT `find_message_by_msgid`, which is cross-channel and would
    // let a member of `#public` probe a msgid from `#private` and have
    // its server_sequence + timestamp echoed back here. The CTF-01
    // regression test pins this.
    let lookups: Vec<(String, Option<(i64, u64)>)> = input
        .message_ids
        .iter()
        .map(|id| {
            let row = state
                .with_db(|db| db.get_message_by_msgid(&channel, id))
                .flatten();
            (id.clone(), row.map(|r| (r.id, r.timestamp)))
        })
        .collect();

    let mut missing: Vec<&String> = Vec::new();
    let mut resolved: Vec<(&String, i64, u64)> = Vec::new();
    for (id, found) in &lookups {
        match found {
            None => missing.push(id),
            Some((seq, ts)) => resolved.push((id, *seq, *ts)),
        }
    }

    if resolved.is_empty() {
        return FactBundle {
            ok: false,
            code: "MESSAGES_NOT_FOUND".into(),
            summary: format!(
                "None of the {} provided msgid(s) were found in `{safe_channel}`.",
                input.message_ids.len()
            ),
            confidence: Confidence::High,
            safe_facts: vec![format!(
                "Looked up {} msgid(s); 0 matched persisted messages.",
                input.message_ids.len()
            )],
            suggested_fixes: vec![SuggestedFix {
                summary: "Confirm the msgids and channel name match what the server emitted."
                    .into(),
                details: None,
            }],
            redactions: vec!["Message bodies omitted.".into()],
            followups: vec![],
            min_disclosure: DisclosureLevel::ChannelMember,
        };
    }

    // Canonical order = ascending server_sequence (auto-increment row id).
    let mut canonical = resolved.clone();
    canonical.sort_by_key(|(_, seq, _)| *seq);
    let canonical_ids: Vec<&String> = canonical.iter().map(|(id, _, _)| *id).collect();

    let observed_resolved: Vec<&String> = resolved.iter().map(|(id, _, _)| *id).collect();

    let order_matches = observed_resolved == canonical_ids;

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!("Channel: `{safe_channel}`."));
    for (id, seq, ts) in &canonical {
        safe_facts.push(format!(
            "msgid `{}` — server_sequence={}, server_time={}.",
            sanitize_label(id),
            seq,
            ts
        ));
    }
    if !missing.is_empty() {
        safe_facts.push(format!(
            "{} provided msgid(s) not found in `{safe_channel}`.",
            missing.len()
        ));
    }
    safe_facts.push(format!(
        "Canonical order (oldest → newest): [{}].",
        canonical_ids
            .iter()
            .map(|s| format!("`{}`", sanitize_label(s)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if let Some(symptom) = &input.symptom {
        safe_facts.push(format!(
            "Caller-reported symptom: {}",
            quote_user_text(symptom)
        ));
    }

    let (code, summary, confidence, fixes) = if order_matches {
        (
            "ORDER_MATCHES_CANONICAL".to_string(),
            "The observed display order matches the canonical server order. The reported \
             symptom must have another cause."
                .to_string(),
            Confidence::High,
            vec![SuggestedFix {
                summary: "Look for client-side reordering after this point (debounce, virtualized \
                          scroll, or merge step)."
                    .into(),
                details: None,
            }],
        )
    } else {
        (
            "DISPLAY_ORDER_MISMATCH".to_string(),
            "Client display order does not match canonical server order. The most common \
             cause is rendering by local receive time instead of `server-time` or \
             server-assigned sequence."
                .to_string(),
            Confidence::High,
            vec![
                SuggestedFix {
                    summary: "Sort committed channel messages by server `time` tag (or \
                              server_sequence when available)."
                        .into(),
                    details: Some(
                        "Treat local pending messages as a separate optimistic layer until \
                         their echo arrives with a real msgid + time."
                            .into(),
                    ),
                },
                SuggestedFix {
                    summary: "During CHATHISTORY replay, buffer the batch and apply order on \
                              batch end."
                        .into(),
                    details: None,
                },
            ],
        )
    };

    FactBundle {
        ok: order_matches,
        code,
        summary,
        confidence,
        safe_facts,
        suggested_fixes: fixes,
        redactions: vec![
            "Message bodies omitted.".into(),
            "Sender identities omitted.".into(),
        ],
        followups: vec![Followup {
            tool: "diagnose_sync".into(),
            reason: "Inspect whether these messages were live-delivered or replayed during a \
                     resume."
                .into(),
        }],
        min_disclosure: DisclosureLevel::ChannelMember,
    }
}

// ─── Tool: diagnose_sync ─────────────────────────────────────────────────

/// Best-effort sync diagnosis given today's server state. Honestly
/// reports what the server *can* and *cannot* know.
///
/// We can answer:
/// - "How many concurrent sessions does this DID have right now?"
/// - "Which of those sessions are joined to the channel?"
///
/// We cannot answer (yet, until per-session delivery cursors are
/// added):
/// - "Which messages did each session actually receive?"
/// - "What's the resume cursor for session X?"
///
/// The bundle says so out loud rather than guessing.
pub fn diagnose_sync(
    input: &DiagnoseSyncInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    // Self-scoping: a non-admin caller may only diagnose their own DID.
    let is_admin = matches!(caller.level, DisclosureLevel::ServerOperator);
    if !is_admin && !caller.is_self(&input.account) {
        return permission_denied(
            "DIAGNOSE_SYNC_SELF_ONLY",
            "Only the account owner (or a server operator) may diagnose this account's sync state.",
            DisclosureLevel::Account,
        );
    }

    let safe_account = sanitize_label(&input.account);
    let sessions: Vec<String> = state
        .did_sessions
        .lock()
        .get(&input.account)
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default();

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!(
        "Account `{safe_account}` has {} active session(s).",
        sessions.len()
    ));

    if sessions.is_empty() {
        return FactBundle {
            ok: false,
            code: "ACCOUNT_NOT_CONNECTED".into(),
            summary: "The account has no active server sessions right now. The symptom \
                      cannot be diagnosed against live state — try again while connected, \
                      or pass `diagnose_message_ordering` for historical messages."
                .into(),
            confidence: Confidence::High,
            safe_facts,
            suggested_fixes: vec![],
            redactions: vec!["Other accounts' session state omitted.".into()],
            followups: vec![Followup {
                tool: "diagnose_message_ordering".into(),
                reason: "Works against persisted messages even when the account is offline.".into(),
            }],
            min_disclosure: DisclosureLevel::Account,
        };
    }

    if let Some(channel) = &input.channel {
        let normalized = normalize_channel(channel);
        let safe_channel = sanitize_label(&normalized);
        let joined: usize = state
            .channels
            .lock()
            .get(&normalized)
            .map(|ch| {
                sessions
                    .iter()
                    .filter(|sid| ch.members.contains(*sid))
                    .count()
            })
            .unwrap_or(0);
        safe_facts.push(format!(
            "{joined} of those {} session(s) are joined to `{safe_channel}`.",
            sessions.len()
        ));
        if joined == 0 {
            safe_facts.push(format!(
                "No live session is in `{safe_channel}` — historical messages will only \
                 reach this account on next JOIN.",
            ));
        }
    }

    if let Some(s) = &input.symptom {
        safe_facts.push(format!("Caller-reported symptom: {}", quote_user_text(s)));
    }

    safe_facts.push(
        "The server does not yet record per-session delivery cursors or resume markers, so \
         it cannot say which specific messages were already delivered to a given session. \
         If you need that, use `diagnose_message_ordering` against the msgids you have."
            .into(),
    );

    FactBundle {
        ok: true,
        code: "SYNC_STATE_REPORTED".into(),
        summary: "Reported the account's live session and channel-join state. Per-session \
                  delivery state is not yet recorded by the server."
            .into(),
        confidence: Confidence::Medium,
        safe_facts,
        suggested_fixes: vec![
            SuggestedFix {
                summary: "If reconnect ordering is the symptom, persist the last seen msgid per \
                          channel and request `CHATHISTORY AFTER` on reconnect."
                    .into(),
                details: None,
            },
            SuggestedFix {
                summary: "Render channel timelines by the server `time` tag, not by local receive \
                          time."
                    .into(),
                details: None,
            },
        ],
        redactions: vec![
            "Session ids and IPs omitted.".into(),
            "Other accounts' session state omitted.".into(),
        ],
        followups: vec![Followup {
            tool: "diagnose_message_ordering".into(),
            reason:
                "Compare canonical order against your client-observed order for specific msgids."
                    .into(),
        }],
        min_disclosure: DisclosureLevel::Account,
    }
}

// ─── Tool: inspect_my_session ────────────────────────────────────────────

/// Reports the wire-state the server has for the caller's DID. Designed
/// for bot developers asking "what does the server actually see?" before
/// chasing a phantom bug in their client code.
pub fn inspect_my_session(
    input: &InspectMySessionInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    let is_admin = matches!(caller.level, DisclosureLevel::ServerOperator);
    if !is_admin && !caller.is_self(&input.account) {
        return permission_denied(
            "INSPECT_MY_SESSION_SELF_ONLY",
            "Only the account owner (or a server operator) may inspect this session.",
            DisclosureLevel::Account,
        );
    }

    let safe_account = sanitize_label(&input.account);
    let session_ids: Vec<String> = state
        .did_sessions
        .lock()
        .get(&input.account)
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default();

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!(
        "Account `{safe_account}` has {} active session(s).",
        session_ids.len()
    ));

    if session_ids.is_empty() {
        return FactBundle {
            ok: false,
            code: "ACCOUNT_NOT_CONNECTED".into(),
            summary: "The server has no active session for this account. Connect first, \
                      then re-run inspect_my_session."
                .into(),
            confidence: Confidence::High,
            safe_facts,
            suggested_fixes: vec![SuggestedFix {
                summary: "Open a WebSocket to /irc and complete SASL ATPROTO-CHALLENGE.".into(),
                details: None,
            }],
            redactions: vec!["Other accounts' session state omitted.".into()],
            followups: vec![],
            min_disclosure: DisclosureLevel::Account,
        };
    }

    // For each session report capabilities, joined channels, away state,
    // signing-key registration, and actor class. Aggregated across
    // multi-device sessions so the bot doesn't need to know which sid
    // it's on.
    let caps_per_session = collect_caps(state, &session_ids);
    let nick = session_ids.first().and_then(|sid| {
        state
            .nick_to_session
            .lock()
            .get_nick(sid.as_str())
            .map(|s| s.to_string())
    });
    let handle = session_ids
        .first()
        .and_then(|sid| state.session_handles.lock().get(sid).cloned());
    let actor_class = session_ids
        .first()
        .and_then(|sid| state.session_actor_class.lock().get(sid).copied())
        .unwrap_or_default();
    let away = session_ids
        .iter()
        .find_map(|sid| state.session_away.lock().get(sid).cloned());

    if let Some(n) = &nick {
        safe_facts.push(format!("Current nick: `{}`.", sanitize_label(n)));
    }
    if let Some(h) = &handle {
        safe_facts.push(format!("Resolved handle: `{}`.", sanitize_label(h)));
    }
    safe_facts.push(format!("Declared actor class: `{actor_class}`."));

    if let Some(reason) = &away {
        safe_facts.push(format!("AWAY: \"{}\"", quote_user_text(reason)));
    } else {
        safe_facts.push("AWAY: not set.".into());
    }

    let signing_key_registered = state.did_msg_keys.lock().contains_key(&input.account);
    safe_facts.push(format!(
        "Client signing key registered: {}.",
        if signing_key_registered { "yes" } else { "no" },
    ));
    if !signing_key_registered {
        safe_facts.push(
            "Without a registered signing key, the server signs messages on your behalf \
             (fallback). Bots should send `MSGSIG <base64url-pubkey>` once per session."
                .into(),
        );
    }

    safe_facts.push(format!("Negotiated capabilities: {caps_per_session}"));

    let joined: Vec<String> = {
        let channels = state.channels.lock();
        channels
            .iter()
            .filter(|(_, ch)| session_ids.iter().any(|sid| ch.members.contains(sid)))
            .map(|(name, _)| name.clone())
            .collect()
    };
    if joined.is_empty() {
        safe_facts.push("Joined channels: none.".into());
    } else {
        safe_facts.push(format!(
            "Joined channels ({}): {}.",
            joined.len(),
            joined
                .iter()
                .map(|c| format!("`{}`", sanitize_label(c)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let mut fixes: Vec<SuggestedFix> = Vec::new();
    if !signing_key_registered {
        fixes.push(SuggestedFix {
            summary: "Send `MSGSIG <base64url-pubkey>` after SASL success.".into(),
            details: Some(
                "Without it, your messages carry the server's fallback signature \
                 instead of yours."
                    .into(),
            ),
        });
    }
    if matches!(actor_class, crate::connection::ActorClass::Human)
        && nick.as_deref().is_some_and(|n| !n.is_empty())
    {
        fixes.push(SuggestedFix {
            summary:
                "If this is a bot/agent, declare actor class via `+freeq.at/actor-class=agent`."
                    .into(),
            details: Some(
                "Other clients flag undeclared agents that look like humans. \
                 Set the cap via your IRCv3 message-tags during JOIN."
                    .into(),
            ),
        });
    }

    FactBundle {
        ok: true,
        code: "SESSION_REPORTED".into(),
        summary: "Session state reported. Cross-check against what your client thinks \
                  it has — drift is the most common bug-source for long-running bots."
            .into(),
        confidence: Confidence::High,
        safe_facts,
        suggested_fixes: fixes,
        redactions: vec![
            "Session ids and IPs omitted.".into(),
            "Other accounts' state omitted.".into(),
        ],
        followups: vec![Followup {
            tool: "diagnose_disconnect".into(),
            reason: "Use this if the session count looks wrong or the bot was dropped recently."
                .into(),
        }],
        min_disclosure: DisclosureLevel::Account,
    }
}

/// Sample which IRCv3 capabilities are negotiated by any of the
/// caller's sessions. Aggregating across multi-device sessions matches
/// how a bot developer thinks ("does the server see my caps?").
fn collect_caps(state: &Arc<SharedState>, sids: &[String]) -> String {
    // Per-cap presence: if ANY session has the cap, count it.
    fn any_in(
        set: &parking_lot::Mutex<std::collections::HashSet<String>>,
        sids: &[String],
    ) -> bool {
        let s = set.lock();
        sids.iter().any(|sid| s.contains(sid))
    }
    let mut caps: Vec<&'static str> = Vec::new();
    if any_in(&state.cap_message_tags, sids) {
        caps.push("message-tags");
    }
    if any_in(&state.cap_server_time, sids) {
        caps.push("server-time");
    }
    if any_in(&state.cap_batch, sids) {
        caps.push("batch");
    }
    if any_in(&state.cap_echo_message, sids) {
        caps.push("echo-message");
    }
    if any_in(&state.cap_account_notify, sids) {
        caps.push("account-notify");
    }
    if any_in(&state.cap_extended_join, sids) {
        caps.push("extended-join");
    }
    if any_in(&state.cap_away_notify, sids) {
        caps.push("away-notify");
    }
    if any_in(&state.cap_account_tag, sids) {
        caps.push("account-tag");
    }
    if any_in(&state.cap_multi_prefix, sids) {
        caps.push("multi-prefix");
    }
    if caps.is_empty() {
        "(none — your client did not negotiate any IRCv3 capabilities)".into()
    } else {
        caps.join(", ")
    }
}

// ─── Tool: diagnose_join_failure ─────────────────────────────────────────

/// Given a channel + DID + (optional) numeric, explain why the JOIN
/// failed and what proof / scope / invite is needed.
pub fn diagnose_join_failure(
    input: &DiagnoseJoinFailureInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    let is_admin = matches!(caller.level, DisclosureLevel::ServerOperator);
    if !is_admin && !caller.is_self(&input.account) {
        return permission_denied(
            "DIAGNOSE_JOIN_FAILURE_SELF_ONLY",
            "Only the account owner (or a server operator) may diagnose their join failures.",
            DisclosureLevel::Account,
        );
    }

    let channel = normalize_channel(&input.channel);
    let safe_channel = sanitize_label(&channel);
    let safe_account = sanitize_label(&input.account);

    let channels = state.channels.lock();
    let Some(ch) = channels.get(&channel) else {
        return FactBundle {
            ok: false,
            code: "CHANNEL_DOES_NOT_EXIST".into(),
            summary: format!(
                "Channel `{safe_channel}` does not exist on this server. JOINing it \
                 will create it (you'll be the founder)."
            ),
            confidence: Confidence::High,
            safe_facts: vec![
                format!("Channel `{safe_channel}` is not currently tracked by the server."),
                "Sending `JOIN #channel` will create the channel and set you as founder \
                 with op privileges."
                    .into(),
            ],
            suggested_fixes: vec![SuggestedFix {
                summary: format!("Send `JOIN {safe_channel}` to create the channel."),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::Account,
        };
    };

    // Collect concrete causes of denial — we may have several active.
    let mut causes: Vec<String> = Vec::new();
    let mut fixes: Vec<SuggestedFix> = Vec::new();

    // +b ban check (DID or hostmask)
    let banned = ch.bans.iter().any(|b| b.matches("", Some(&input.account)));
    if banned {
        causes.push(format!("Your DID is on the ban list for `{safe_channel}`."));
        fixes.push(SuggestedFix {
            summary: "Ask a channel operator to lift the ban (MODE -b).".into(),
            details: None,
        });
    }

    // +k key required
    if ch.key.is_some() {
        causes.push(format!("Channel `{safe_channel}` requires a key (+k)."));
        fixes.push(SuggestedFix {
            summary: format!("Send `JOIN {safe_channel} <passphrase>` (key on the same line)."),
            details: Some(
                "The key is shared out-of-band by the channel ops; the server does not \
                 disclose it via this diagnostic."
                    .into(),
            ),
        });
    }

    // +i invite-only — check invite list (DID or hostmask form)
    if ch.invite_only {
        let invited = ch.invites.iter().any(|inv| inv == &input.account);
        if !invited {
            causes.push(format!(
                "Channel `{safe_channel}` is invite-only (+i) and your DID is not on \
                 the invite list."
            ));
            fixes.push(SuggestedFix {
                summary: format!(
                    "Ask a channel operator to send `INVITE {safe_account} {safe_channel}`."
                ),
                details: None,
            });
        }
    }

    // Policy gate (if policy engine is enabled) — surface only the
    // existence + minimum proof type, not the full policy expression.
    if let Some(_engine) = state.policy_engine.as_ref() {
        // The actual policy engine API would need a per-channel lookup
        // to enumerate required proofs. For MVP we surface the fact
        // that policy is in play and point at the v1 endpoint.
        causes.push(format!(
            "Channel `{safe_channel}` may have a join policy. Fetch \
             /api/v1/policy/{safe_channel} for the full requirement set."
        ));
        fixes.push(SuggestedFix {
            summary: format!("GET /api/v1/policy/{safe_channel} to see what proofs are required."),
            details: None,
        });
    }

    // Translate observed numeric, if provided.
    if let Some(numeric) = &input.observed_numeric {
        let translated = translate_join_numeric(numeric);
        if let Some(t) = translated {
            causes.push(format!("Observed IRC numeric {numeric}: {t}"));
        }
    }

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!(
        "Channel `{safe_channel}` exists with {} local member(s).",
        ch.members.len()
    ));
    let mut mode_chars: Vec<&str> = Vec::new();
    if ch.invite_only {
        mode_chars.push("+i");
    }
    if ch.key.is_some() {
        mode_chars.push("+k");
    }
    if ch.no_ext_msg {
        mode_chars.push("+n");
    }
    if ch.moderated {
        mode_chars.push("+m");
    }
    if ch.encrypted_only {
        mode_chars.push("+E");
    }
    if ch.topic_locked {
        mode_chars.push("+t");
    }
    if !mode_chars.is_empty() {
        safe_facts.push(format!("Channel modes: {}.", mode_chars.join(" ")));
    }
    if !causes.is_empty() {
        safe_facts.extend(causes.iter().cloned());
    }

    let (code, summary, confidence) = if causes.is_empty() {
        (
            "JOIN_SHOULD_SUCCEED".to_string(),
            format!(
                "No obvious blocker for `{safe_account}` to JOIN `{safe_channel}`. \
                 If it's still failing, the cause may be transient (network, broker \
                 rate limit) — retry."
            ),
            Confidence::Medium,
        )
    } else {
        (
            "JOIN_DENIED".to_string(),
            format!(
                "{} reason(s) prevent `{safe_account}` from joining `{safe_channel}`.",
                causes.len()
            ),
            Confidence::High,
        )
    };

    FactBundle {
        ok: causes.is_empty(),
        code,
        summary,
        confidence,
        safe_facts,
        suggested_fixes: fixes,
        redactions: vec![
            "Other members' identities omitted.".into(),
            "Full policy expression omitted (channel operators see it via /api/v1/policy).".into(),
            "Channel key (+k) value omitted — passphrase is shared out-of-band.".into(),
        ],
        followups: vec![],
        min_disclosure: DisclosureLevel::Account,
    }
}

fn translate_join_numeric(n: &str) -> Option<&'static str> {
    Some(match n {
        "473" => "ERR_INVITEONLYCHAN — channel is +i and you weren't invited.",
        "474" => "ERR_BANNEDFROMCHAN — your DID or hostmask is on the ban list.",
        "475" => "ERR_BADCHANNELKEY — channel is +k and the key was missing or wrong.",
        "477" => "ERR_NOCHANMODES (freeq usage) — channel requires policy proof acceptance.",
        "404" => "ERR_CANNOTSENDTOCHAN — usually +n or +m, but on JOIN may indicate flood block.",
        "482" => "ERR_CHANOPRIVSNEEDED — only relevant for ops actions, not for plain JOIN.",
        _ => return None,
    })
}

// ─── Tool: diagnose_disconnect ───────────────────────────────────────────

/// Best-effort cause inference for a recent disconnect. Reads the few
/// signals the server can offer today:
/// - `ghost_sessions` (was the bot's session held in grace?)
/// - server boot time (did the server restart since the bot was up?)
/// - active sessions for the DID right now (multi-device displacement?)
pub fn diagnose_disconnect(
    input: &DiagnoseDisconnectInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    let is_admin = matches!(caller.level, DisclosureLevel::ServerOperator);
    if !is_admin && !caller.is_self(&input.account) {
        return permission_denied(
            "DIAGNOSE_DISCONNECT_SELF_ONLY",
            "Only the account owner (or a server operator) may diagnose their disconnects.",
            DisclosureLevel::Account,
        );
    }

    let safe_account = sanitize_label(&input.account);
    let mut safe_facts: Vec<String> = Vec::new();

    let active_sessions = state
        .did_sessions
        .lock()
        .get(&input.account)
        .map(|s| s.len())
        .unwrap_or(0);
    safe_facts.push(format!(
        "Account `{safe_account}` has {active_sessions} active session(s) right now."
    ));

    let ghost = state.ghost_sessions.lock().get(&input.account).map(|g| {
        let elapsed = g.disconnect_time.elapsed().as_secs();
        (g.nick.clone(), elapsed, g.channels.len())
    });
    if let Some((nick, elapsed_secs, ch_count)) = ghost {
        safe_facts.push(format!(
            "Ghost session present: nick `{}`, disconnected {elapsed_secs}s ago, was in {ch_count} channel(s). \
             If you reconnect within the grace window, channel state is preserved without JOIN/PART churn.",
            sanitize_label(&nick)
        ));
    } else {
        safe_facts.push(
            "No ghost session held for this DID. Either the grace period already \
             expired, or the bot QUIT cleanly (no grace is held for clean QUITs)."
                .into(),
        );
    }

    let boot_age_secs = state.boot_time.elapsed().as_secs();
    safe_facts.push(format!(
        "Server has been up for {boot_age_secs}s. If this is shorter than your last-seen \
         duration, the server restarted — your bot needs to re-authenticate and rejoin."
    ));

    // The recorder captures session events; surface a concise count.
    let recent = crate::agent_assist::recorder::RECORDER.query(|e| {
        e.did.as_deref() == Some(&input.account)
            && matches!(
                e.kind,
                crate::agent_assist::recorder::EventKind::SessionOpened
                    | crate::agent_assist::recorder::EventKind::SessionClosed
            )
    });
    if !recent.is_empty() {
        safe_facts.push(format!(
            "Diagnostic ring buffer holds {} recent session event(s) for this DID.",
            recent.len()
        ));
    }

    let fixes = vec![
        SuggestedFix {
            summary: "Implement reconnect-with-backoff and re-do SASL on every reconnect.".into(),
            details: Some(
                "If your client got a Guest nick after reconnecting, your SASL credentials \
                 expired — refresh via the broker before sending the next IRC command."
                    .into(),
            ),
        },
        SuggestedFix {
            summary: "Persist last-seen msgid per channel so you can call \
                      `CHATHISTORY AFTER` and not double-render history."
                .into(),
            details: None,
        },
    ];

    FactBundle {
        ok: true,
        code: "DISCONNECT_REPORTED".into(),
        summary: "Reported what the server can see about your recent disconnect. \
                  The server does not record per-session reason codes yet, so this \
                  is best-effort inference."
            .into(),
        confidence: Confidence::Medium,
        safe_facts,
        suggested_fixes: fixes,
        redactions: vec!["Other accounts' disconnect history omitted.".into()],
        followups: vec![Followup {
            tool: "inspect_my_session".into(),
            reason: "Confirm which sessions are currently live and what state they have.".into(),
        }],
        min_disclosure: DisclosureLevel::Account,
    }
}

// ─── Tool: replay_missed_messages ────────────────────────────────────────

/// Counts the messages between `since_msgid` and "now" in a channel,
/// reports the sequence + msgids of the gap. Does NOT return bodies —
/// the bot fetches those via `CHATHISTORY AFTER` once it knows the gap
/// exists.
pub fn replay_missed_messages(
    input: &ReplayMissedMessagesInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    let channel = normalize_channel(&input.channel);
    let safe_channel = sanitize_label(&channel);
    let effective = caller::effective_level(caller, state, &channel);
    if !effective.satisfies(DisclosureLevel::ChannelMember) {
        return permission_denied(
            "REPLAY_MISSED_MESSAGES_REQUIRES_MEMBERSHIP",
            "You must be a member of the channel to ask what was missed.",
            DisclosureLevel::ChannelMember,
        );
    }

    let limit = input.limit.unwrap_or(1000).min(2000);
    // Anchor: look up the since_msgid to get its timestamp.
    //
    // SECURITY: channel-scoped lookup. The cross-channel
    // `find_message_by_msgid` would leak the server_sequence and
    // timestamp of a `#private` msgid to any caller who is a member of
    // some other channel. CTF-01 regression test pins this.
    let anchor = state
        .with_db(|db| db.get_message_by_msgid(&channel, &input.since_msgid))
        .flatten();
    let Some(anchor) = anchor else {
        return FactBundle {
            ok: false,
            code: "ANCHOR_MSGID_NOT_FOUND".into(),
            summary: format!(
                "Could not locate `{}` in any channel. Either the msgid is wrong or \
                 the message has been pruned. Try fetching CHATHISTORY LATEST instead.",
                sanitize_label(&input.since_msgid)
            ),
            confidence: Confidence::High,
            safe_facts: vec![],
            suggested_fixes: vec![SuggestedFix {
                summary: format!("Send `CHATHISTORY LATEST {safe_channel} * 50` to refill."),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::ChannelMember,
        };
    };

    // Now query the gap.
    let rows = state
        .with_db(|db| db.get_messages_after(&channel, anchor.timestamp, limit))
        .unwrap_or_default();

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!(
        "Anchor `{}` is at server_sequence {}, time {}.",
        sanitize_label(&input.since_msgid),
        anchor.id,
        anchor.timestamp
    ));
    safe_facts.push(format!(
        "Between then and now, channel `{safe_channel}` has {} new message(s) \
         (cap: {limit}).",
        rows.len()
    ));
    if let Some(first) = rows.first() {
        safe_facts.push(format!(
            "First missed: msgid=`{}`, seq={}, time={}.",
            first
                .msgid
                .as_deref()
                .map(sanitize_label)
                .unwrap_or_else(|| "(no msgid)".into()),
            first.id,
            first.timestamp,
        ));
    }
    if let Some(last) = rows.last() {
        safe_facts.push(format!(
            "Last missed: msgid=`{}`, seq={}, time={}.",
            last.msgid
                .as_deref()
                .map(sanitize_label)
                .unwrap_or_else(|| "(no msgid)".into()),
            last.id,
            last.timestamp,
        ));
    }

    let fixes = vec![SuggestedFix {
        summary: format!(
            "Send `CHATHISTORY AFTER {safe_channel} msgid={} {}` to fetch the bodies.",
            sanitize_label(&input.since_msgid),
            rows.len().min(50),
        ),
        details: Some(
            "Use the `batch` IRCv3 capability to receive the replay as a delimited \
             group; otherwise live messages may interleave."
                .into(),
        ),
    }];

    FactBundle {
        ok: true,
        code: "GAP_REPORTED".into(),
        summary: format!(
            "Reported the canonical gap between your anchor and now in `{safe_channel}`. \
             Use CHATHISTORY AFTER to fetch the actual content."
        ),
        confidence: Confidence::High,
        safe_facts,
        suggested_fixes: fixes,
        redactions: vec![
            "Message bodies omitted — fetch via CHATHISTORY AFTER.".into(),
            "Sender identities omitted from this overview.".into(),
        ],
        followups: vec![],
        min_disclosure: DisclosureLevel::ChannelMember,
    }
}

// ─── Tool: predict_message_outcome ───────────────────────────────────────

/// Dry-run a PRIVMSG. Reports whether it would be accepted, rate-
/// limited, blocked by channel mode, etc. No actual send.
pub fn predict_message_outcome(
    input: &PredictMessageOutcomeInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    let is_admin = matches!(caller.level, DisclosureLevel::ServerOperator);
    if !is_admin && !caller.is_self(&input.account) {
        return permission_denied(
            "PREDICT_MESSAGE_OUTCOME_SELF_ONLY",
            "Only the account owner (or a server operator) may predict their own send outcome.",
            DisclosureLevel::Account,
        );
    }

    let safe_account = sanitize_label(&input.account);
    let target = input.target.trim();
    let safe_target = sanitize_label(target);
    let is_channel = target.starts_with('#') || target.starts_with('&');

    let session_ids: Vec<String> = state
        .did_sessions
        .lock()
        .get(&input.account)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    if session_ids.is_empty() {
        return FactBundle {
            ok: false,
            code: "PREDICT_NOT_CONNECTED".into(),
            summary: format!(
                "Account `{safe_account}` has no live session — any send would fail \
                 because there is no socket to send through."
            ),
            confidence: Confidence::High,
            safe_facts: vec![],
            suggested_fixes: vec![SuggestedFix {
                summary: "Connect (open WebSocket + SASL) before predicting send outcome.".into(),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::Account,
        };
    }

    // Per-session flood predict: 5 msgs per 2s. Aggregate the most
    // permissive session so a multi-device bot picks the right slot.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let (best_sid, recent_sends, room_left) = {
        let map = state.msg_timestamps.lock();
        let mut best: (String, usize, i32) = (String::new(), usize::MAX, -1);
        for sid in &session_ids {
            let recent = map
                .get(sid)
                .map(|v| {
                    v.iter()
                        .filter(|t| now_ms.saturating_sub(**t) < 2000)
                        .count()
                })
                .unwrap_or(0);
            let room = 5_i32 - recent as i32;
            if room > best.2 {
                best = (sid.clone(), recent, room);
            }
        }
        best
    };
    let _ = best_sid; // session id kept private; only the count is reported.

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!(
        "Sender `{safe_account}` has {} live session(s); best one has {recent_sends} send(s) \
         in the last 2s window (limit 5 / 2s, so {room_left} send(s) of headroom).",
        session_ids.len()
    ));

    let mut blockers: Vec<String> = Vec::new();
    if room_left <= 0 {
        blockers.push(
            "RATE_LIMITED: every session is at the per-2s flood cap. Wait at \
                       least one second and retry."
                .into(),
        );
    }

    if is_channel {
        let channels = state.channels.lock();
        let normalized = target.to_lowercase();
        let Some(ch) = channels.get(&normalized) else {
            return FactBundle {
                ok: false,
                code: "PREDICT_CHANNEL_DOES_NOT_EXIST".into(),
                summary: format!(
                    "Channel `{safe_target}` does not exist on this server. Sending will \
                     create it (you'd be founder)."
                ),
                confidence: Confidence::High,
                safe_facts,
                suggested_fixes: vec![SuggestedFix {
                    summary: format!("If intentional, send `JOIN {safe_target}` first."),
                    details: None,
                }],
                redactions: vec![],
                followups: vec![],
                min_disclosure: DisclosureLevel::Account,
            };
        };
        let in_channel = session_ids.iter().any(|sid| ch.members.contains(sid));
        if !in_channel && ch.no_ext_msg {
            blockers.push(format!(
                "NOT_IN_CHANNEL_AND_NO_EXTERNAL: `{safe_target}` is +n; you must JOIN first."
            ));
        }
        let voiced_or_op = session_ids
            .iter()
            .any(|sid| ch.voiced.contains(sid) || ch.ops.contains(sid) || ch.halfops.contains(sid));
        if ch.moderated && !voiced_or_op {
            blockers.push(format!(
                "MODERATED: `{safe_target}` is +m; only voiced / op / halfop may send."
            ));
        }
        if ch.encrypted_only {
            safe_facts.push(format!(
                "Note: `{safe_target}` is +E. Your client must include the `+encrypted` tag \
                 and an ENC1-prefixed payload — plain PRIVMSG will be rejected as 404."
            ));
        }
    }

    let (code, summary, confidence) = if blockers.is_empty() {
        (
            "PREDICTED_ACCEPTED".to_string(),
            format!("A PRIVMSG to `{safe_target}` from `{safe_account}` should be accepted."),
            Confidence::High,
        )
    } else {
        safe_facts.extend(blockers.iter().cloned());
        (
            "PREDICTED_REJECTED".to_string(),
            format!(
                "A PRIVMSG to `{safe_target}` would be rejected for {} reason(s).",
                blockers.len()
            ),
            Confidence::High,
        )
    };

    let _ = input.draft_size_bytes; // reserved

    FactBundle {
        ok: blockers.is_empty(),
        code,
        summary,
        confidence,
        safe_facts,
        suggested_fixes: vec![],
        redactions: vec!["Other senders' rate state omitted.".into()],
        followups: vec![Followup {
            tool: "diagnose_join_failure".into(),
            reason: "Use this if the prediction was NOT_IN_CHANNEL and you can't JOIN.".into(),
        }],
        min_disclosure: DisclosureLevel::Account,
    }
}

// ─── Tool: explain_message_routing ───────────────────────────────────────

/// Pure parser: takes a wire IRC line + the bot's own nick and explains
/// where the message goes, who sent it, and whether it triggers any of
/// the routing footguns (self-echo, mention false-positive, encrypted,
/// edit, delete, action). No state read.
pub fn explain_message_routing(input: &ExplainMessageRoutingInput) -> FactBundle {
    // Refuse oversized lines outright. RFC 1459 §2.3 caps IRC lines
    // at 512 bytes; IRCv3 message-tags raise that to ~8KiB in practice.
    // The HTTP body limit is 12 MB, which would otherwise let a caller
    // hand the SDK parser megabytes of pathological input. CTF-04 pins
    // this — the bundle is the same shape the parse-failure branch
    // returns so callers don't need a special-case.
    const MAX_WIRE_BYTES: usize = 8 * 1024;
    if input.wire_line.len() > MAX_WIRE_BYTES {
        return FactBundle {
            ok: false,
            code: "WIRE_LINE_TOO_LARGE".into(),
            summary: format!(
                "wire_line is {} bytes; the maximum accepted is {MAX_WIRE_BYTES} bytes \
                 (well above the 512-byte IRC line limit). Refusing to parse.",
                input.wire_line.len()
            ),
            confidence: Confidence::High,
            safe_facts: vec![],
            suggested_fixes: vec![SuggestedFix {
                summary: "Send only one IRC line at a time, with no line above ~8 KiB.".into(),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::Public,
        };
    }
    let line = input.wire_line.trim_end_matches(['\r', '\n']);
    let Some(msg) = freeq_sdk::irc::Message::parse(line) else {
        return FactBundle {
            ok: false,
            code: "WIRE_LINE_PARSE_FAILED".into(),
            summary: "Could not parse the supplied IRC line.".into(),
            confidence: Confidence::High,
            safe_facts: vec![format!(
                "Input was: {}",
                quote_user_text(&input.wire_line)
            )],
            suggested_fixes: vec![SuggestedFix {
                summary: "Verify the line is a single CRLF-terminated IRC message with optional `@tags` and `:prefix`.".into(),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::Public,
        };
    };

    let cmd = msg.command.to_uppercase();
    let prefix_nick = msg
        .prefix
        .as_deref()
        .and_then(|p| p.split('!').next())
        .unwrap_or("")
        .to_string();
    let from_safe = sanitize_label(&prefix_nick);
    let my_nick_safe = sanitize_label(&input.my_nick);
    let target = msg.params.first().map(String::as_str).unwrap_or("");
    let safe_target = sanitize_label(target);
    let text = msg.params.get(1).map(String::as_str).unwrap_or("");

    let is_channel_target = target.starts_with('#') || target.starts_with('&');
    let is_self = !prefix_nick.is_empty()
        && !input.my_nick.is_empty()
        && prefix_nick.eq_ignore_ascii_case(&input.my_nick);

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!("Command: `{cmd}`."));
    if !prefix_nick.is_empty() {
        safe_facts.push(format!(
            "Sender: `{from_safe}`{}.",
            if is_self { " (you, self-echo)" } else { "" }
        ));
    }
    if !target.is_empty() {
        safe_facts.push(format!(
            "Target: `{safe_target}` ({}).",
            if is_channel_target {
                "channel"
            } else {
                "user / DM"
            }
        ));
    }

    // Buffer name the bot should route this into.
    let buffer = if is_channel_target || is_self {
        target.to_string()
    } else {
        prefix_nick.clone()
    };
    safe_facts.push(format!(
        "Buffer to route into: `{}` (bot logic should display the message there).",
        sanitize_label(&buffer)
    ));

    // Tag-driven routing flags
    let has_tag = |k: &str| msg.tags.contains_key(k);
    if has_tag("+draft/edit") {
        safe_facts.push(
            "Edit: this message replaces a previous one (msgid in `+draft/edit`). \
             Update your stored copy in place; do not append."
                .into(),
        );
    }
    if has_tag("+draft/delete") {
        safe_facts.push(
            "Delete: this is a TAGMSG removing an existing message (msgid in `+draft/delete`). \
             Hide it from your log."
                .into(),
        );
    }
    if has_tag("+freeq.at/streaming") {
        safe_facts.push(
            "Streaming: progressive update — multiple chunks share one msgid; render only \
             the last edit when the streaming flag clears."
                .into(),
        );
    }
    if has_tag("+encrypted") || text.starts_with("ENC1:") || text.starts_with("ENC3:") {
        safe_facts.push(
            "Encrypted: payload is opaque ciphertext (ENC1 = channel, ENC3 = DM). \
             Decrypt with the relevant key before logging or quoting."
                .into(),
        );
    }
    let is_action =
        text.starts_with('\u{0001}') && text.ends_with('\u{0001}') && text.contains("ACTION");
    if is_action {
        safe_facts.push(
            "Action: a `/me` message — render as third-person (`* nick text`), \
             not as a quoted line."
                .into(),
        );
    }

    // Mention detection — boundary-aware so URLs like
    // `https://example.com/admin/panel` don't trigger for nick `admin`.
    let mention = !is_self
        && !input.my_nick.is_empty()
        && contains_word_boundary(&text.to_lowercase(), &input.my_nick.to_lowercase());
    if mention {
        safe_facts.push(format!(
            "Mention: text contains `{my_nick_safe}` at a word boundary. \
             Bots may want to respond — but check `is_self` first."
        ));
    } else if !is_self
        && !input.my_nick.is_empty()
        && text.to_lowercase().contains(&input.my_nick.to_lowercase())
    {
        safe_facts.push(format!(
            "FALSE-POSITIVE GUARD: `{my_nick_safe}` appears in the text but NOT at a \
             word boundary (likely inside a URL, code block, or another word). Do NOT \
             treat as a mention — that's a common bot-loop trigger."
        ));
    }

    let mut warnings: Vec<String> = Vec::new();
    if is_self {
        warnings.push(
            "Self-echo: this is your own message reflected back via echo-message. \
             Skip it in your reply logic, otherwise you'll loop."
                .into(),
        );
    }

    safe_facts.extend(warnings);

    FactBundle {
        ok: true,
        code: "ROUTING_EXPLAINED".into(),
        summary: format!(
            "Parsed `{cmd}` with target `{safe_target}` — see facts for routing implications."
        ),
        confidence: Confidence::High,
        safe_facts,
        suggested_fixes: vec![],
        redactions: vec![],
        followups: vec![],
        min_disclosure: DisclosureLevel::Public,
    }
}

/// True if `needle` appears in `hay` at a *strict mention boundary*.
///
/// "Word boundary" in the IRC mention sense is narrower than the
/// generic regex `\b`: we only count occurrences flanked by whitespace
/// or sentence punctuation, NOT by slashes, hyphens, underscores, dots,
/// or other characters that commonly appear in URLs (`example.com/admin/`),
/// identifiers (`my_admin`), or code (`#admin-tag`). The classic
/// false-positive these rules kill: a bot named `admin` lighting up
/// every time someone posts a URL containing `/admin/`.
fn contains_word_boundary(hay: &str, needle: &str) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    let bytes = hay.as_bytes();
    let n_bytes = needle.as_bytes();
    fn is_mention_boundary(b: u8) -> bool {
        matches!(
            b,
            b' ' | b'\t'
                | b'\n'
                | b'\r'
                | b','
                | b'.'
                | b'?'
                | b'!'
                | b':'
                | b';'
                | b'('
                | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'"'
                | b'\''
        )
    }
    let mut i = 0;
    while i + n_bytes.len() <= bytes.len() {
        if &bytes[i..i + n_bytes.len()] == n_bytes {
            let before_ok = i == 0 || is_mention_boundary(bytes[i - 1]);
            let after = i + n_bytes.len();
            let after_ok = after == bytes.len() || is_mention_boundary(bytes[after]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

// ─── Helpers ─────────────────────────────────────────────────────────────

/// "Why isn't the call working / why does one device see different
/// participants than the other?" Returns a privacy-preserving snapshot
/// of the channel's active AV session and flags divergences between
/// the three places state lives:
///
/// 1. `AvSessionManager` — the participant slot map served via
///    `/api/v1/sessions/{id}` to clients.
/// 2. `av_instances_per_conn` — which IRC connection registered which
///    `av-instance` ids. Used by the disconnect handler to clean only
///    the dying connection's slots.
/// 3. `av_bridges` — whether the MoQ↔Room bridge is up for the
///    session.
///
/// If (1) and (2) disagree, that's the "phantom participant / ghost
/// publisher" class of bug; if (1) and (3) disagree, the session
/// metadata is stale relative to live media. Both are reported as
/// distinct findings so a bot or an operator can act on them.
///
/// Privacy: DIDs and instance ids are hashed (8-char b64), EXCEPT the
/// caller's own DID — its instances are quoted in cleartext so the
/// caller can self-correlate with what their client thinks it has.
pub fn diagnose_av_session(
    input: &DiagnoseAvSessionInput,
    caller: &Caller,
    state: &Arc<SharedState>,
) -> FactBundle {
    use base64::Engine as _;
    use sha2::Digest;

    let channel = if input.channel.starts_with('#') || input.channel.starts_with('&') {
        input.channel.clone()
    } else {
        format!("#{}", input.channel)
    };
    let safe_channel = sanitize_label(&channel);
    let level = crate::agent_assist::caller::effective_level(caller, state, &channel);
    // Counts + divergence flag are visible to anyone — they leak no
    // identities. Per-slot listings (with hashed DIDs/instances) are
    // ChannelMember-gated so the response isn't a probe surface for
    // who's currently in a private call.
    let detailed = level.satisfies(DisclosureLevel::ChannelMember);

    // 8-char URL-safe hash; enough to disambiguate within a session
    // without leaking identity. We mix in the channel so two sessions
    // can't be cross-correlated.
    let hash = |s: &str| -> String {
        let mut hasher = sha2::Sha256::new();
        hasher.update(safe_channel.as_bytes());
        hasher.update(b":");
        hasher.update(s.as_bytes());
        let digest = hasher.finalize();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..6])
    };

    let mgr = state.av_sessions.lock();
    let Some(session) = mgr.active_session_for_channel(&channel) else {
        return FactBundle {
            ok: true,
            code: "NO_ACTIVE_AV_SESSION".into(),
            summary: format!("No active AV session in `{safe_channel}`."),
            confidence: Confidence::High,
            safe_facts: vec![],
            suggested_fixes: vec![SuggestedFix {
                summary: "Have someone send +freeq.at/av-start TAGMSG to the channel.".into(),
                details: None,
            }],
            redactions: vec![],
            followups: vec![],
            min_disclosure: DisclosureLevel::ChannelMember,
        };
    };
    let session = session.clone();
    drop(mgr);
    let session_id = session.id.clone();
    let safe_session = hash(&session_id);

    // Mgr-side: active slots in the participants map.
    let mgr_slots: Vec<(String, String, bool)> = session
        .participants
        .values()
        .filter(|p| p.left_at.is_none())
        .map(|p| {
            let inst_label = match p.instance_id.as_deref() {
                Some(i) if caller.is_self(&p.did) => format!("`{}`", sanitize_label(i)),
                Some(i) => format!("`#{}`", hash(i)),
                None => "`<no-instance>`".into(),
            };
            (hash(&p.did), inst_label, caller.is_self(&p.did))
        })
        .collect();

    // av_instances_per_conn: the set of (did, instance) currently
    // tracked against a live IRC connection.
    let conn_slots: Vec<(String, String, bool)> = {
        let per_conn = state.av_instances_per_conn.lock();
        let dids = state.session_dids.lock();
        per_conn
            .iter()
            .flat_map(|(sid, set)| {
                let did = dids.get(sid).cloned().unwrap_or_default();
                set.iter().map(move |inst| (did.clone(), inst.clone()))
            })
            .map(|(did, inst)| {
                let inst_label = if caller.is_self(&did) {
                    format!("`{}`", sanitize_label(&inst))
                } else {
                    format!("`#{}`", hash(&inst))
                };
                (hash(&did), inst_label, caller.is_self(&did))
            })
            .collect()
    };

    #[cfg(feature = "av-native")]
    let bridge_up = state.av_bridges.lock().contains_key(&session_id);
    #[cfg(not(feature = "av-native"))]
    let bridge_up = false;

    let mut safe_facts: Vec<String> = Vec::new();
    safe_facts.push(format!("Session `#{safe_session}` in `{safe_channel}`."));
    safe_facts.push(format!(
        "AvSessionManager active slots: {}.",
        mgr_slots.len()
    ));
    safe_facts.push(format!(
        "av_instances_per_conn tracked slots: {}.",
        conn_slots.len()
    ));
    safe_facts.push(format!(
        "MoQ↔Room bridge: {}.",
        if bridge_up { "up" } else { "down" }
    ));

    if detailed {
        for (i, (did, inst, is_self)) in mgr_slots.iter().enumerate() {
            safe_facts.push(format!(
                "mgr[{}] did=`#{}` instance={}{}",
                i,
                did,
                inst,
                if *is_self { " (you)" } else { "" }
            ));
        }
        for (i, (did, inst, is_self)) in conn_slots.iter().enumerate() {
            safe_facts.push(format!(
                "conn[{}] did=`#{}` instance={}{}",
                i,
                did,
                inst,
                if *is_self { " (you)" } else { "" }
            ));
        }
    }

    // ── Divergence detection ─────────────────────────────────────
    let mut fixes: Vec<SuggestedFix> = Vec::new();
    let mut code = "AV_SESSION_HEALTHY";
    let mut ok = true;

    let conn_set: std::collections::HashSet<(String, String)> = conn_slots
        .iter()
        .map(|(d, i, _)| (d.clone(), i.clone()))
        .collect();
    let mgr_set: std::collections::HashSet<(String, String)> = mgr_slots
        .iter()
        .map(|(d, i, _)| (d.clone(), i.clone()))
        .collect();

    let mgr_without_conn: Vec<_> = mgr_set.difference(&conn_set).collect();
    let conn_without_mgr: Vec<_> = conn_set.difference(&mgr_set).collect();

    if !mgr_without_conn.is_empty() {
        ok = false;
        code = "PHANTOM_MGR_SLOT";
        safe_facts.push(format!(
            "DIVERGENCE: {} participant slot(s) appear in AvSessionManager but have no \
             live IRC connection registered. Their MoQ broadcasts are likely silent.",
            mgr_without_conn.len()
        ));
        fixes.push(SuggestedFix {
            summary: "Reap orphan slots on the next av-join — already wired, may need a \
                      kick: have someone fresh av-join the channel."
                .into(),
            details: Some(
                "The orphan reaper runs at av-join time using av_instances_per_conn as \
                 ground truth."
                    .into(),
            ),
        });
    }
    if !conn_without_mgr.is_empty() {
        ok = false;
        // Don't overwrite the more serious code if both apply.
        if code == "AV_SESSION_HEALTHY" {
            code = "MISSING_MGR_SLOT";
        }
        safe_facts.push(format!(
            "DIVERGENCE: {} (DID, instance) pair(s) are tracked against a live IRC \
             connection but are NOT in the participants map. The av-join either failed \
             silently or its slot was reaped after insertion.",
            conn_without_mgr.len()
        ));
        fixes.push(SuggestedFix {
            summary: "Have the affected client re-send av-join (server may have rejected \
                      the original)."
                .into(),
            details: None,
        });
    }
    if mgr_slots.is_empty() && conn_slots.is_empty() && !bridge_up {
        // Pretty likely a wedged session — present but empty.
        ok = false;
        code = "EMPTY_SESSION";
    }

    let summary = match code {
        "AV_SESSION_HEALTHY" => format!(
            "Session `#{safe_session}` has {} consistent participant slot(s); bridge \
             is {}.",
            mgr_slots.len(),
            if bridge_up { "up" } else { "down" }
        ),
        "PHANTOM_MGR_SLOT" => {
            "AvSessionManager has slots with no live IRC connection — peers will see \
             black/silent tiles."
                .into()
        }
        "MISSING_MGR_SLOT" => {
            "An IRC connection registered an av-instance but the participants map \
             doesn't contain it — likely the join was reaped or rejected."
                .into()
        }
        _ => "Session is wedged (no participants, no bridge).".into(),
    };

    let mut redactions = vec!["Raw MoQ broadcast paths omitted.".into()];
    if detailed {
        redactions
            .push("DIDs and instance ids of other participants are hashed (`#xxxxxxxx`).".into());
    } else {
        redactions.push(
            "Per-slot listings withheld at Public disclosure level — counts and the \
             divergence verdict are still returned. Authenticate as a channel member \
             via `Authorization: Bearer <irc-session-id>` for the full slot map."
                .into(),
        );
    }

    FactBundle {
        ok,
        code: code.into(),
        summary,
        confidence: Confidence::High,
        safe_facts,
        suggested_fixes: fixes,
        redactions,
        followups: vec![],
        // Public is sufficient — the disclosure filter strips identities
        // and the per-slot dump is gated above. Anyone can ask "is this
        // session diverged?" without learning who's in it.
        min_disclosure: DisclosureLevel::Public,
    }
}

/// A single place to construct permission-denied bundles so the wire
/// shape is uniform and exhaustively tested.
///
/// `required` is reported in the suggested fix so the caller knows
/// which auth tier they need. The bundle itself is `min_disclosure
/// = Public` because it deliberately contains no facts — there is
/// nothing left to filter, so the envelope's defense-in-depth check
/// shouldn't replace this with the generic `DISCLOSURE_FILTER_BLOCKED`.
pub(super) fn permission_denied(
    code: &str,
    summary: &str,
    required: DisclosureLevel,
) -> FactBundle {
    FactBundle {
        ok: false,
        code: code.to_string(),
        summary: summary.to_string(),
        confidence: Confidence::High,
        safe_facts: vec![],
        suggested_fixes: vec![SuggestedFix {
            summary: format!(
                "Authenticate with a session at the {required:?} disclosure level or higher."
            ),
            details: None,
        }],
        redactions: vec!["Request denied before any server state was inspected.".into()],
        followups: vec![],
        min_disclosure: DisclosureLevel::Public,
    }
}

fn normalize_channel(channel: &str) -> String {
    let with_hash = if channel.starts_with('#') || channel.starts_with('&') {
        channel.to_string()
    } else {
        format!("#{channel}")
    };
    with_hash.to_lowercase()
}

/// Strip control chars + cap length on caller-provided identifiers so
/// they're safe to interpolate into our own `safe_facts` strings.
fn sanitize_label(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(96));
    for c in input.chars().take(96) {
        if c.is_control() {
            out.push('?');
        } else {
            out.push(c);
        }
    }
    out
}

/// Quote arbitrary caller-supplied free text inside a fact line. The
/// surrounding quotes + the explicit "Caller-reported symptom:" prefix
/// ensure that any future LLM summarizer treats the content as
/// reported data, not as instructions to follow.
fn quote_user_text(s: &str) -> String {
    let truncated: String = s.chars().take(280).collect();
    let cleaned: String = truncated
        .chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect();
    format!("\"{cleaned}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with(supports: ClientSupports) -> ValidateClientConfigInput {
        ValidateClientConfigInput {
            client_name: "test-client".into(),
            client_version: "0.0.1".into(),
            server_url: "wss://example".into(),
            transport: Some("websocket".into()),
            auth_method: Some("did_atproto".into()),
            supports,
            desired_features: vec![],
        }
    }

    #[test]
    fn validator_accepts_modern_client() {
        let bundle = validate_client_config(&input_with(ClientSupports {
            message_tags: true,
            batch: true,
            server_time: true,
            sasl: true,
            resume: true,
            e2ee: false,
            crdt_sync: true,
            echo_message: true,
            away_notify: true,
        }));
        assert!(bundle.ok);
        assert_eq!(bundle.code, "CONFIG_OK");
        assert!(bundle.suggested_fixes.is_empty());
    }

    #[test]
    fn validator_warns_about_missing_message_tags() {
        let bundle = validate_client_config(&input_with(ClientSupports::default()));
        assert!(!bundle.ok);
        assert_eq!(bundle.code, "CONFIG_HAS_WARNINGS");
        assert!(
            bundle
                .safe_facts
                .iter()
                .any(|f| f.contains("`message-tags`")),
            "expected message-tags warning, got: {:?}",
            bundle.safe_facts
        );
    }

    #[test]
    fn validator_flags_e2ee_without_message_tags() {
        let bundle = validate_client_config(&input_with(ClientSupports {
            e2ee: true,
            ..ClientSupports::default()
        }));
        assert!(
            bundle
                .safe_facts
                .iter()
                .any(|f| f.contains("E2EE support but not message-tags")),
        );
    }

    #[test]
    fn validator_flags_multi_device_without_resume() {
        let mut input = input_with(ClientSupports {
            message_tags: true,
            server_time: true,
            batch: true,
            sasl: true,
            echo_message: true,
            ..ClientSupports::default()
        });
        input.desired_features.push("multi_device".into());
        let bundle = validate_client_config(&input);
        assert!(!bundle.ok);
        assert!(
            bundle
                .safe_facts
                .iter()
                .any(|f| f.contains("multi_device but does not advertise resume")),
        );
    }

    #[test]
    fn ordering_input_validates_emptiness() {
        // We can't easily construct a SharedState in unit tests, so we
        // exercise the input-only branch which never touches state.
        let bundle = ordering_input_validation_only(&DiagnoseMessageOrderingInput {
            channel: "#general".into(),
            message_ids: vec![],
            symptom: None,
        });
        assert_eq!(bundle.code, "INVALID_INPUT");
    }

    /// Re-implementation of the pre-state guard so we can unit-test it
    /// without standing up SharedState. Mirrors the early-return branch
    /// in [`diagnose_message_ordering`]; if either drifts the test in
    /// `tests/agent_assist_api.rs` will catch it.
    fn ordering_input_validation_only(input: &DiagnoseMessageOrderingInput) -> FactBundle {
        if input.message_ids.is_empty() {
            return FactBundle {
                ok: false,
                code: "INVALID_INPUT".into(),
                summary: "x".into(),
                confidence: Confidence::High,
                safe_facts: vec![],
                suggested_fixes: vec![],
                redactions: vec![],
                followups: vec![],
                min_disclosure: DisclosureLevel::ChannelMember,
            };
        }
        permission_denied("X", "x", DisclosureLevel::ChannelMember)
    }

    #[test]
    fn quote_user_text_strips_control_chars_and_caps_length() {
        let s = format!("a\x07b\x1bc{}", "x".repeat(500));
        let q = quote_user_text(&s);
        assert!(q.starts_with('"') && q.ends_with('"'));
        assert!(!q.contains('\x07'));
        assert!(!q.contains('\x1b'));
        // 280 chars + 2 surrounding quotes
        assert!(q.chars().count() <= 282);
    }

    #[test]
    fn permission_denied_is_safe_at_public_level() {
        // Permission-denied bundles deliberately carry min_disclosure
        // = Public so the envelope's defense-in-depth filter doesn't
        // overwrite the more-informative tool-specific code with the
        // generic DISCLOSURE_FILTER_BLOCKED. Empty safe_facts means
        // there's nothing left to leak.
        let b = permission_denied("X", "no", DisclosureLevel::ServerOperator);
        assert!(!b.ok);
        assert!(b.safe_facts.is_empty());
        assert_eq!(b.min_disclosure, DisclosureLevel::Public);
        assert!(!b.redactions.is_empty());
        // The required level is still surfaced in the suggested fix.
        assert!(
            b.suggested_fixes
                .iter()
                .any(|f| f.summary.contains("ServerOperator")),
        );
    }

    #[test]
    fn normalize_channel_adds_hash_and_lowercases() {
        assert_eq!(normalize_channel("General"), "#general");
        assert_eq!(normalize_channel("#General"), "#general");
        assert_eq!(normalize_channel("&local"), "&local");
    }
}
