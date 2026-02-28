#![allow(clippy::too_many_arguments)]
//! Helper functions for broadcasting, S2S relay, and utilities.

use std::sync::Arc;
use crate::server::{RemoteMember, SharedState};

/// Generate a cloaked hostname from an optional DID.
pub fn cloaked_host_for_did(did: Option<&str>) -> String {
    if let Some(did) = did {
        let short = did.strip_prefix("did:").unwrap_or(did);
        let parts: Vec<&str> = short.splitn(2, ':').collect();
        if parts.len() == 2 {
            let method = parts[0];
            let id = &parts[1][..parts[1].len().min(8)];
            format!("freeq/{method}/{id}")
        } else {
            "freeq/did".to_string()
        }
    } else {
        "freeq/guest".to_string()
    }
}
use super::Connection;

/// Resolved target of a nick within a channel's roster.
///
/// This is the canonical way to resolve a nick for any operation that
/// "acts on" a user in a channel (MODE +o/-o, +v/-v, KICK, INVITE).
/// It consults both local session tables and S2S remote_members.
pub(super) enum ChannelTarget {
    /// Nick belongs to a user connected to this server.
    Local { session_id: String },
    /// Nick belongs to a user on a remote federated server.
    Remote(RemoteMember),
    /// Nick is not in the channel's roster (local or remote).
    NotPresent,
}

/// Resolve a nick within a channel, checking both local members and
/// remote members from S2S federation.
///
/// Returns `ChannelTarget::Local` if the nick maps to a local session
/// that is a member of the channel, `ChannelTarget::Remote` if the nick
/// is in `remote_members`, or `ChannelTarget::NotPresent` otherwise.
pub(super) fn resolve_channel_target(
    state: &SharedState,
    channel: &str,
    target_nick: &str,
) -> ChannelTarget {
    let nick_lower = target_nick.to_lowercase();

    // Check local: case-insensitive nick → session, session ∈ channel.members
    let local_session = {
        let n2s = state.nick_to_session.lock();
        n2s.get_session(target_nick).map(|s| s.to_string())
    };
    if let Some(ref sid) = local_session {
        let in_channel = state.channels.lock()
            .get(channel)
            .map(|ch| ch.members.contains(sid))
            .unwrap_or(false);
        if in_channel {
            return ChannelTarget::Local { session_id: sid.clone() };
        }
    }

    // Check remote: case-insensitive nick ∈ channel.remote_members
    let remote = state.channels.lock()
        .get(channel)
        .and_then(|ch| {
            ch.remote_members.iter()
                .find(|(n, _)| n.to_lowercase() == nick_lower)
                .map(|(_, rm)| rm.clone())
        });
    if let Some(rm) = remote {
        return ChannelTarget::Remote(rm);
    }

    ChannelTarget::NotPresent
}

/// Resolved target of a nick anywhere on the network.
///
/// Unlike `ChannelTarget`, this doesn't require the nick to be in a
/// specific channel — it just checks if the nick exists at all (locally
/// or as a known remote user in any channel).
pub(super) enum NetworkTarget {
    /// Nick belongs to a user connected to this server.
    Local { session_id: String },
    /// Nick belongs to a user on a remote federated server.
    Remote(RemoteMember),
    /// Nick is not known anywhere.
    Unknown,
}

/// Resolve a nick across the entire network: local sessions + all
/// channels' remote_members. Used for operations like INVITE where
/// the target doesn't need to be in a specific channel.
pub(super) fn resolve_network_target(
    state: &SharedState,
    target_nick: &str,
) -> NetworkTarget {
    let nick_lower = target_nick.to_lowercase();

    // Check local first (case-insensitive — NickMap handles it)
    let local_sid = {
        let n2s = state.nick_to_session.lock();
        n2s.get_session(target_nick).map(|s| s.to_string())
    };
    if let Some(sid) = local_sid {
        return NetworkTarget::Local { session_id: sid };
    }

    // Check all channels' remote_members (case-insensitive)
    let channels = state.channels.lock();
    for ch in channels.values() {
        let rm = ch.remote_members.iter()
            .find(|(n, _)| n.to_lowercase() == nick_lower)
            .map(|(_, rm)| rm.clone());
        if let Some(rm) = rm {
            return NetworkTarget::Remote(rm);
        }
    }

    NetworkTarget::Unknown
}

pub(super) fn normalize_channel(name: &str) -> String {
    name.to_lowercase()
}


pub(super) fn s2s_broadcast(state: &Arc<SharedState>, msg: crate::s2s::S2sMessage) {
    let manager = state.s2s_manager.lock().clone();
    if let Some(manager) = manager {
        manager.broadcast(msg);
    }
}

/// Generate a unique event ID for outgoing S2S messages.
pub(super) fn s2s_next_event_id(state: &Arc<SharedState>) -> String {
    let manager = state.s2s_manager.lock().clone();
    match manager {
        Some(m) => m.next_event_id(),
        None => String::new(),
    }
}

/// Broadcast a channel mode change to S2S peers.
pub(super) fn s2s_broadcast_mode(
    state: &Arc<SharedState>,
    conn: &Connection,
    channel: &str,
    mode: &str,
    arg: Option<&str>,
) {
    let event_id = s2s_next_event_id(state);
    let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
    s2s_broadcast(state, crate::s2s::S2sMessage::Mode {
        event_id,
        channel: channel.to_string(),
        mode: mode.to_string(),
        arg: arg.map(|s| s.to_string()),
        set_by: conn.nick.as_deref().unwrap_or("*").to_string(),
        origin,
    });
}

pub(super) fn broadcast_to_channel(state: &Arc<SharedState>, channel: &str, msg: &str) {
    let members: Vec<String> = state
        .channels
        .lock()
        .get(channel)
        .map(|ch| ch.members.iter().cloned().collect())
        .unwrap_or_default();

    let conns = state.connections.lock();
    for member_session in &members {
        if let Some(tx) = conns.get(member_session) {
            let _ = tx.try_send(msg.to_string());
        }
    }
}


pub(crate) fn broadcast_account_notify(
    state: &SharedState,
    session_id: &str,
    nick: &str,
    did: &str,
) {
    let host = cloaked_host_for_did(Some(did));
    let hostmask = format!("{nick}!~u@{host}");
    let line = format!(":{hostmask} ACCOUNT {did}\r\n");

    // Find all channels this user is in
    let channels = state.channels.lock();
    let mut notified = std::collections::HashSet::new();
    for ch in channels.values() {
        if ch.members.contains(session_id) {
            let cap_set = state.cap_account_notify.lock();
            let conns = state.connections.lock();
            for member_sid in &ch.members {
                if member_sid != session_id && !notified.contains(member_sid) {
                    if cap_set.contains(member_sid)
                        && let Some(tx) = conns.get(member_sid) {
                            let _ = tx.try_send(line.clone());
                        }
                    notified.insert(member_sid.clone());
                }
            }
        }
    }
}

/// Build a JOIN line for extended-join capable clients.
/// Format: `:nick!user@host JOIN #channel account :realname`
pub(crate) fn make_extended_join(hostmask: &str, channel: &str, did: Option<&str>, realname: &str) -> String {
    let account = did.unwrap_or("*");
    format!(":{hostmask} JOIN {channel} {account} :{realname}\r\n")
}

/// Build a standard JOIN line.
pub(crate) fn make_standard_join(hostmask: &str, channel: &str) -> String {
    format!(":{hostmask} JOIN {channel}\r\n")
}
