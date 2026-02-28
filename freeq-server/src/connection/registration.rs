#![allow(clippy::too_many_arguments)]
//! IRC registration (NICK/USER completion).

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;

/// Attach a new session to existing sessions with the same DID.
/// Instead of ghosting (killing) old sessions, this enables multi-device:
/// - The new session shares the same nick
/// - The new session is added to all channels the DID is already in
/// - Messages fan out to all sessions for the DID
/// - The user appears once in member lists
///
/// Called at SASL success time.
pub(super) fn attach_same_did(
    conn: &mut Connection,
    state: &Arc<SharedState>,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let did = match conn.authenticated_did.as_ref() {
        Some(d) => d.clone(),
        None => return,
    };

    // Register this session in did_sessions
    state.did_sessions.lock()
        .entry(did.clone())
        .or_default()
        .insert(session_id.to_string());

    // Check for ghost session (recently disconnected — reclaim without join/part churn)
    let ghost = state.ghost_sessions.lock().remove(&did);
    if let Some(ghost) = ghost {
        // Cancel the deferred QUIT broadcast
        let _ = ghost.cancel.send(());
        let elapsed = ghost.disconnect_time.elapsed();
        tracing::info!(
            did = %did, nick = %ghost.nick, session = %session_id,
            elapsed_ms = elapsed.as_millis() as u64,
            channels = ghost.channels.len(),
            "Reclaimed ghost session — suppressing quit/join churn"
        );

        // Adopt the ghost's nick
        if conn.nick.as_ref().map(|n| n.to_lowercase()) != Some(ghost.nick.to_lowercase()) {
            if let Some(ref old_nick) = conn.nick {
                state.nick_to_session.lock().remove_by_nick(old_nick);
            }
            conn.nick = Some(ghost.nick.clone());
        }
        // Point the nick at the new session
        state.nick_to_session.lock().insert(&ghost.nick, session_id);

        // Re-join all channels the ghost was in (silently — no broadcast)
        let mut channels = state.channels.lock();
        for (ch_name, was_op, was_voiced, was_halfop) in &ghost.channels {
            if let Some(ch) = channels.get_mut(&ch_name.to_lowercase()) {
                ch.members.insert(session_id.to_string());
                if *was_op { ch.ops.insert(session_id.to_string()); }
                if *was_voiced { ch.voiced.insert(session_id.to_string()); }
                if *was_halfop { ch.halfops.insert(session_id.to_string()); }
            }
        }
        drop(channels);

        // Store reclaimed channel names so try_complete_registration can send
        // synthetic state AFTER the client is fully registered (needed for CHATHISTORY).
        conn.ghost_channels = Some(ghost.channels.iter().map(|(name, _, _, _)| name.clone()).collect());

        return;
    }

    // Find existing sessions for this DID
    let existing_sessions: Vec<String> = {
        let session_dids = state.session_dids.lock();
        session_dids.iter()
            .filter(|(sid, d)| d.as_str() == did && sid.as_str() != session_id)
            .map(|(sid, _)| sid.clone())
            .collect()
    };

    if existing_sessions.is_empty() {
        // First session for this DID — normal registration
        // Ensure nick is in nick_to_session
        if let Some(ref nick) = conn.nick {
            let mut nts = state.nick_to_session.lock();
            if !nts.contains_nick(nick) {
                nts.insert(nick, session_id);
                tracing::info!(nick = %nick, "Registered nick for DID {did}");
            }
        }
        // Reclaim if we got a fallback nick with trailing '_'
        let reclaim = conn.nick.as_ref()
            .filter(|n| n.ends_with('_'))
            .map(|n| (n.clone(), n.trim_end_matches('_').to_string()));
        if let Some((current_nick, desired)) = reclaim {
            let mut nts = state.nick_to_session.lock();
            if !nts.contains_nick(&desired) {
                nts.remove_by_nick(&current_nick);
                nts.insert(&desired, session_id);
                tracing::info!(old = %current_nick, new = %desired, "Reclaimed nick");
                conn.nick = Some(desired);
            }
        }
        return;
    }

    // Multi-device attach: existing sessions exist for this DID
    tracing::info!(did = %did, session = %session_id, existing = ?existing_sessions.len(),
                   "Attaching additional session for DID");

    // Find the canonical nick from existing sessions
    let canonical_nick = {
        let nts = state.nick_to_session.lock();
        let sd = state.session_dids.lock();
        nts.iter()
            .find(|&(_, sid)| {
                let sid_str: &str = sid;
                sd.get(sid_str) == Some(&did)
            })
            .map(|(nick, _)| nick.to_string())
    };

    // Adopt the canonical nick
    if let Some(ref canon) = canonical_nick
        && conn.nick.as_ref().map(|n| n.to_lowercase()) != Some(canon.to_lowercase()) {
            // Remove any nick mapping we created during CAP/SASL
            if let Some(ref old_nick) = conn.nick {
                state.nick_to_session.lock().remove_by_nick(old_nick);
            }
            conn.nick = Some(canon.clone());
        }

    // Find all channels the DID is in via existing sessions
    let channels_to_join: Vec<String> = {
        let channels = state.channels.lock();
        channels.iter()
            .filter(|(_, ch)| {
                existing_sessions.iter().any(|sid| ch.members.contains(sid))
            })
            .map(|(name, _)| name.clone())
            .collect()
    };

    // Add this session to those channels (silently — no JOIN broadcast)
    {
        let mut channels = state.channels.lock();
        for ch_name in &channels_to_join {
            if let Some(ch) = channels.get_mut(ch_name) {
                ch.members.insert(session_id.to_string());
                // Copy op/voice status from existing session
                let is_op = existing_sessions.iter().any(|s| ch.ops.contains(s));
                let is_voiced = existing_sessions.iter().any(|s| ch.voiced.contains(s));
                if is_op { ch.ops.insert(session_id.to_string()); }
                if is_voiced { ch.voiced.insert(session_id.to_string()); }
            }
        }
    }

    // Send the new session a replay of channel state so it knows where it is
    let nick = conn.nick.as_deref().unwrap_or("*");
    let server_name = &state.server_name;
    for ch_name in &channels_to_join {
        // Synthesize JOIN for the client
        let host = super::helpers::cloaked_host_for_did(Some(did.as_str()));
        send(state, session_id, format!(":{nick}!~u@{host} JOIN {ch_name}\r\n"));

        // Send topic
        let channels = state.channels.lock();
        if let Some(ch) = channels.get(ch_name) {
            if let Some(ref topic) = ch.topic {
                let topic_msg = crate::irc::Message::from_server(
                    server_name, crate::irc::RPL_TOPIC,
                    vec![nick, ch_name, &topic.text],
                );
                send(state, session_id, format!("{topic_msg}\r\n"));
            }
            // Send NAMES
            let nts = state.nick_to_session.lock();
            let mut names: Vec<String> = Vec::new();
            let mut seen_nicks = std::collections::HashSet::new();
            for member_sid in &ch.members {
                if let Some(member_nick) = nts.get_nick(member_sid) {
                    let nick_lower = member_nick.to_lowercase();
                    if seen_nicks.contains(&nick_lower) { continue; }
                    seen_nicks.insert(nick_lower);
                    let prefix = if ch.ops.contains(member_sid) {
                        "@"
                    } else if ch.voiced.contains(member_sid) {
                        "+"
                    } else {
                        ""
                    };
                    names.push(format!("{prefix}{member_nick}"));
                }
            }
            drop(channels);
            let names_str = names.join(" ");
            let names_msg = crate::irc::Message::from_server(
                server_name, crate::irc::RPL_NAMREPLY,
                vec![nick, "=", ch_name, &names_str],
            );
            let end_msg = crate::irc::Message::from_server(
                server_name, crate::irc::RPL_ENDOFNAMES,
                vec![nick, ch_name, "End of /NAMES list"],
            );
            send(state, session_id, format!("{names_msg}\r\n{end_msg}\r\n"));
        } else {
            drop(channels);
        }
    }

    tracing::info!(did = %did, channels = ?channels_to_join.len(),
                   "Session attached to {} existing channels", channels_to_join.len());
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
                state.nick_to_session.lock().remove_by_nick(nick);
                state.nick_to_session.lock().insert(&guest_nick, session_id);
                conn.nick = Some(guest_nick);
            }
        }
    }

    // Multi-device attach is handled at SASL success time (cap.rs).
    // This catch-all covers edge cases where registration completes
    // without going through the SASL path.
    attach_same_did(conn, state, session_id, send);

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

    // Send synthetic state for ghost-reclaimed channels (now that registration is complete,
    // so the client can issue CHATHISTORY after receiving ENDOFNAMES).
    if let Some(ghost_chs) = conn.ghost_channels.take() {
        let nick = conn.nick.as_deref().unwrap_or("*").to_string();
        for ch_name in &ghost_chs {
            // Send JOIN to the client so it knows it's in the channel
            let hostmask = conn.hostmask();
            send(state, session_id, format!(":{hostmask} JOIN {ch_name}\r\n"));

            // Topic
            {
                let channels = state.channels.lock();
                if let Some(ch) = channels.get(&ch_name.to_lowercase())
                    && let Some(ref topic) = ch.topic {
                        let topic_msg = crate::irc::Message::from_server(
                            server_name, crate::irc::RPL_TOPIC,
                            vec![&nick, ch_name, &topic.text],
                        );
                        send(state, session_id, format!("{topic_msg}\r\n"));
                    }
            }

            // Names (sends NAMREPLY + ENDOFNAMES → triggers client CHATHISTORY request)
            super::channel::handle_names(conn, ch_name, state, server_name, session_id, send);
        }
    }

    // Auto-rejoin channels for DID-authenticated users.
    // Skip channels already joined via attach_same_did (multi-device).
    if let Some(ref did) = conn.authenticated_did {
        let did = did.clone();
        if let Some(channels) = state.with_db(|db| db.get_user_channels(&did)) {
            // Filter out channels this session is already in (from multi-device attach)
            let already_in: std::collections::HashSet<String> = {
                let chs = state.channels.lock();
                chs.iter()
                    .filter(|(_, ch)| ch.members.contains(session_id))
                    .map(|(name, _)| name.to_lowercase())
                    .collect()
            };
            let to_join: Vec<String> = channels.into_iter()
                .filter(|ch| !already_in.contains(&ch.to_lowercase()))
                .collect();
            if !to_join.is_empty() {
                tracing::info!(%session_id, %did, count = to_join.len(), "Auto-rejoining saved channels");
                for channel in to_join {
                    super::channel::handle_join(
                        conn, &channel, None, state, server_name, session_id, send,
                    );
                }
            }
        }
    }
}

