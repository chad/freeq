#![allow(clippy::too_many_arguments)]
//! Helper functions for broadcasting, S2S relay, and utilities.

use std::sync::Arc;
use crate::server::SharedState;
use super::Connection;

pub(super) fn normalize_channel(name: &str) -> String {
    name.to_lowercase()
}


pub(super) fn s2s_broadcast(state: &Arc<SharedState>, msg: crate::s2s::S2sMessage) {
    let manager = state.s2s_manager.lock().unwrap().clone();
    if let Some(manager) = manager {
        manager.broadcast(msg);
    }
}

/// Generate a unique event ID for outgoing S2S messages.
pub(super) fn s2s_next_event_id(state: &Arc<SharedState>) -> String {
    let manager = state.s2s_manager.lock().unwrap().clone();
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
    let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
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
        .unwrap()
        .get(channel)
        .map(|ch| ch.members.iter().cloned().collect())
        .unwrap_or_default();

    let conns = state.connections.lock().unwrap();
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
    let hostmask = format!("{nick}!~u@host");
    let line = format!(":{hostmask} ACCOUNT {did}\r\n");

    // Find all channels this user is in
    let channels = state.channels.lock().unwrap();
    let mut notified = std::collections::HashSet::new();
    for ch in channels.values() {
        if ch.members.contains(session_id) {
            let cap_set = state.cap_account_notify.lock().unwrap();
            let conns = state.connections.lock().unwrap();
            for member_sid in &ch.members {
                if member_sid != session_id && !notified.contains(member_sid) {
                    if cap_set.contains(member_sid) {
                        if let Some(tx) = conns.get(member_sid) {
                            let _ = tx.try_send(line.clone());
                        }
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
