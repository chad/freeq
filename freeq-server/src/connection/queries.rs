#![allow(clippy::too_many_arguments)]
//! Query commands: WHOIS, WHO, LUSERS, AWAY.

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;
use super::helpers::normalize_channel;

pub(super) fn handle_whois(
    conn: &Connection,
    target_nick: &str,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let my_nick = conn.nick_or_star();

    // Find target's session
    let target_session = state
        .nick_to_session
        .lock()
        .unwrap()
        .get(target_nick)
        .cloned();

    let Some(target_session) = target_session else {
        // Check if this is a remote user (from S2S)
        let remote_info: Option<crate::server::RemoteMember> = {
            let channels = state.channels.lock().unwrap();
            channels.values()
                .find_map(|ch| ch.remote_members.get(target_nick).cloned())
        };

        if let Some(rm) = remote_info {
            // Remote user — show what we know
            let realname = match (&rm.handle, &rm.did) {
                (Some(h), _) => format!("{h} (via S2S federation)"),
                (_, Some(d)) => format!("{d} (via S2S federation)"),
                _ => "Remote user (via S2S federation)".to_string(),
            };
            let whoisuser = Message::from_server(
                server_name,
                irc::RPL_WHOISUSER,
                vec![my_nick, target_nick, target_nick, "s2s", "*", &realname],
            );
            send(state, session_id, format!("{whoisuser}\r\n"));

            let whoisserver = Message::from_server(
                server_name,
                irc::RPL_WHOISSERVER,
                vec![my_nick, target_nick, "s2s", &format!("Connected via S2S ({}…)", &rm.origin[..16.min(rm.origin.len())])],
            );
            send(state, session_id, format!("{whoisserver}\r\n"));

            // Show AT Protocol handle
            if let Some(ref handle) = rm.handle {
                let handle_line = Message::from_server(
                    server_name,
                    "671",
                    vec![my_nick, target_nick, &format!("AT Protocol handle: {handle}")],
                );
                send(state, session_id, format!("{handle_line}\r\n"));
            }

            // Show DID
            if let Some(ref d) = rm.did {
                let did_line = Message::from_server(
                    server_name,
                    irc::RPL_WHOISACCOUNT,
                    vec![my_nick, target_nick, d, "is authenticated as"],
                );
                send(state, session_id, format!("{did_line}\r\n"));
            }

            // Show channels they're in
            let user_channels: Vec<String> = {
                let channels = state.channels.lock().unwrap();
                channels.iter()
                    .filter(|(_, ch)| ch.remote_members.contains_key(target_nick))
                    .map(|(name, ch)| {
                        let is_op = rm.did.as_ref().is_some_and(|d| {
                            ch.founder_did.as_deref() == Some(d) || ch.did_ops.contains(d)
                        });
                        if is_op { format!("@{name}") } else { name.clone() }
                    })
                    .collect()
            };
            if !user_channels.is_empty() {
                let channels_line = Message::from_server(
                    server_name,
                    "319",  // RPL_WHOISCHANNELS
                    vec![my_nick, target_nick, &user_channels.join(" ")],
                );
                send(state, session_id, format!("{channels_line}\r\n"));
            }

            let end = Message::from_server(
                server_name,
                irc::RPL_ENDOFWHOIS,
                vec![my_nick, target_nick, "End of /WHOIS list"],
            );
            send(state, session_id, format!("{end}\r\n"));
        } else {
            let reply = Message::from_server(
                server_name,
                irc::ERR_NOSUCHNICK,
                vec![my_nick, target_nick, "No such nick"],
            );
            send(state, session_id, format!("{reply}\r\n"));
            let end = Message::from_server(
                server_name,
                irc::RPL_ENDOFWHOIS,
                vec![my_nick, target_nick, "End of /WHOIS list"],
            );
            send(state, session_id, format!("{end}\r\n"));
        }
        return;
    };

    // 311 RPL_WHOISUSER
    let whoisuser = Message::from_server(
        server_name,
        irc::RPL_WHOISUSER,
        vec![my_nick, target_nick, "~u", "host", "*", "IRC User"],
    );
    send(state, session_id, format!("{whoisuser}\r\n"));

    // 312 RPL_WHOISSERVER
    let whoisserver = Message::from_server(
        server_name,
        irc::RPL_WHOISSERVER,
        vec![my_nick, target_nick, server_name, "freeq"],
    );
    send(state, session_id, format!("{whoisserver}\r\n"));

    // 330 RPL_WHOISACCOUNT — show DID if authenticated
    let did = state
        .session_dids
        .lock()
        .unwrap()
        .get(&target_session)
        .cloned();

    if let Some(ref did) = did {
        let whoisaccount = Message::from_server(
            server_name,
            irc::RPL_WHOISACCOUNT,
            vec![my_nick, target_nick, did, "is authenticated as"],
        );
        send(state, session_id, format!("{whoisaccount}\r\n"));
    }

    // Show Bluesky handle if resolved
    if did.is_some() {
        let handle = state
            .session_handles
            .lock()
            .unwrap()
            .get(&target_session)
            .cloned();
        if let Some(handle) = handle {
            // Use a server notice (not a standard numeric, but informational)
            let notice = Message::from_server(
                server_name,
                "671",  // RPL_WHOISSECURE (repurposed for extra info)
                vec![my_nick, target_nick, &format!("AT Protocol handle: {handle}")],
            );
            send(state, session_id, format!("{notice}\r\n"));
        }
    }

    // Show iroh endpoint ID if connected via iroh
    let iroh_id = state
        .session_iroh_ids
        .lock()
        .unwrap()
        .get(&target_session)
        .cloned();
    if let Some(iroh_id) = iroh_id {
        let iroh_notice = Message::from_server(
            server_name,
            "672",  // Custom numeric for iroh info
            vec![my_nick, target_nick, &format!("iroh endpoint: {iroh_id}")],
        );
        send(state, session_id, format!("{iroh_notice}\r\n"));
    }

    // 318 RPL_ENDOFWHOIS
    let end = Message::from_server(
        server_name,
        irc::RPL_ENDOFWHOIS,
        vec![my_nick, target_nick, "End of /WHOIS list"],
    );
    send(state, session_id, format!("{end}\r\n"));
}


pub(super) fn handle_who(
    conn: &Connection,
    target: &str,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();

    if target.starts_with('#') || target.starts_with('&') {
        let channel = normalize_channel(target);
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(&channel) {
            let n2s = state.nick_to_session.lock().unwrap();
            let reverse: std::collections::HashMap<&String, &String> =
                n2s.iter().map(|(n, s)| (s, n)).collect();
            let away = state.session_away.lock().unwrap();

            for session in &ch.members {
                if let Some(member_nick) = reverse.get(session) {
                    let user = "~u";
                    let host = "host";
                    let away_flag = if away.contains_key(session) { "G" } else { "H" };
                    let op_flag = if ch.ops.contains(session) { "@" }
                        else if ch.voiced.contains(session) { "+" }
                        else { "" };
                    let flags = format!("{away_flag}{op_flag}");
                    let reply = Message::from_server(
                        server_name,
                        irc::RPL_WHOREPLY,
                        vec![nick, &channel, user, host, server_name, member_nick, &flags, "0 IRC User"],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                }
            }
        }
        let end = Message::from_server(
            server_name,
            irc::RPL_ENDOFWHO,
            vec![nick, &channel, "End of /WHO list"],
        );
        send(state, session_id, format!("{end}\r\n"));
    } else {
        // WHO for a nick
        let target_session = state.nick_to_session.lock().unwrap().get(target).cloned();
        if let Some(ref session) = target_session {
            let away = state.session_away.lock().unwrap();
            let away_flag = if away.contains_key(session) { "G" } else { "H" };
            let reply = Message::from_server(
                server_name,
                irc::RPL_WHOREPLY,
                vec![nick, "*", "~u", "host", server_name, target, away_flag, "0 IRC User"],
            );
            send(state, session_id, format!("{reply}\r\n"));
        }
        let end = Message::from_server(
            server_name,
            irc::RPL_ENDOFWHO,
            vec![nick, target, "End of /WHO list"],
        );
        send(state, session_id, format!("{end}\r\n"));
    }
}

// ── AWAY command ────────────────────────────────────────────────────


pub(super) fn handle_lusers(
    conn: &Connection,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();
    let user_count = state.connections.lock().unwrap().len();
    let channel_count = state.channels.lock().unwrap().len();

    // Count remote users across all channels (deduplicated)
    let remote_count = {
        let channels = state.channels.lock().unwrap();
        let mut remote_nicks = std::collections::HashSet::new();
        for ch in channels.values() {
            for nick in ch.remote_members.keys() {
                remote_nicks.insert(nick.clone());
            }
        }
        remote_nicks.len()
    };

    let total = user_count + remote_count;
    let r1 = Message::from_server(
        server_name,
        irc::RPL_LUSERCLIENT,
        vec![nick, &format!("There are {total} users on 1 server ({remote_count} remote)")],
    );
    let r2 = Message::from_server(
        server_name,
        irc::RPL_LUSEROP,
        vec![nick, "0", "operator(s) online"],
    );
    let r3 = Message::from_server(
        server_name,
        irc::RPL_LUSERCHANNELS,
        vec![nick, &channel_count.to_string(), "channels formed"],
    );
    let r4 = Message::from_server(
        server_name,
        irc::RPL_LUSERME,
        vec![nick, &format!("I have {user_count} clients and 0 servers")],
    );
    for r in [r1, r2, r3, r4] {
        send(state, session_id, format!("{r}\r\n"));
    }
}


pub(super) fn handle_away(
    conn: &Connection,
    away_msg: Option<&str>,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();

    match away_msg {
        Some(msg) if !msg.is_empty() => {
            state.session_away.lock().unwrap()
                .insert(session_id.to_string(), msg.to_string());
            let reply = Message::from_server(
                server_name,
                irc::RPL_NOWAWAY,
                vec![nick, "You have been marked as being away"],
            );
            send(state, session_id, format!("{reply}\r\n"));
        }
        _ => {
            state.session_away.lock().unwrap().remove(session_id);
            let reply = Message::from_server(
                server_name,
                irc::RPL_UNAWAY,
                vec![nick, "You are no longer marked as being away"],
            );
            send(state, session_id, format!("{reply}\r\n"));
        }
    }
}

