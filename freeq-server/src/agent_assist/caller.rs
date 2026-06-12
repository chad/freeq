//! Caller identification + permission matrix.
//!
//! For MVP we accept one auth scheme: `Authorization: Bearer <session_id>`.
//! The session id must belong to a currently-connected, registered IRC
//! session, which gives us its DID via `state.session_dids`. If the
//! DID is in `--oper-dids`, the caller is a server operator. Anything
//! else is anonymous (`DisclosureLevel::Public`).
//!
//! Future PRs can add OAuth bearer tokens or signed DID assertions
//! here without changing tool code, since tools accept a [`Caller`]
//! and never look at headers themselves.

use super::types::{Caller, DisclosureLevel};
use crate::server::SharedState;
use axum::http::HeaderMap;
use std::sync::Arc;

/// Extract a [`Caller`] from request headers.
///
/// Anonymous on any failure (missing header, unknown session, etc.) —
/// tools that need a higher disclosure level will surface a clear
/// `PermissionDenied` diagnosis when downgraded.
pub fn extract(headers: &HeaderMap, state: &Arc<SharedState>) -> Caller {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    let Some(session_id) = bearer else {
        return Caller::anonymous();
    };
    if session_id.is_empty() {
        return Caller::anonymous();
    }

    // Resolve session_id → DID via the active sessions map.
    let did = state.session_dids.lock().get(session_id).cloned();
    let Some(did) = did else {
        return Caller::anonymous();
    };

    let level = if state.config.oper_dids.iter().any(|d| d == &did) {
        DisclosureLevel::ServerOperator
    } else {
        DisclosureLevel::Account
    };

    Caller {
        did: Some(did),
        session_id: Some(session_id.to_string()),
        level,
    }
}

/// True if `caller` is a member of `channel` (lowercase).
pub fn is_channel_member(caller: &Caller, state: &Arc<SharedState>, channel: &str) -> bool {
    let Some(sid) = &caller.session_id else {
        return false;
    };
    let channels = state.channels.lock();
    channels
        .get(&channel.to_lowercase())
        .map(|ch| ch.members.contains(sid))
        .unwrap_or(false)
}

/// True if `caller` is an op (`@`) in `channel` (lowercase).
pub fn is_channel_operator(caller: &Caller, state: &Arc<SharedState>, channel: &str) -> bool {
    let Some(sid) = &caller.session_id else {
        return false;
    };
    let channels = state.channels.lock();
    channels
        .get(&channel.to_lowercase())
        .map(|ch| ch.ops.contains(sid))
        .unwrap_or(false)
}

/// Effective disclosure level for `caller` against a specific channel.
/// Server operators always satisfy any channel-scoped requirement.
pub fn effective_level(
    caller: &Caller,
    state: &Arc<SharedState>,
    channel: &str,
) -> DisclosureLevel {
    if matches!(caller.level, DisclosureLevel::ServerOperator) {
        return DisclosureLevel::ServerOperator;
    }
    if is_channel_operator(caller, state, channel) {
        return DisclosureLevel::ChannelOperator;
    }
    if is_channel_member(caller, state, channel) {
        return DisclosureLevel::ChannelMember;
    }
    caller.level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_default() {
        let c = Caller::anonymous();
        assert!(c.did.is_none());
        assert!(c.session_id.is_none());
        assert_eq!(c.level, DisclosureLevel::Public);
        assert!(!c.is_self("did:plc:anything"));
    }

    #[test]
    fn disclosure_ordering_holds() {
        // Tools rely on this ordering to compare caller level to required level.
        assert!(DisclosureLevel::ServerOperator > DisclosureLevel::ChannelOperator);
        assert!(DisclosureLevel::ChannelOperator > DisclosureLevel::ChannelMember);
        assert!(DisclosureLevel::ChannelMember > DisclosureLevel::Account);
        assert!(DisclosureLevel::Account > DisclosureLevel::Public);
        assert!(DisclosureLevel::ServerOperator.satisfies(DisclosureLevel::Account));
        assert!(!DisclosureLevel::Public.satisfies(DisclosureLevel::Account));
    }

    #[test]
    fn is_self_compares_did() {
        let c = Caller {
            did: Some("did:plc:abc".into()),
            session_id: Some("s1".into()),
            level: DisclosureLevel::Account,
        };
        assert!(c.is_self("did:plc:abc"));
        assert!(!c.is_self("did:plc:xyz"));
    }
}
