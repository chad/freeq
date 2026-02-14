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

