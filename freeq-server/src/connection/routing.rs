//! # Federation Routing Layer
//!
//! ## Architectural rule
//!
//! `remote_members` is a **display cache**, not a routing gate.
//!
//! It tells us what to show in NAMES, WHOIS, and WHO. It does NOT
//! determine whether a nick is reachable. The two concepts are different:
//!
//! - **Display**: "Is this nick in a channel we're tracking?" → check `remote_members`
//! - **Routing**: "Can we deliver a message to this nick?" → check local, then try S2S
//! - **Authorization**: "Is this nick an op?" → check `remote_members.is_op` / `did_ops`
//!
//! Any code that gates an **action** (PM, KICK, INVITE, MODE) on
//! `remote_members.contains_key()` is a latent asymmetric-federation bug.
//! Sync is eventually-consistent and may not have completed in both
//! directions. The receiving server doesn't need remote_members to
//! deliver a message — it just checks nick_to_session.
//!
//! ## When to use what
//!
//! | Need | Use | NOT |
//! |------|-----|-----|
//! | Send PM to nick | `relay_to_nick()` | `remote_members.contains_key()` |
//! | Show nick in NAMES | `remote_members` iteration | — |
//! | Check if nick is op | `resolve_channel_target()` | — |
//! | Kick remote user | `resolve_channel_target()` | ad-hoc remote_members scan |
//! | Invite any nick | `resolve_network_target()` | ad-hoc scan |
//! | WHOIS info | `remote_members.get()` (display) | — |
//!
//! ## Enforcement
//!
//! `scripts/lint-federation.sh` greps for patterns that indicate
//! local-only lookups or remote_members routing gates in action paths.
//! Run it in CI.

use crate::server::SharedState;
use std::sync::Arc;

/// The result of trying to route a message to a nick.
pub(crate) enum RouteResult {
    /// Nick is a local user — here's their session ID.
    Local(String),
    /// Nick is not local but we have S2S peers — message was relayed.
    Relayed,
    /// Nick is not local and we have no S2S peers — truly unreachable.
    Unreachable,
}

/// Route a PRIVMSG/NOTICE to a nick. Checks local first, then relays
/// to all S2S peers if federation is active. Never gates on
/// `remote_members` — that's a display cache, not a routing table.
///
/// When `multiline_lines` is `Some`, the message originated as a
/// `draft/multiline` BATCH and the breakdown is included in the S2S
/// relay event so the peer can re-emit BATCH frames to its own
/// multiline-capable channel members. For local-target delivery,
/// `multiline_lines` is unused — the caller (messaging.rs DM branch)
/// does its own per-receiver wire formatting via `build_dm_frames`.
pub(crate) fn relay_to_nick(
    state: &Arc<SharedState>,
    from: &str,
    target: &str,
    text: &str,
    event_id: String,
    multiline_lines: Option<&[crate::connection::draft_multiline::BatchLine]>,
) -> RouteResult {
    // 1. Local delivery (case-insensitive nick lookup)
    let _target_lower = target.to_lowercase();
    let local_session = {
        let n2s = state.nick_to_session.lock();
        n2s.get_session(target).map(|s| s.to_string())
    };
    if let Some(sid) = local_session {
        return RouteResult::Local(sid);
    }

    // 2. S2S relay (if federation active)
    let has_s2s = state.s2s_manager.lock().is_some();
    if has_s2s {
        let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
        let manager = state.s2s_manager.lock().clone();
        if let Some(m) = manager {
            let (s2s_text, s2s_tags) =
                crate::s2s::encode_privmsg_text_for_s2s(text, std::collections::HashMap::new());
            m.broadcast(crate::s2s::S2sMessage::Privmsg {
                event_id,
                from: from.to_string(),
                target: target.to_string(),
                text: s2s_text,
                origin,
                msgid: None, // PM relay — no msgid (recipient server assigns)
                sig: None,   // PM relay — sig not available at routing layer
                tags: s2s_tags,
                multiline_lines: multiline_lines.map(|lines| {
                    lines
                        .iter()
                        .map(|l| crate::s2s::MultilineLine {
                            body: l.body.clone(),
                            concat: l.concat_to_previous,
                        })
                        .collect()
                }),
            });
        }
        return RouteResult::Relayed;
    }

    // 3. No federation — nick doesn't exist
    RouteResult::Unreachable
}
