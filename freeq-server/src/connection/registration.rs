#![allow(clippy::too_many_arguments)]
//! IRC registration (NICK/USER completion).

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;

/// Ghost all existing sessions with the same DID, then reclaim the nick
/// if the current one is a fallback (ends with `_`).
///
/// Called at SASL success time so the old session is removed before NICK
/// collision can block the new connection.
pub(super) fn ghost_same_did(
    conn: &mut Connection,
    state: &Arc<SharedState>,
    session_id: &str,
) {
    let did = match conn.authenticated_did.as_ref() {
        Some(d) => d.clone(),
        None => return,
    };

    let mut sessions_to_ghost = Vec::new();
    {
        let session_dids = state.session_dids.lock();
        for (sid, d) in session_dids.iter() {
            if *d == did && sid != session_id {
                sessions_to_ghost.push(sid.clone());
            }
        }
    }

    for old_session in &sessions_to_ghost {
        // Find old nick
        let old_nick = state.nick_to_session.lock()
            .iter()
            .find(|(_, s)| *s == old_session)
            .map(|(n, _)| n.clone());
        if let Some(ref old_nick) = old_nick {
            // Send QUIT to all channels the old session is in
            let channels: Vec<String> = state.channels.lock()
                .iter()
                .filter(|(_, ch)| ch.members.contains(old_session))
                .map(|(name, _)| name.clone())
                .collect();
            let host = super::helpers::cloaked_host_for_did(Some(did.as_str()));
            let quit_msg = format!(
                ":{old_nick}!~u@{host} QUIT :Ghosted (same identity reconnected)\r\n"
            );
            for ch_name in &channels {
                let members: Vec<String> = state.channels.lock()
                    .get(ch_name)
                    .map(|ch| ch.members.iter().cloned().collect())
                    .unwrap_or_default();
                let conns = state.connections.lock();
                for member_session in &members {
                    if member_session != old_session {
                        if let Some(tx) = conns.get(member_session) {
                            let _ = tx.try_send(quit_msg.clone());
                        }
                    }
                }
            }
            // Remove from channels
            let mut channels_lock = state.channels.lock();
            for ch_name in &channels {
                if let Some(ch) = channels_lock.get_mut(ch_name) {
                    ch.members.remove(old_session);
                    ch.ops.remove(old_session);
                    ch.voiced.remove(old_session);
                    ch.halfops.remove(old_session);
                }
            }
            drop(channels_lock);
            // Remove nick mapping
            state.nick_to_session.lock().remove(old_nick);
        }
        // Clean up session metadata
        state.session_dids.lock().remove(old_session);
        state.session_handles.lock().remove(old_session);
        state.session_iroh_ids.lock().remove(old_session);
        // Send ERROR to old session and close it
        if let Some(tx) = state.connections.lock().get(old_session) {
            let _ = tx.try_send(
                "ERROR :Closing link (same identity reconnected)\r\n".to_string()
            );
        }
        state.connections.lock().remove(old_session);
        tracing::info!(did = %did, old_session = %old_session, "Ghosted old session for same DID");
    }

    // After ghosting, ensure our nick is in nick_to_session.
    // It may not be there if the server deferred insertion during CAP/SASL
    // negotiation (nick was in use by the ghost we just killed).
    if let Some(ref nick) = conn.nick {
        let mut nts = state.nick_to_session.lock();
        let nick_lower = nick.to_lowercase();
        let already_mapped = nts.keys().any(|k| k.to_lowercase() == nick_lower);
        if !already_mapped {
            nts.insert(nick.clone(), session_id.to_string());
            tracing::info!(nick = %nick, "Registered nick after ghost");
        }
    }

    // Also reclaim if we got a fallback nick with trailing '_'.
    let reclaim = conn.nick.as_ref()
        .filter(|n| n.ends_with('_'))
        .map(|n| (n.clone(), n.trim_end_matches('_').to_string()));
    if let Some((current_nick, desired)) = reclaim {
        let nick_free = !state.nick_to_session.lock()
            .keys().any(|k| k.to_lowercase() == desired.to_lowercase());
        if nick_free {
            state.nick_to_session.lock().remove(&current_nick);
            state.nick_to_session.lock()
                .insert(desired.clone(), session_id.to_string());
            tracing::info!(old = %current_nick, new = %desired, "Reclaimed nick after ghost");
            conn.nick = Some(desired);
        }
    }
}

pub(super) fn try_complete_registration(
    conn: &mut Connection,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    if conn.registered || conn.cap_negotiating || conn.sasl_in_progress {
        return;
    }
    if conn.nick.is_none() || conn.user.is_none() {
        return;
    }

    // Enforce nick ownership at registration time.
    // If the user claimed a registered nick during CAP negotiation
    // but didn't authenticate as the owner, force-rename them.
    if let Some(ref nick) = conn.nick {
        let nick_lower = nick.to_lowercase();
        let owner_did = state.nick_owners.lock().get(&nick_lower).cloned();
        if let Some(owner) = owner_did {
            let is_owner = conn.authenticated_did.as_ref().is_some_and(|d| d == &owner);
            if !is_owner {
                // Nick is registered to a DID — rename to a temp nick.
                // The web client detects Guest rename and disconnects (no ghost).
                // The iOS client continues with the temp nick and auto-joins channels.
                let guest_id: u32 = rand::random::<u32>() % 100000;
                let guest_nick = format!("Guest{guest_id}");
                let notice = Message::from_server(
                    server_name,
                    "NOTICE",
                    vec!["*", &format!("Nick {nick} is registered — renamed to {guest_nick}. Authenticate to reclaim.")],
                );
                send(state, session_id, format!("{notice}\r\n"));
                state.nick_to_session.lock().remove(nick);
                state.nick_to_session.lock().insert(guest_nick.clone(), session_id.to_string());
                conn.nick = Some(guest_nick);
            }
        }
    }

    // Ghost + nick reclaim is handled at SASL success time (cap.rs).
    // This catch-all covers edge cases where registration completes
    // without going through the SASL path.
    ghost_same_did(conn, state, session_id);

    conn.registered = true;
    let nick = conn.nick.as_deref().unwrap();

    // Store iroh endpoint ID in shared state for WHOIS lookups
    if let Some(ref iroh_id) = conn.iroh_endpoint_id {
        state.session_iroh_ids.lock()
            .insert(session_id.to_string(), iroh_id.clone());
    }

    let auth_info = match &conn.authenticated_did {
        Some(did) => format!(" (authenticated as {did})"),
        None => " (guest)".to_string(),
    };

    let welcome = Message::from_server(
        server_name,
        irc::RPL_WELCOME,
        vec![
            nick,
            &format!("Welcome to {server_name}, {nick}{auth_info}"),
        ],
    );
    let yourhost = Message::from_server(
        server_name,
        irc::RPL_YOURHOST,
        vec![
            nick,
            &format!("Your host is {server_name}, running freeq 0.1"),
        ],
    );
    let created = Message::from_server(
        server_name,
        irc::RPL_CREATED,
        vec![nick, "This server was created just now"],
    );
    let myinfo = Message::from_server(
        server_name,
        irc::RPL_MYINFO,
        vec![nick, server_name, "freeq-0.1", "o", "o"],
    );

    for msg in [welcome, yourhost, created, myinfo] {
        send(state, session_id, format!("{msg}\r\n"));
    }

    // Send MOTD
    if let Some(ref motd) = state.config.motd {
        let start = Message::from_server(
            server_name,
            irc::RPL_MOTDSTART,
            vec![nick, &format!("- {server_name} Message of the day -")],
        );
        send(state, session_id, format!("{start}\r\n"));
        for line in motd.lines() {
            let motd_line = Message::from_server(
                server_name,
                irc::RPL_MOTD,
                vec![nick, &format!("- {line}")],
            );
            send(state, session_id, format!("{motd_line}\r\n"));
        }
        let end = Message::from_server(
            server_name,
            irc::RPL_ENDOFMOTD,
            vec![nick, "End of /MOTD command"],
        );
        send(state, session_id, format!("{end}\r\n"));
    } else {
        let no_motd = Message::from_server(
            server_name,
            irc::ERR_NOMOTD,
            vec![nick, "MOTD File is missing"],
        );
        send(state, session_id, format!("{no_motd}\r\n"));
    }
}

