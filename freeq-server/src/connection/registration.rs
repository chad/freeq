#![allow(clippy::too_many_arguments)]
//! IRC registration (NICK/USER completion).

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;

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
        let owner_did = state.nick_owners.lock().unwrap().get(&nick_lower).cloned();
        if let Some(owner) = owner_did {
            let is_owner = conn.authenticated_did.as_ref().is_some_and(|d| d == &owner);
            if !is_owner {
                // Generate a guest nick
                let guest_nick = format!("Guest{}", &session_id[session_id.len().saturating_sub(4)..]);
                let notice = Message::from_server(
                    server_name,
                    "NOTICE",
                    vec!["*", &format!("Nick {nick} is registered. You have been renamed to {guest_nick}")],
                );
                send(state, session_id, format!("{notice}\r\n"));
                state.nick_to_session.lock().unwrap().remove(nick);
                state.nick_to_session.lock().unwrap().insert(guest_nick.clone(), session_id.to_string());
                conn.nick = Some(guest_nick);
            }
        }
    }

    // Ghost any existing session with the same DID.
    // When a DID-authenticated user reconnects (e.g. from another device),
    // the old session is killed — like NickServ GHOST but automatic.
    if let Some(ref did) = conn.authenticated_did {
        let mut sessions_to_ghost = Vec::new();
        {
            let session_dids = state.session_dids.lock().unwrap();
            for (sid, d) in session_dids.iter() {
                if d == did && sid != session_id {
                    sessions_to_ghost.push(sid.clone());
                }
            }
        }
        for old_session in &sessions_to_ghost {
            // Find old nick
            let old_nick = state.nick_to_session.lock().unwrap()
                .iter()
                .find(|(_, s)| *s == old_session)
                .map(|(n, _)| n.clone());
            if let Some(ref old_nick) = old_nick {
                // Send QUIT to all channels the old session is in
                let channels: Vec<String> = state.channels.lock().unwrap()
                    .iter()
                    .filter(|(_, ch)| ch.members.contains(old_session))
                    .map(|(name, _)| name.clone())
                    .collect();
                let quit_msg = format!(
                    ":{old_nick}!~u@host QUIT :Ghosted (same identity reconnected)\r\n"
                );
                for ch_name in &channels {
                    let members: Vec<String> = state.channels.lock().unwrap()
                        .get(ch_name)
                        .map(|ch| ch.members.iter().cloned().collect())
                        .unwrap_or_default();
                    let conns = state.connections.lock().unwrap();
                    for member_session in &members {
                        if member_session != old_session {
                            if let Some(tx) = conns.get(member_session) {
                                let _ = tx.try_send(quit_msg.clone());
                            }
                        }
                    }
                }
                // Remove from channels
                let mut channels_lock = state.channels.lock().unwrap();
                for ch_name in &channels {
                    if let Some(ch) = channels_lock.get_mut(ch_name) {
                        ch.members.remove(old_session);
                        ch.ops.remove(old_session);
                        ch.voiced.remove(old_session);
                    }
                }
                drop(channels_lock);
                // Remove nick mapping
                state.nick_to_session.lock().unwrap().remove(old_nick);
            }
            // Clean up session metadata
            state.session_dids.lock().unwrap().remove(old_session);
            state.session_handles.lock().unwrap().remove(old_session);
            state.session_iroh_ids.lock().unwrap().remove(old_session);
            // Send ERROR to old session and close it
            if let Some(tx) = state.connections.lock().unwrap().get(old_session) {
                let _ = tx.try_send(
                    "ERROR :Closing link (same identity reconnected)\r\n".to_string()
                );
            }
            // Remove old session from connections (causes its read loop to end)
            state.connections.lock().unwrap().remove(old_session);
            tracing::info!(did = %did, old_session = %old_session, "Ghosted old session for same DID");
        }

        // After ghosting, try to reclaim the desired nick if we got a fallback (e.g. nick_).
        // The DID owner's preferred nick is now free.
        let reclaim = conn.nick.as_ref()
            .filter(|n| n.ends_with('_'))
            .map(|n| (n.clone(), n.trim_end_matches('_').to_string()));
        if let Some((current_nick, desired)) = reclaim {
            let nick_free = !state.nick_to_session.lock().unwrap()
                .keys().any(|k| k.to_lowercase() == desired.to_lowercase());
            if nick_free {
                state.nick_to_session.lock().unwrap().remove(&current_nick);
                state.nick_to_session.lock().unwrap()
                    .insert(desired.clone(), session_id.to_string());
                tracing::info!(old = %current_nick, new = %desired, "Reclaimed nick after ghost");
                conn.nick = Some(desired);
            }
        }
    }

    conn.registered = true;
    let nick = conn.nick.as_deref().unwrap();

    // Store iroh endpoint ID in shared state for WHOIS lookups
    if let Some(ref iroh_id) = conn.iroh_endpoint_id {
        state.session_iroh_ids.lock().unwrap()
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

