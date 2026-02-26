#![allow(clippy::too_many_arguments)]
//! Message handling: PRIVMSG, NOTICE, TAGMSG, CHATHISTORY.

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;
use super::helpers::{s2s_broadcast, s2s_next_event_id, normalize_channel};

/// Verify a client-provided signature, or server-sign as fallback.
///
/// If the client included `+freeq.at/sig` AND has a registered session key,
/// verify it. If valid, return the client's signature (true non-repudiation).
/// If the client didn't sign but is DID-authenticated, server-sign as fallback.
/// Returns None for guests.
///
/// Canonical form: `{sender_did}\0{target}\0{text}\0{timestamp}`
fn resolve_signature(
    conn: &Connection,
    target: &str,
    text: &str,
    timestamp: u64,
    client_sig: Option<&str>,
    state: &Arc<SharedState>,
) -> Option<String> {
    let did = conn.authenticated_did.as_ref()?;
    let canonical = format!("{did}\0{target}\0{text}\0{timestamp}");

    // If client provided a signature, verify it against their registered key
    if let Some(sig_b64) = client_sig {
        if let Some(vk) = state.session_msg_keys.lock().get(&conn.id).cloned() {
            use base64::Engine;
            if let Ok(sig_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64) {
                if sig_bytes.len() == 64 {
                    let sig_array: [u8; 64] = sig_bytes.try_into().unwrap();
                    let sig = ed25519_dalek::Signature::from_bytes(&sig_array);
                    use ed25519_dalek::Verifier;
                    if vk.verify(canonical.as_bytes(), &sig).is_ok() {
                        // Client signature valid — use it (true non-repudiation)
                        return Some(sig_b64.to_string());
                    } else {
                        tracing::warn!(
                            session = %conn.id,
                            did = %did,
                            "Client message signature verification failed — falling back to server signing"
                        );
                    }
                }
            }
        }
    }

    // Fallback: server signs on behalf of authenticated user
    use ed25519_dalek::Signer;
    let sig = state.msg_signing_key.sign(canonical.as_bytes());
    use base64::Engine;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes()))
}

pub(super) fn handle_tagmsg(
    conn: &Connection,
    target: &str,
    tags: &std::collections::HashMap<String, String>,
    state: &Arc<SharedState>,
) {
    if tags.is_empty() {
        return; // TAGMSG with no tags is meaningless
    }

    // ── Message deletion (+draft/delete=<msgid>) ──
    if let Some(original_msgid) = tags.get("+draft/delete") {
        handle_delete(conn, target, original_msgid, state);
        return;
    }

    let hostmask = conn.hostmask();

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let time_tag = chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S.000Z")
        .to_string();

    let tag_msg = irc::Message {
        tags: tags.clone(),
        prefix: Some(hostmask.clone()),
        command: "TAGMSG".to_string(),
        params: vec![target.to_string()],
    };
    let tagged_line = format!("{tag_msg}\r\n");

    let mut tags_with_time = tags.clone();
    tags_with_time.insert("time".to_string(), time_tag);
    let tag_msg_with_time = irc::Message {
        tags: tags_with_time,
        prefix: Some(hostmask.clone()),
        command: "TAGMSG".to_string(),
        params: vec![target.to_string()],
    };
    let tagged_line_with_time = format!("{tag_msg_with_time}\r\n");

    // Generate a PRIVMSG fallback for plain clients (server-side downgrade).
    // Only for known tag types — unknown TAGMSGs are silently dropped for plain clients.
    let plain_fallback = tags.get("+react").map(|emoji| {
        format!(":{hostmask} PRIVMSG {target} :\x01ACTION reacted with {emoji}\x01\r\n")
    });

    // Rich clients get TAGMSG, plain clients get fallback PRIVMSG (if any)
    if target.starts_with('#') || target.starts_with('&') {
        let members: Vec<String> = state
            .channels.lock()
            .get(target)
            .map(|ch| ch.members.iter().cloned().collect())
            .unwrap_or_default();

        let tag_caps = state.cap_message_tags.lock();
        let time_caps = state.cap_server_time.lock();
        let echo_caps = state.cap_echo_message.lock();
        let conns = state.connections.lock();
        for member_session in &members {
            // Skip sender unless they have echo-message
            if member_session == &conn.id && !echo_caps.contains(member_session) {
                continue;
            }
            if let Some(tx) = conns.get(member_session) {
                if tag_caps.contains(member_session) {
                    let line = if time_caps.contains(member_session) {
                        &tagged_line_with_time
                    } else {
                        &tagged_line
                    };
                    let _ = tx.try_send(line.clone());
                } else if let Some(ref fallback) = plain_fallback {
                    let _ = tx.try_send(fallback.clone());
                }
            }
        }
    } else {
        // TAGMSG to a nick — route through federation layer.
        use super::routing::{relay_to_nick, RouteResult};
        // TAGMSG uses the same relay path as PRIVMSG.
        // The text payload is empty for TAGMSG; tags ride in the from-line.
        let from_nick = conn.nick.as_deref().unwrap_or("*").to_string();
        let tag_text = plain_fallback.as_deref().unwrap_or("").to_string();
        match relay_to_nick(state, &from_nick, target, &tag_text, super::helpers::s2s_next_event_id(state)) {
            RouteResult::Local(ref session) => {
                if let Some(tx) = state.connections.lock().get(session) {
                    let has_tags = state.cap_message_tags.lock().contains(session);
                    let has_time = state.cap_server_time.lock().contains(session);
                    if has_tags {
                        let line = if has_time { &tagged_line_with_time } else { &tagged_line };
                        let _ = tx.try_send(line.clone());
                    } else if let Some(ref fallback) = plain_fallback {
                        let _ = tx.try_send(fallback.clone());
                    }
                }
            }
            RouteResult::Relayed | RouteResult::Unreachable => {
                // TAGMSG to remote user — best-effort relay (or silently dropped).
                // No error sent: TAGMSG has no delivery expectation.
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

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let time_tag = chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S.000Z")
        .to_string();

    // ── Message editing (+draft/edit=<msgid>) ──
    if let Some(original_msgid) = tags.get("+draft/edit") {
        handle_edit(conn, target, text, original_msgid, tags, state);
        return;
    }

    if target.starts_with('#') || target.starts_with('&') {
        // Per-session channel flood protection: max 5 messages per 2 seconds
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let mut ts_map = state.msg_timestamps.lock();
            let ts = ts_map.entry(conn.id.clone()).or_default();
            // Prune timestamps older than 2 seconds
            ts.retain(|&t| now.saturating_sub(t) < 2000);
            if ts.len() >= 5 {
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &state.server_name,
                    irc::ERR_CANNOTSENDTOCHAN,
                    vec![nick, target, "Flood protection: sending too fast"],
                );
                if let Some(tx) = state.connections.lock().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
                return;
            }
            ts.push(now);
        }

        // Channel message — enforce +n (no external messages) and +m (moderated)
        {
            let channels = state.channels.lock();
            if let Some(ch) = channels.get(target) {
                // +n: must be a member to send
                if ch.no_ext_msg && !ch.members.contains(&conn.id) {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        irc::ERR_CANNOTSENDTOCHAN,
                        vec![nick, target, "Cannot send to channel (+n)"],
                    );
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                    return;
                }
                // +m: must be voiced or op to send
                if ch.moderated
                    && !ch.ops.contains(&conn.id)
                    && !ch.halfops.contains(&conn.id)
                    && !ch.voiced.contains(&conn.id)
                {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        irc::ERR_CANNOTSENDTOCHAN,
                        vec![nick, target, "Cannot send to channel (+m)"],
                    );
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                    return;
                }
                // +E: encrypted-only mode
                if ch.encrypted_only && !tags.contains_key("+encrypted") {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        irc::ERR_CANNOTSENDTOCHAN,
                        vec![nick, target, "Cannot send to channel (+E) — messages must be encrypted"],
                    );
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
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

        // Generate msgid for every PRIVMSG/NOTICE
        let msgid = crate::msgid::generate();

        // Build tags with msgid injected (for tag-capable clients)
        let mut full_tags = tags.clone();
        full_tags.insert("msgid".to_string(), msgid.clone());

        // Verify client signature or server-sign as fallback
        let client_sig = tags.get("+freeq.at/sig").map(|s| s.as_str());
        if let Some(sig) = resolve_signature(conn, target, text, timestamp, client_sig, state) {
            full_tags.insert("+freeq.at/sig".to_string(), sig);
        }

        let mut full_tags_with_time = full_tags.clone();
        full_tags_with_time.insert("time".to_string(), time_tag.clone());

        // Plain line (no tags) for clients that don't support message-tags
        let plain_line = format!(":{hostmask} {command} {target} :{text}\r\n");
        // Tagged line for clients that negotiated message-tags (no server-time)
        let tagged_line = {
            let tag_msg = irc::Message {
                tags: full_tags.clone(),
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            format!("{tag_msg}\r\n")
        };
        // Tagged line with server-time
        let tagged_line_with_time = {
            let tag_msg = irc::Message {
                tags: full_tags_with_time.clone(),
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            format!("{tag_msg}\r\n")
        };

        // Store in channel history
        if command == "PRIVMSG" {
            use crate::server::{HistoryMessage, MAX_HISTORY};
            let mut channels = state.channels.lock();
            if let Some(ch) = channels.get_mut(target) {
                ch.history.push_back(HistoryMessage {
                    from: hostmask.clone(),
                    text: text.to_string(),
                    timestamp,
                    tags: full_tags.clone(),
                    msgid: Some(msgid.clone()),
                });
                while ch.history.len() > MAX_HISTORY {
                    ch.history.pop_front();
                }
            }
            drop(channels);
            state.with_db(|db| db.insert_message(target, &hostmask, text, timestamp, tags, Some(&msgid)));

            // Prune old messages if configured
            let max = state.config.max_messages_per_channel;
            if max > 0 {
                state.with_db(|db| db.prune_messages(target, max));
            }
        }

        let members: Vec<String> = state
            .channels
            .lock()
            .get(target)
            .map(|ch| ch.members.iter().cloned().collect())
            .unwrap_or_default();

        let tag_caps = state.cap_message_tags.lock();
        let time_caps = state.cap_server_time.lock();
        let echo_caps = state.cap_echo_message.lock();
        let conns = state.connections.lock();
        for member_session in &members {
            // echo-message: include sender if they requested it
            if member_session == &conn.id && !echo_caps.contains(member_session) {
                continue;
            }
            if let Some(tx) = conns.get(member_session) {
                let line = if tag_caps.contains(member_session) {
                    if time_caps.contains(member_session) {
                        &tagged_line_with_time
                    } else {
                        &tagged_line
                    }
                } else {
                    &plain_line
                };
                let _ = tx.try_send(line.clone());
            }
        }

        // Broadcast channel PRIVMSG to S2S peers
        if command == "PRIVMSG" {
            let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
            let sig = full_tags.get("+freeq.at/sig").cloned();
            s2s_broadcast(state, crate::s2s::S2sMessage::Privmsg {
                event_id: s2s_next_event_id(state),
                from: conn.nick.as_deref().unwrap_or("*").to_string(),
                target: target.to_string(),
                text: text.to_string(),
                origin,
                msgid: Some(msgid.clone()),
                sig,
            });
        }
    } else {
        // Private message — check RPL_AWAY and deliver
        let pm_msgid = crate::msgid::generate();
        let mut pm_tags = tags.clone();
        pm_tags.insert("msgid".to_string(), pm_msgid.clone());

        // Verify client signature or server-sign DMs
        let client_sig = tags.get("+freeq.at/sig").map(|s| s.as_str());
        if let Some(sig) = resolve_signature(conn, target, text, timestamp, client_sig, state) {
            pm_tags.insert("+freeq.at/sig".to_string(), sig);
        }

        let mut pm_tags_with_time = pm_tags.clone();
        pm_tags_with_time.insert("time".to_string(), time_tag.clone());

        let plain_line = format!(":{hostmask} {command} {target} :{text}\r\n");
        let tagged_line = {
            let tag_msg = irc::Message {
                tags: pm_tags.clone(),
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            format!("{tag_msg}\r\n")
        };
        let tagged_line_with_time = {
            let tag_msg = irc::Message {
                tags: pm_tags_with_time,
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            format!("{tag_msg}\r\n")
        };

        // Route through the federation routing layer.
        // See routing.rs for why we NEVER gate on remote_members here.
        use super::routing::{relay_to_nick, RouteResult};
        let from_nick = conn.nick.as_deref().unwrap_or("*").to_string();
        match relay_to_nick(state, &from_nick, target, text, s2s_next_event_id(state)) {
            RouteResult::Local(ref session) => {
                // Target is local — deliver to ALL sessions for target's DID (multi-device)
                // Send RPL_AWAY if target is away
                if let Some(away_msg) = state.session_away.lock().get(session) {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        irc::RPL_AWAY,
                        vec![nick, target, away_msg],
                    );
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                }

                // Find all sessions for target's DID
                let target_sessions: Vec<String> = {
                    let target_did = state.session_dids.lock().get(session).cloned();
                    if let Some(ref did) = target_did {
                        state.did_sessions.lock().get(did)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_else(|| vec![session.clone()])
                    } else {
                        vec![session.clone()] // Guest — single session
                    }
                };

                let conns = state.connections.lock();
                // Deliver to all target sessions
                for target_session in &target_sessions {
                    let has_tags = state.cap_message_tags.lock().contains(target_session);
                    let has_time = state.cap_server_time.lock().contains(target_session);
                    let line = if has_tags {
                        if has_time { &tagged_line_with_time } else { &tagged_line }
                    } else { &plain_line };
                    if let Some(tx) = conns.get(target_session) {
                        let _ = tx.try_send(line.clone());
                    }
                }

                // echo-message: echo DM back to ALL sender's sessions
                let sender_sessions: Vec<String> = {
                    if let Some(ref did) = conn.authenticated_did {
                        state.did_sessions.lock().get(did)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_else(|| vec![conn.id.clone()])
                    } else {
                        vec![conn.id.clone()]
                    }
                };
                for sender_session in &sender_sessions {
                    if sender_session == &conn.id {
                        // Original sender — use echo-message cap
                        let sender_has_echo = state.cap_echo_message.lock().contains(&conn.id);
                        if sender_has_echo {
                            let has_tags = state.cap_message_tags.lock().contains(&conn.id);
                            let has_time = state.cap_server_time.lock().contains(&conn.id);
                            let echo_line = if has_tags {
                                if has_time { &tagged_line_with_time } else { &tagged_line }
                            } else { &plain_line };
                            if let Some(tx) = conns.get(&conn.id) {
                                let _ = tx.try_send(echo_line.clone());
                            }
                        }
                    } else {
                        // Other sessions of sender — deliver as if they received it
                        let has_tags = state.cap_message_tags.lock().contains(sender_session);
                        let has_time = state.cap_server_time.lock().contains(sender_session);
                        let line = if has_tags {
                            if has_time { &tagged_line_with_time } else { &tagged_line }
                        } else { &plain_line };
                        if let Some(tx) = conns.get(sender_session) {
                            let _ = tx.try_send(line.clone());
                        }
                    }
                }
            }
            RouteResult::Relayed => {
                // Sent to S2S peers — receiving server will deliver.
                // No ERR_NOSUCHNICK: we can't know if it arrived (same as email).
                // echo-message: echo DM back to sender even for relayed messages
                let sender_has_echo = state.cap_echo_message.lock().contains(&conn.id);
                if sender_has_echo {
                    let sender_has_tags = state.cap_message_tags.lock().contains(&conn.id);
                    let sender_has_time = state.cap_server_time.lock().contains(&conn.id);
                    let echo_line = if sender_has_tags {
                        if sender_has_time { &tagged_line_with_time } else { &tagged_line }
                    } else { &plain_line };
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(echo_line.clone());
                    }
                }
            }
            RouteResult::Unreachable => {
                // No federation, nick doesn't exist locally
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &state.server_name,
                    irc::ERR_NOSUCHNICK,
                    vec![nick, target, "No such nick/channel"],
                );
                if let Some(tx) = state.connections.lock().get(&conn.id) {
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
        let channels = state.channels.lock();
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

    let has_tags = state.cap_message_tags.lock().contains(session_id);
    let has_time = state.cap_server_time.lock().contains(session_id);
    let has_batch = state.cap_batch.lock().contains(session_id);

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

    // Send as a batch (unique ID per request)
    let batch_id = format!("ch{}", crate::msgid::generate());
    if has_batch {
        send(state, session_id, format!(
            ":{server_name} BATCH +{batch_id} chathistory {target}\r\n"
        ));
    }

    for row in &messages {
        let mut tags = if has_tags { row.tags.clone() } else { std::collections::HashMap::new() };
        // Include msgid if available
        if has_tags {
            if let Some(ref mid) = row.msgid {
                tags.insert("msgid".to_string(), mid.clone());
            }
            if let Some(ref replaces) = row.replaces_msgid {
                tags.entry("+draft/edit".to_string()).or_insert_with(|| replaces.clone());
            }
        }
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


// ── Message editing ─────────────────────────────────────────────────

/// Handle a PRIVMSG with +draft/edit=<msgid> tag.
/// Verifies authorship, stores the edit, and broadcasts to channel.
fn handle_edit(
    conn: &Connection,
    target: &str,
    new_text: &str,
    original_msgid: &str,
    tags: &std::collections::HashMap<String, String>,
    state: &Arc<SharedState>,
) {
    let hostmask = conn.hostmask();
    let nick = conn.nick_or_star();

    // Only channel messages can be edited (for now)
    if !target.starts_with('#') && !target.starts_with('&') {
        return;
    }

    // Verify authorship: look up original message by msgid
    let original = state.with_db(|db| db.get_message_by_msgid(target, original_msgid));
    match original {
        Some(Some(row)) => {
            // Extract nick from the stored sender hostmask
            let original_nick = row.sender.split('!').next().unwrap_or("");
            let current_nick = nick;
            if !original_nick.eq_ignore_ascii_case(current_nick) {
                let reply = Message::from_server(
                    &state.server_name,
                    "FAIL",
                    vec!["EDIT", "AUTHOR_MISMATCH", "You can only edit your own messages"],
                );
                if let Some(tx) = state.connections.lock().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
                return;
            }
            if row.deleted_at.is_some() {
                return; // Can't edit a deleted message
            }
        }
        _ => {
            // Message not found (no DB, pruned, or wrong msgid) — reject
            let reply = Message::from_server(
                &state.server_name,
                "FAIL",
                vec!["EDIT", "MESSAGE_NOT_FOUND", "Original message not found"],
            );
            if let Some(tx) = state.connections.lock().get(&conn.id) {
                let _ = tx.try_send(format!("{reply}\r\n"));
            }
            return;
        }
    }

    // Generate new msgid for the edit
    let edit_msgid = crate::msgid::generate();

    // Build tags with edit reference + new msgid
    let mut full_tags = tags.clone();
    full_tags.insert("msgid".to_string(), edit_msgid.clone());
    // Keep the +draft/edit tag so clients know this is an edit

    // Verify/sign edited message
    let edit_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let client_sig = tags.get("+freeq.at/sig").map(|s| s.as_str());
    if let Some(sig) = resolve_signature(conn, target, new_text, edit_timestamp, client_sig, state) {
        full_tags.insert("+freeq.at/sig".to_string(), sig);
    }

    // Plain line for non-tag clients (they see it as a new message)
    let plain_line = format!(":{hostmask} PRIVMSG {target} :{new_text}\r\n");
    // Tagged line with edit reference
    let tagged_line = {
        let tag_msg = irc::Message {
            tags: full_tags.clone(),
            prefix: Some(hostmask.clone()),
            command: "PRIVMSG".to_string(),
            params: vec![target.to_string(), new_text.to_string()],
        };
        format!("{tag_msg}\r\n")
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let time_tag = chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S.000Z")
        .to_string();
    let mut full_tags_with_time = full_tags.clone();
    full_tags_with_time.insert("time".to_string(), time_tag);
    let tagged_line_with_time = {
        let tag_msg = irc::Message {
            tags: full_tags_with_time,
            prefix: Some(hostmask.clone()),
            command: "PRIVMSG".to_string(),
            params: vec![target.to_string(), new_text.to_string()],
        };
        format!("{tag_msg}\r\n")
    };

    // Store in DB
    let store_tags: std::collections::HashMap<String, String> = tags.iter()
        .filter(|(k, _)| *k != "msgid")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    state.with_db(|db| db.insert_edit(target, &hostmask, new_text, timestamp, &store_tags, &edit_msgid, original_msgid));

    // Update in-memory history: replace the original message text
    {
        let mut channels = state.channels.lock();
        if let Some(ch) = channels.get_mut(target) {
            // Find and update the original message in history
            for hist in ch.history.iter_mut() {
                if hist.msgid.as_deref() == Some(original_msgid) {
                    hist.text = new_text.to_string();
                    hist.tags.insert("msgid".to_string(), edit_msgid.clone());
                    break;
                }
            }
        }
    }

    // Deliver to channel members
    let members: Vec<String> = state
        .channels.lock()
        .get(target)
        .map(|ch| ch.members.iter().cloned().collect())
        .unwrap_or_default();

    let tag_caps = state.cap_message_tags.lock();
    let time_caps = state.cap_server_time.lock();
    let echo_caps = state.cap_echo_message.lock();
    let conns = state.connections.lock();
    for sid in &members {
        if sid == &conn.id && !echo_caps.contains(sid) {
            continue;
        }
        if let Some(tx) = conns.get(sid) {
            let line = if tag_caps.contains(sid) {
                if time_caps.contains(sid) { &tagged_line_with_time } else { &tagged_line }
            } else { &plain_line };
            let _ = tx.try_send(line.clone());
        }
    }

    // Broadcast to S2S peers
    let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
    let sig = full_tags.get("+freeq.at/sig").cloned();
    s2s_broadcast(state, crate::s2s::S2sMessage::Privmsg {
        event_id: s2s_next_event_id(state),
        from: nick.to_string(),
        target: target.to_string(),
        text: new_text.to_string(),
        origin,
        msgid: Some(edit_msgid),
        sig,
    });
}

// ── Message deletion ────────────────────────────────────────────────

/// Handle a TAGMSG with +draft/delete=<msgid> tag.
/// Verifies authorship, soft-deletes the message, broadcasts to channel.
fn handle_delete(
    conn: &Connection,
    target: &str,
    original_msgid: &str,
    state: &Arc<SharedState>,
) {
    let hostmask = conn.hostmask();
    let nick = conn.nick_or_star();

    // Only channel messages can be deleted (for now)
    if !target.starts_with('#') && !target.starts_with('&') {
        return;
    }

    // Verify authorship
    let original = state.with_db(|db| db.get_message_by_msgid(target, original_msgid));
    match original {
        Some(Some(row)) => {
            let original_nick = row.sender.split('!').next().unwrap_or("");
            let current_nick = nick;
            if !original_nick.eq_ignore_ascii_case(current_nick) {
                // Also allow ops to delete messages
                let is_op = state.channels.lock()
                    .get(target)
                    .map(|ch| ch.ops.contains(&conn.id))
                    .unwrap_or(false);
                if !is_op {
                    let reply = Message::from_server(
                        &state.server_name,
                        "FAIL",
                        vec!["DELETE", "AUTHOR_MISMATCH", "You can only delete your own messages"],
                    );
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                    return;
                }
            }
            if row.deleted_at.is_some() {
                return; // Already deleted
            }
        }
        _ => {
            // Message not found (no DB, pruned, or wrong msgid) — reject
            let reply = Message::from_server(
                &state.server_name,
                "FAIL",
                vec!["DELETE", "MESSAGE_NOT_FOUND", "Original message not found"],
            );
            if let Some(tx) = state.connections.lock().get(&conn.id) {
                let _ = tx.try_send(format!("{reply}\r\n"));
            }
            return;
        }
    }

    // Soft-delete in DB
    state.with_db(|db| db.soft_delete_message(target, original_msgid));

    // Remove from in-memory history
    {
        let mut channels = state.channels.lock();
        if let Some(ch) = channels.get_mut(target) {
            ch.history.retain(|h| h.msgid.as_deref() != Some(original_msgid));
        }
    }

    // Build TAGMSG with +draft/delete for tag-capable clients
    let mut del_tags = std::collections::HashMap::new();
    del_tags.insert("+draft/delete".to_string(), original_msgid.to_string());
    let tagged_line = {
        let tag_msg = irc::Message {
            tags: del_tags,
            prefix: Some(hostmask.clone()),
            command: "TAGMSG".to_string(),
            params: vec![target.to_string()],
        };
        format!("{tag_msg}\r\n")
    };

    // Deliver to tag-capable channel members only (plain clients can't see deletes)
    let members: Vec<String> = state
        .channels.lock()
        .get(target)
        .map(|ch| ch.members.iter().cloned().collect())
        .unwrap_or_default();

    let tag_caps = state.cap_message_tags.lock();
    let conns = state.connections.lock();
    for sid in &members {
        if sid == &conn.id {
            continue; // Don't echo delete back to sender
        }
        if tag_caps.contains(sid) {
            if let Some(tx) = conns.get(sid) {
                let _ = tx.try_send(tagged_line.clone());
            }
        }
    }
}
