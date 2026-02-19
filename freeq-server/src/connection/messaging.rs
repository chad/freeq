#![allow(clippy::too_many_arguments)]
//! Message handling: PRIVMSG, NOTICE, TAGMSG, CHATHISTORY.

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;
use super::helpers::{s2s_broadcast, s2s_next_event_id, normalize_channel};

pub(super) fn handle_tagmsg(
    conn: &Connection,
    target: &str,
    tags: &std::collections::HashMap<String, String>,
    state: &Arc<SharedState>,
) {
    if tags.is_empty() {
        return; // TAGMSG with no tags is meaningless
    }

    let hostmask = conn.hostmask();
    let tag_msg = irc::Message {
        tags: tags.clone(),
        prefix: Some(hostmask.clone()),
        command: "TAGMSG".to_string(),
        params: vec![target.to_string()],
    };
    let tagged_line = format!("{tag_msg}\r\n");

    // Generate a PRIVMSG fallback for plain clients (server-side downgrade).
    // Only for known tag types — unknown TAGMSGs are silently dropped for plain clients.
    let plain_fallback = tags.get("+react").map(|emoji| {
        format!(":{hostmask} PRIVMSG {target} :\x01ACTION reacted with {emoji}\x01\r\n")
    });

    // Rich clients get TAGMSG, plain clients get fallback PRIVMSG (if any)
    if target.starts_with('#') || target.starts_with('&') {
        let members: Vec<String> = state
            .channels.lock().unwrap()
            .get(target)
            .map(|ch| ch.members.iter().cloned().collect())
            .unwrap_or_default();

        let tag_caps = state.cap_message_tags.lock().unwrap();
        let conns = state.connections.lock().unwrap();
        for member_session in &members {
            if member_session != &conn.id
                && let Some(tx) = conns.get(member_session)
            {
                if tag_caps.contains(member_session) {
                    let _ = tx.try_send(tagged_line.clone());
                } else if let Some(ref fallback) = plain_fallback {
                    let _ = tx.try_send(fallback.clone());
                }
            }
        }
    } else {
        let target_session = state.nick_to_session.lock().unwrap().get(target).cloned();
        if let Some(ref session) = target_session
            && let Some(tx) = state.connections.lock().unwrap().get(session)
        {
            if state.cap_message_tags.lock().unwrap().contains(session) {
                let _ = tx.try_send(tagged_line.clone());
            } else if let Some(ref fallback) = plain_fallback {
                let _ = tx.try_send(fallback.clone());
            }
        }
    }
}


pub(super) fn handle_privmsg(
    conn: &Connection,
    command: &str,
    target: &str,
    text: &str,
    tags: &std::collections::HashMap<String, String>,
    state: &Arc<SharedState>,
) {
    let hostmask = conn.hostmask();

    if target.starts_with('#') || target.starts_with('&') {
        // Channel message — enforce +n (no external messages) and +m (moderated)
        {
            let channels = state.channels.lock().unwrap();
            if let Some(ch) = channels.get(target) {
                // +n: must be a member to send
                if ch.no_ext_msg && !ch.members.contains(&conn.id) {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        irc::ERR_CANNOTSENDTOCHAN,
                        vec![nick, target, "Cannot send to channel (+n)"],
                    );
                    if let Some(tx) = state.connections.lock().unwrap().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                    return;
                }
                // +m: must be voiced or op to send
                if ch.moderated
                    && !ch.ops.contains(&conn.id)
                    && !ch.voiced.contains(&conn.id)
                {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        irc::ERR_CANNOTSENDTOCHAN,
                        vec![nick, target, "Cannot send to channel (+m)"],
                    );
                    if let Some(tx) = state.connections.lock().unwrap().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                    return;
                }
            }
        }

        // Run plugin on_message hook
        let msg_event = crate::plugin::MessageEvent {
            nick: conn.nick.clone().unwrap_or_default(),
            command: command.to_string(),
            target: target.to_string(),
            text: text.to_string(),
            did: conn.authenticated_did.clone(),
            session_id: conn.id.clone(),
        };
        let msg_result = state.plugin_manager.on_message(&msg_event);
        if msg_result.suppress {
            return;
        }
        let text = msg_result.rewrite_text.as_deref().unwrap_or(text);

        // Plain line (no tags) for clients that don't support message-tags
        let plain_line = format!(":{hostmask} {command} {target} :{text}\r\n");
        // Tagged line for clients that negotiated message-tags
        let tagged_line = if tags.is_empty() {
            None
        } else {
            let tag_msg = irc::Message {
                tags: tags.clone(),
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            Some(format!("{tag_msg}\r\n"))
        };

        // Store in channel history
        if command == "PRIVMSG" {
            use crate::server::{HistoryMessage, MAX_HISTORY};
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let mut channels = state.channels.lock().unwrap();
            if let Some(ch) = channels.get_mut(target) {
                ch.history.push_back(HistoryMessage {
                    from: hostmask.clone(),
                    text: text.to_string(),
                    timestamp,
                    tags: tags.clone(),
                });
                while ch.history.len() > MAX_HISTORY {
                    ch.history.pop_front();
                }
            }
            drop(channels);
            state.with_db(|db| db.insert_message(target, &hostmask, text, timestamp, tags));

            // Prune old messages if configured
            let max = state.config.max_messages_per_channel;
            if max > 0 {
                state.with_db(|db| db.prune_messages(target, max));
            }
        }

        let members: Vec<String> = state
            .channels
            .lock()
            .unwrap()
            .get(target)
            .map(|ch| ch.members.iter().cloned().collect())
            .unwrap_or_default();

        let tag_caps = state.cap_message_tags.lock().unwrap();
        let echo_caps = state.cap_echo_message.lock().unwrap();
        let conns = state.connections.lock().unwrap();
        for member_session in &members {
            // echo-message: include sender if they requested it
            if member_session == &conn.id && !echo_caps.contains(member_session) {
                continue;
            }
            if let Some(tx) = conns.get(member_session) {
                let line = match (&tagged_line, tag_caps.contains(member_session)) {
                    (Some(tagged), true) => tagged,
                    _ => &plain_line,
                };
                let _ = tx.try_send(line.clone());
            }
        }

        // Broadcast channel PRIVMSG to S2S peers
        if command == "PRIVMSG" {
            let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
            s2s_broadcast(state, crate::s2s::S2sMessage::Privmsg {
                event_id: s2s_next_event_id(state),
                from: conn.nick.as_deref().unwrap_or("*").to_string(),
                target: target.to_string(),
                text: text.to_string(),
                origin,
            });
        }
    } else {
        // Private message — check RPL_AWAY and deliver
        let plain_line = format!(":{hostmask} {command} {target} :{text}\r\n");
        let tagged_line = if tags.is_empty() {
            None
        } else {
            let tag_msg = irc::Message {
                tags: tags.clone(),
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            Some(format!("{tag_msg}\r\n"))
        };

        let target_session = state.nick_to_session.lock().unwrap().get(target).cloned();
        if let Some(ref session) = target_session {
            // Target is local — deliver directly
            // Send RPL_AWAY if target is away
            if let Some(away_msg) = state.session_away.lock().unwrap().get(session) {
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &state.server_name,
                    irc::RPL_AWAY,
                    vec![nick, target, away_msg],
                );
                if let Some(tx) = state.connections.lock().unwrap().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
            }

            let has_tags = state.cap_message_tags.lock().unwrap().contains(session);
            let line = match (&tagged_line, has_tags) {
                (Some(tagged), true) => tagged,
                _ => &plain_line,
            };
            if let Some(tx) = state.connections.lock().unwrap().get(session) {
                let _ = tx.try_send(line.clone());
            }
        } else if command == "PRIVMSG" || command == "NOTICE" {
            // Target is not local — relay to S2S peers if federation is active.
            //
            // We intentionally do NOT gate on remote_members here. The sending
            // server may not have the target in any channel's remote_members
            // (e.g. sync hasn't completed, or no shared channel exists) but the
            // target's home server will deliver if they're connected. Gating on
            // remote_members caused asymmetric PM failures: A→B works but B→A
            // gets ERR_NOSUCHNICK if B's server hasn't synced A's presence yet.
            let has_s2s = state.s2s_manager.lock().unwrap().is_some();
            if has_s2s {
                let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
                s2s_broadcast(state, crate::s2s::S2sMessage::Privmsg {
                    event_id: s2s_next_event_id(state),
                    from: conn.nick.as_deref().unwrap_or("*").to_string(),
                    target: target.to_string(),
                    text: text.to_string(),
                    origin,
                });
            } else {
                // No federation — target truly doesn't exist
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &state.server_name,
                    irc::ERR_NOSUCHNICK,
                    vec![nick, target, "No such nick/channel"],
                );
                if let Some(tx) = state.connections.lock().unwrap().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
            }
        }
    }
}

// ── LIST command ────────────────────────────────────────────────────


fn parse_chathistory_ts(s: &str) -> Option<u64> {
    let s = s.strip_prefix("timestamp=").unwrap_or(s);
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp() as u64)
}


pub(super) fn handle_chathistory(
    conn: &Connection,
    msg: &irc::Message,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &dyn Fn(&Arc<SharedState>, &str, String),
) {
    let _nick = conn.nick_or_star();

    // CHATHISTORY <subcommand> <target> [<param1> [<param2>]] <limit>
    if msg.params.len() < 3 {
        let reply = Message::from_server(
            server_name,
            "FAIL",
            vec!["CHATHISTORY", "NEED_MORE_PARAMS", "Insufficient parameters"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    let subcmd = msg.params[0].to_uppercase();
    let target = normalize_channel(&msg.params[1]);

    // Verify user is a member of the channel
    {
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(&target) {
            if !ch.members.contains(session_id) {
                let reply = Message::from_server(
                    server_name,
                    "FAIL",
                    vec!["CHATHISTORY", "INVALID_TARGET", &target, "You are not in that channel"],
                );
                send(state, session_id, format!("{reply}\r\n"));
                return;
            }
        } else {
            let reply = Message::from_server(
                server_name,
                "FAIL",
                vec!["CHATHISTORY", "INVALID_TARGET", &target, "No such channel"],
            );
            send(state, session_id, format!("{reply}\r\n"));
            return;
        }
    }

    let has_tags = state.cap_message_tags.lock().unwrap().contains(session_id);
    let has_time = state.cap_server_time.lock().unwrap().contains(session_id);
    let has_batch = state.cap_batch.lock().unwrap().contains(session_id);

    // Fetch messages from DB based on subcommand
    let messages: Vec<crate::db::MessageRow> = match subcmd.as_str() {
        "BEFORE" => {
            if msg.params.len() < 4 { vec![] } else {
                let ts = parse_chathistory_ts(&msg.params[2]).unwrap_or(u64::MAX);
                let limit = msg.params[3].parse::<usize>().unwrap_or(50).min(500);
                state.with_db(|db| db.get_messages(&target, limit, Some(ts)))
                    
                    .unwrap_or_default()
            }
        }
        "AFTER" => {
            if msg.params.len() < 4 { vec![] } else {
                let ts = parse_chathistory_ts(&msg.params[2]).unwrap_or(0);
                let limit = msg.params[3].parse::<usize>().unwrap_or(50).min(500);
                state.with_db(|db| db.get_messages_after(&target, ts, limit))
                    
                    .unwrap_or_default()
            }
        }
        "LATEST" => {
            if msg.params.len() < 4 { vec![] } else {
                let limit = msg.params[3].parse::<usize>().unwrap_or(50).min(500);
                if msg.params[2] == "*" {
                    state.with_db(|db| db.get_messages(&target, limit, None))
                        
                        .unwrap_or_default()
                } else {
                    let ts = parse_chathistory_ts(&msg.params[2]).unwrap_or(0);
                    state.with_db(|db| db.get_messages_after(&target, ts, limit))
                        
                        .unwrap_or_default()
                }
            }
        }
        "BETWEEN" => {
            if msg.params.len() < 5 { vec![] } else {
                let start = parse_chathistory_ts(&msg.params[2]).unwrap_or(0);
                let end = parse_chathistory_ts(&msg.params[3]).unwrap_or(u64::MAX);
                let limit = msg.params[4].parse::<usize>().unwrap_or(50).min(500);
                state.with_db(|db| db.get_messages_between(&target, start, end, limit))
                    
                    .unwrap_or_default()
            }
        }
        _ => vec![],
    };

    // Send as a batch
    let batch_id = format!("ch{}", session_id.len());
    if has_batch {
        send(state, session_id, format!(
            ":{server_name} BATCH +{batch_id} chathistory {target}\r\n"
        ));
    }

    for row in &messages {
        let mut tags = if has_tags { row.tags.clone() } else { std::collections::HashMap::new() };
        if has_time {
            let ts = chrono::DateTime::from_timestamp(row.timestamp as i64, 0)
                .unwrap_or_default()
                .format("%Y-%m-%dT%H:%M:%S.000Z")
                .to_string();
            tags.insert("time".to_string(), ts);
        }
        if has_batch {
            tags.insert("batch".to_string(), batch_id.clone());
        }

        if !tags.is_empty() && has_tags {
            let tag_msg = irc::Message {
                tags,
                prefix: Some(row.sender.clone()),
                command: "PRIVMSG".to_string(),
                params: vec![target.clone(), row.text.clone()],
            };
            send(state, session_id, format!("{tag_msg}\r\n"));
        } else {
            send(state, session_id, format!(
                ":{} PRIVMSG {} :{}\r\n", row.sender, target, row.text
            ));
        }
    }

    if has_batch {
        send(state, session_id, format!(
            ":{server_name} BATCH -{batch_id}\r\n"
        ));
    }
}

