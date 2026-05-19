#![allow(clippy::too_many_arguments)]
//! Message handling: PRIVMSG, NOTICE, TAGMSG, CHATHISTORY.

use super::Connection;
use super::helpers::{normalize_channel, s2s_broadcast, s2s_next_event_id};
use crate::irc::{self, Message};
use crate::server::SharedState;
use std::sync::Arc;

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
    if let Some(sig_b64) = client_sig
        && let Some(vk) = state.session_msg_keys.lock().get(&conn.id).cloned()
    {
        use base64::Engine;
        if let Ok(sig_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64)
            && sig_bytes.len() == 64
        {
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

    // Normalize IRCv3 draft tags to their canonical forms so all downstream
    // code (persistence, relay, fallback) only needs to check one name.
    let mut tags = tags.clone();
    for (draft, canonical) in [
        ("+draft/react", "+react"),
        ("+draft/reply", "+reply"),
    ] {
        if let Some(v) = tags.remove(draft) {
            tags.entry(canonical.to_string()).or_insert(v);
        }
    }
    let tags = &tags;

    // ── Message deletion (+draft/delete=<msgid>) ──
    if let Some(original_msgid) = tags.get("+draft/delete") {
        handle_delete(conn, target, original_msgid, state);
        return;
    }

    // ── Coordination event storage (+freeq.at/event) ──
    if let Some(event_type) = tags.get("+freeq.at/event") {
        if let Some(ref did) = conn.authenticated_did {
            // SECURITY (CTF-20): rate-limit event storage per session.
            // Previously TAGMSG had no flood protection, so an
            // authenticated user could spam hundreds of event TAGMSGs
            // per second to fill the DB. Cap at 5 events / 2s, same
            // window as PRIVMSG flood protection.
            //
            // Reuses msg_timestamps under a session-derived synthetic
            // key so this counter is independent of the PRIVMSG one.
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let key = format!("event:{}", conn.id);
                let mut ts_map = state.msg_timestamps.lock();
                let ts = ts_map.entry(key).or_default();
                ts.retain(|&t| now.saturating_sub(t) < 2000);
                if ts.len() >= 5 {
                    let nick = conn.nick_or_star();
                    let reply = Message::from_server(
                        &state.server_name,
                        "FAIL",
                        vec![
                            "TAGMSG",
                            "RATE_LIMITED",
                            "event-storage TAGMSG flood: 5 events / 2s per session",
                        ],
                    );
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(format!("{reply}\r\n"));
                    }
                    tracing::warn!(
                        actor = %did, nick = %nick,
                        "Rate-limited coordination-event TAGMSG flood",
                    );
                    return;
                }
                ts.push(now);
            }
            // SECURITY (CTF-20 cont.): also cap payload size before
            // decoding + storing. The 8 KB IRC line cap already bounds
            // each payload, but the explicit cap here is defense in
            // depth — and lets us return a clean FAIL instead of a
            // silent truncation.
            const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
            let raw_payload = tags
                .get("+freeq.at/payload")
                .map(String::as_str)
                .unwrap_or("");
            if raw_payload.len() > MAX_PAYLOAD_BYTES {
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &state.server_name,
                    "FAIL",
                    vec![
                        "TAGMSG",
                        "PAYLOAD_TOO_LARGE",
                        &format!(
                            "+freeq.at/payload exceeds {MAX_PAYLOAD_BYTES} bytes; got {}",
                            raw_payload.len()
                        ),
                    ],
                );
                if let Some(tx) = state.connections.lock().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
                tracing::warn!(
                    actor = %did, nick = %nick, size = raw_payload.len(),
                    "Refused oversized coordination event payload",
                );
                return;
            }
            let event_id = tags.get("msgid")
                .cloned()
                .unwrap_or_else(|| crate::msgid::generate());
            let ref_id = tags.get("+freeq.at/ref")
                .or_else(|| tags.get("+freeq.at/task-id"))
                .cloned();
            let payload = if raw_payload.is_empty() {
                "{}".to_string()
            } else {
                urlencoding::decode(raw_payload)
                    .unwrap_or_else(|_| raw_payload.into())
                    .into_owned()
            };
            // Re-check after decoding: percent-decoding can expand by
            // up to ~3x if the input was all `%xx`, so even a
            // payload that fit before decoding may exceed the cap
            // after.
            if payload.len() > MAX_PAYLOAD_BYTES {
                tracing::warn!(actor = %did, "Decoded payload exceeded cap; dropping");
                return;
            }
            let signature = tags.get("+freeq.at/sig").cloned();
            let now = chrono::Utc::now().timestamp();
            let event = crate::db::CoordinationEventRow {
                event_id: event_id.clone(),
                event_type: event_type.clone(),
                actor_did: did.clone(),
                channel: target.to_string(),
                ref_id,
                payload_json: payload,
                signature,
                timestamp: now,
            };
            state.with_db(|db| db.store_coordination_event(&event));
            tracing::debug!(
                event_type = %event_type,
                event_id = %event_id,
                actor = %did,
                channel = %target,
                "Stored coordination event"
            );
        }
    }

    // Log av-signal relay for debugging
    if tags.contains_key("+freeq.at/av-signal") {
        tracing::info!(
            from = %conn.nick_or_star(),
            target = %target,
            "Relaying WebRTC signal TAGMSG"
        );
    }

    // ── AV session control (+freeq.at/av-*) ──
    //
    // The dispatch key must be the *action* tag (av-start / av-join /
    // av-leave / av-end), never a parameter (av-id, av-instance,
    // av-title, …). Previously we grabbed `tags.keys().find(...)` on
    // anything starting with `+freeq.at/av-` and HashMap iteration
    // order picked whichever tag happened to hash first — so an
    // av-join TAGMSG that also carried av-id and av-instance would
    // sometimes dispatch under av-id, fall into the `_ => debug!` arm,
    // and silently do nothing. That was the "av-join succeeds for one
    // device but not the other" bug.
    //
    // av-signal / av-chunk are relay tags (WebRTC signalling / data
    // chunks) — must be forwarded, not consumed.
    const AV_ACTIONS: &[&str] = &[
        "+freeq.at/av-start",
        "+freeq.at/av-join",
        "+freeq.at/av-leave",
        "+freeq.at/av-end",
    ];
    if let Some(action) = AV_ACTIONS.iter().find(|tag| tags.contains_key(**tag)) {
        handle_av_tagmsg(conn, target, tags, action, state);
        return; // AV control tags are consumed server-side; don't relay
    }

    // ── Persist reactions (+react with +reply) ──
    if let (Some(emoji), Some(target_msgid)) = (tags.get("+react"), tags.get("+reply")) {
        let nick = conn.nick_or_star().to_string();
        let did = conn.authenticated_did.clone();
        let channel = target.to_string();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let emoji = emoji.clone();
        let target_msgid = target_msgid.clone();
        state.with_db(|db| db.store_reaction(&target_msgid, &channel, &nick, did.as_deref(), &emoji, ts));
    }

    // ── Remove reactions (+freeq.at/unreact with +reply) ──
    // The reactor is identified by the connection's current nick — same key the
    // add path uses to scope a reaction. The TAGMSG itself still relays through
    // the broadcast below so other clients can drop the pill from the UI.
    if let (Some(emoji), Some(target_msgid)) =
        (tags.get("+freeq.at/unreact"), tags.get("+reply"))
    {
        let nick = conn.nick_or_star().to_string();
        let target_msgid = target_msgid.clone();
        let emoji = emoji.clone();
        state.with_db(|db| db.remove_reaction(&target_msgid, &nick, &emoji));
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
        // Channel TAGMSG — enforce +n (no external messages) and +m (moderated)
        // Resolve sender DID once, before taking the channels lock.
        let sender_did = state.session_dids.lock().get(&conn.id).cloned();
        {
            let channels = state.channels.lock();
            if let Some(ch) = channels.get(target) {
                // Founder + persistent DID-ops bypass +m. (+n is membership-based;
                // a non-member can't be founder anyway, so no bypass needed there.)
                let is_did_authority = sender_did.as_deref().is_some_and(|d| {
                    ch.founder_did.as_deref() == Some(d) || ch.did_ops.contains(d)
                });
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
                    && !is_did_authority
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

        // Broadcast channel TAGMSG to S2S peers
        super::helpers::s2s_broadcast(
            state,
            crate::s2s::S2sMessage::Tagmsg {
                event_id: super::helpers::s2s_next_event_id(state),
                from: conn.hostmask(),
                target: target.to_string(),
                tags: tags.clone(),
                origin: state.server_iroh_id.lock().clone().unwrap_or_default(),
            },
        );
    } else {
        // TAGMSG to a nick — route through federation layer.
        use super::routing::{RouteResult, relay_to_nick};
        // TAGMSG uses the same relay path as PRIVMSG.
        // The text payload is empty for TAGMSG; tags ride in the from-line.
        let from_nick = conn.nick.as_deref().unwrap_or("*").to_string();
        let tag_text = plain_fallback.as_deref().unwrap_or("").to_string();
        match relay_to_nick(
            state,
            &from_nick,
            target,
            &tag_text,
            super::helpers::s2s_next_event_id(state),
        ) {
            RouteResult::Local(ref session) => {
                // Deliver locally
                if let Some(tx) = state.connections.lock().get(session) {
                    let has_tags = state.cap_message_tags.lock().contains(session);
                    let has_time = state.cap_server_time.lock().contains(session);
                    if has_tags {
                        let line = if has_time {
                            &tagged_line_with_time
                        } else {
                            &tagged_line
                        };
                        let _ = tx.try_send(line.clone());
                    } else if let Some(ref fallback) = plain_fallback {
                        let _ = tx.try_send(fallback.clone());
                    }
                }
                // Also relay via S2S for cross-server visibility
                super::helpers::s2s_broadcast(
                    state,
                    crate::s2s::S2sMessage::Tagmsg {
                        event_id: super::helpers::s2s_next_event_id(state),
                        from: conn.hostmask(),
                        target: target.to_string(),
                        tags: tags.clone(),
                        origin: state.server_iroh_id.lock().clone().unwrap_or_default(),
                    },
                );
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

    let is_channel = target.starts_with('#') || target.starts_with('&');
    let is_notice = command == "NOTICE";

    // Per-session flood protection: max 5 messages per 2 seconds (channels + DMs).
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut ts_map = state.msg_timestamps.lock();
        let ts = ts_map.entry(conn.id.clone()).or_default();
        ts.retain(|&t| now.saturating_sub(t) < 2000);
        if ts.len() >= 5 {
            // NOTICE must never generate error replies (RFC 2812 3.3.2)
            if !is_notice {
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &state.server_name,
                    irc::ERR_CANNOTSENDTOCHAN,
                    vec![nick, target, "Flood protection: sending too fast"],
                );
                if let Some(tx) = state.connections.lock().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
            }
            return;
        }
        ts.push(now);
    }

    if is_channel {
        // Channel message — enforce +n (no external messages) and +m (moderated)
        // Resolve sender DID once, before taking the channels lock.
        let sender_did = state.session_dids.lock().get(&conn.id).cloned();
        {
            let channels = state.channels.lock();
            if let Some(ch) = channels.get(target) {
                // Founder + persistent DID-ops bypass +m.
                let is_did_authority = sender_did.as_deref().is_some_and(|d| {
                    ch.founder_did.as_deref() == Some(d) || ch.did_ops.contains(d)
                });
                // +n: must be a member to send
                if ch.no_ext_msg && !ch.members.contains(&conn.id) {
                    // NOTICE must never generate error replies (RFC 2812 3.3.2)
                    if !is_notice {
                        let nick = conn.nick_or_star();
                        let reply = Message::from_server(
                            &state.server_name,
                            irc::ERR_CANNOTSENDTOCHAN,
                            vec![nick, target, "Cannot send to channel (+n)"],
                        );
                        if let Some(tx) = state.connections.lock().get(&conn.id) {
                            let _ = tx.try_send(format!("{reply}\r\n"));
                        }
                    }
                    return;
                }
                // +m: must be voiced or op to send
                if ch.moderated
                    && !is_did_authority
                    && !ch.ops.contains(&conn.id)
                    && !ch.halfops.contains(&conn.id)
                    && !ch.voiced.contains(&conn.id)
                {
                    if !is_notice {
                        let nick = conn.nick_or_star();
                        let reply = Message::from_server(
                            &state.server_name,
                            irc::ERR_CANNOTSENDTOCHAN,
                            vec![nick, target, "Cannot send to channel (+m)"],
                        );
                        if let Some(tx) = state.connections.lock().get(&conn.id) {
                            let _ = tx.try_send(format!("{reply}\r\n"));
                        }
                    }
                    return;
                }
                // +E: encrypted-only mode.
                //
                // SECURITY (CTF-21): require BOTH the `+encrypted` tag
                // AND an actual ENC1-prefixed ciphertext body. Previously
                // only the tag was checked, so a malicious client could
                // send `@+encrypted PRIVMSG #ch :leaked plaintext` and
                // any logger or non-aware viewer would see the body in
                // the clear.
                let has_tag = tags.contains_key("+encrypted");
                let has_enc1 = text.starts_with("ENC1:") && text.len() > 5;
                if ch.encrypted_only && !(has_tag && has_enc1) {
                    if !is_notice {
                        let nick = conn.nick_or_star();
                        let reason = if !has_tag {
                            "Cannot send to channel (+E) — messages must carry the +encrypted tag"
                        } else {
                            "Cannot send to channel (+E) — body must be ENC1-prefixed ciphertext"
                        };
                        let reply = Message::from_server(
                            &state.server_name,
                            irc::ERR_CANNOTSENDTOCHAN,
                            vec![nick, target, reason],
                        );
                        if let Some(tx) = state.connections.lock().get(&conn.id) {
                            let _ = tx.try_send(format!("{reply}\r\n"));
                        }
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
            let mut history_tags = full_tags.clone();
            if let Some(did) = conn.authenticated_did.as_deref() {
                history_tags.insert("account".to_string(), did.to_string());
            }
            let mut channels = state.channels.lock();
            if let Some(ch) = channels.get_mut(target) {
                ch.history.push_back(HistoryMessage {
                    from: hostmask.clone(),
                    text: text.to_string(),
                    timestamp,
                    tags: history_tags,
                    msgid: Some(msgid.clone()),
                });
                while ch.history.len() > MAX_HISTORY {
                    ch.history.pop_front();
                }
            }
            drop(channels);
            let sender_did = conn.authenticated_did.as_deref();
            state.with_db(|db| {
                db.insert_message(target, &hostmask, text, timestamp, tags, Some(&msgid), sender_did)
            });

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
        let account_caps = state.cap_account_tag.lock();
        let echo_caps = state.cap_echo_message.lock();
        let conns = state.connections.lock();
        let sender_did = conn.authenticated_did.as_deref();
        for member_session in &members {
            // echo-message: include sender if they requested it
            if member_session == &conn.id && !echo_caps.contains(member_session) {
                continue;
            }
            if let Some(tx) = conns.get(member_session) {
                let has_tags = tag_caps.contains(member_session);
                let has_time = time_caps.contains(member_session);
                let wants_account =
                    sender_did.is_some() && account_caps.contains(member_session);
                let line: String = if !has_tags {
                    plain_line.clone()
                } else if !wants_account {
                    if has_time {
                        tagged_line_with_time.clone()
                    } else {
                        tagged_line.clone()
                    }
                } else {
                    // Per-recipient build with `account` tag injected.
                    // IRCv3 account-tag spec requires this only for opted-in clients.
                    let mut recip_tags = if has_time {
                        full_tags_with_time.clone()
                    } else {
                        full_tags.clone()
                    };
                    recip_tags.insert("account".to_string(), sender_did.unwrap().to_string());
                    let tag_msg = irc::Message {
                        tags: recip_tags,
                        prefix: Some(hostmask.clone()),
                        command: command.to_string(),
                        params: vec![target.to_string(), text.to_string()],
                    };
                    format!("{tag_msg}\r\n")
                };
                let _ = tx.try_send(line);
            }
        }

        // Broadcast channel PRIVMSG to S2S peers
        if command == "PRIVMSG" {
            let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
            let sig = full_tags.get("+freeq.at/sig").cloned();
            s2s_broadcast(
                state,
                crate::s2s::S2sMessage::Privmsg {
                    event_id: s2s_next_event_id(state),
                    from: conn.nick.as_deref().unwrap_or("*").to_string(),
                    target: target.to_string(),
                    text: text.to_string(),
                    origin,
                    msgid: Some(msgid.clone()),
                    sig,
                    // Coordination card tags ride with the message so a
                    // federated peer's clients render the same card.
                    tags: crate::s2s::relay_coordination_tags(&full_tags),
                },
            );
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
                tags: pm_tags_with_time.clone(),
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            format!("{tag_msg}\r\n")
        };

        // Build the line for a recipient based on their negotiated caps.
        // Honors message-tags, server-time, and account-tag (per IRCv3 spec).
        let sender_did_for_dm = conn.authenticated_did.clone();
        let build_dm_line = |recipient_session: &str| -> String {
            let has_tags = state.cap_message_tags.lock().contains(recipient_session);
            if !has_tags {
                return plain_line.clone();
            }
            let has_time = state.cap_server_time.lock().contains(recipient_session);
            let wants_account = sender_did_for_dm.is_some()
                && state.cap_account_tag.lock().contains(recipient_session);
            if !wants_account {
                return if has_time {
                    tagged_line_with_time.clone()
                } else {
                    tagged_line.clone()
                };
            }
            let mut recip_tags = if has_time {
                pm_tags_with_time.clone()
            } else {
                pm_tags.clone()
            };
            recip_tags.insert(
                "account".to_string(),
                sender_did_for_dm.clone().unwrap(),
            );
            let tag_msg = irc::Message {
                tags: recip_tags,
                prefix: Some(hostmask.clone()),
                command: command.to_string(),
                params: vec![target.to_string(), text.to_string()],
            };
            format!("{tag_msg}\r\n")
        };

        // Route through the federation routing layer.
        // See routing.rs for why we NEVER gate on remote_members here.
        use super::routing::{RouteResult, relay_to_nick};
        let from_nick = conn.nick.as_deref().unwrap_or("*").to_string();
        match relay_to_nick(state, &from_nick, target, text, s2s_next_event_id(state)) {
            RouteResult::Local(ref session) => {
                // Target is local — deliver to ALL sessions for target's DID (multi-device).
                // Also relay via S2S so the DM is visible on other federated servers
                // (e.g. sender logged into multiple servers).
                super::helpers::s2s_broadcast(
                    state,
                    crate::s2s::S2sMessage::Privmsg {
                        event_id: s2s_next_event_id(state),
                        from: conn.hostmask(),
                        target: target.to_string(),
                        text: text.to_string(),
                        origin: state.server_iroh_id.lock().clone().unwrap_or_default(),
                        msgid: Some(pm_msgid.clone()),
                        sig: pm_tags.get("+freeq.at/sig").cloned(),
                        tags: crate::s2s::relay_coordination_tags(&pm_tags),
                    },
                );
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
                        state
                            .did_sessions
                            .lock()
                            .get(did)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_else(|| vec![session.clone()])
                    } else {
                        vec![session.clone()] // Guest — single session
                    }
                };

                let conns = state.connections.lock();
                // Deliver to all target sessions
                for target_session in &target_sessions {
                    let line = build_dm_line(target_session);
                    if let Some(tx) = conns.get(target_session) {
                        if let Err(_e) = tx.try_send(line) {
                            let target_nick = state.nick_to_session.lock().get_nick(target_session).map(|s| s.to_string()).unwrap_or_default();
                            tracing::warn!(
                                from = %conn.nick.as_deref().unwrap_or("?"),
                                to = %target_nick,
                                session = %target_session,
                                "DM dropped: target send buffer full"
                            );
                        }
                    }
                }

                // echo-message: echo DM back to ALL sender's sessions
                let sender_sessions: Vec<String> = {
                    if let Some(ref did) = conn.authenticated_did {
                        state
                            .did_sessions
                            .lock()
                            .get(did)
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
                            let echo_line = build_dm_line(&conn.id);
                            if let Some(tx) = conns.get(&conn.id) {
                                let _ = tx.try_send(echo_line);
                            }
                        }
                    } else {
                        // Other sessions of sender — deliver as if they received it
                        let line = build_dm_line(sender_session);
                        if let Some(tx) = conns.get(sender_session) {
                            let _ = tx.try_send(line);
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
                    let echo_line = build_dm_line(&conn.id);
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(echo_line);
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

        // Persist DM if both sender and recipient have DIDs
        let sender_did = conn.authenticated_did.as_deref();
        let recipient_did = state.nick_owners.lock().get(&target.to_lowercase()).cloned();
        if let (Some(s_did), Some(r_did)) = (sender_did, recipient_did.as_deref()) {
            let dm_key = crate::db::canonical_dm_key(s_did, r_did);
            let did_for_db = Some(s_did);
            state.with_db(|db| {
                db.insert_message(&dm_key, &hostmask, text, timestamp, &pm_tags, Some(&pm_msgid), did_for_db)
            });
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

    // Handle TARGETS subcommand separately — different parameter format.
    // CHATHISTORY TARGETS <from_ts> <to_ts> <limit>
    if subcmd == "TARGETS" {
        handle_chathistory_targets(conn, msg, state, server_name, session_id, send);
        return;
    }

    let raw_target = &msg.params[1];
    let is_channel = raw_target.starts_with('#') || raw_target.starts_with('&');

    // Resolve target and authorize access.
    // For channels: membership check. For DMs: auth check + canonical key.
    // db_key = key used for DB queries, target = display name for IRC messages.
    let (db_key, target) = if is_channel {
        let target = normalize_channel(raw_target);
        {
            let channels = state.channels.lock();
            if let Some(ch) = channels.get(&target) {
                if !ch.members.contains(session_id) {
                    let reply = Message::from_server(
                        server_name,
                        "FAIL",
                        vec![
                            "CHATHISTORY",
                            "INVALID_TARGET",
                            &target,
                            "You are not in that channel",
                        ],
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
        (target.clone(), target)
    } else {
        // DM target — require DID authentication
        let requester_did = match conn.authenticated_did.as_deref() {
            Some(did) => did.to_string(),
            None => {
                let reply = Message::from_server(
                    server_name,
                    "FAIL",
                    vec![
                        "CHATHISTORY",
                        "ACCOUNT_REQUIRED",
                        raw_target,
                        "You must be authenticated to access DM history",
                    ],
                );
                send(state, session_id, format!("{reply}\r\n"));
                return;
            }
        };

        // Resolve target to DID — accept DID directly or resolve nick
        let target_did = if raw_target.starts_with("did:") {
            raw_target.to_string()
        } else {
            match state
                .nick_owners
                .lock()
                .get(&raw_target.to_lowercase())
                .cloned()
            {
                Some(did) => did,
                None => {
                    let reply = Message::from_server(
                        server_name,
                        "FAIL",
                        vec![
                            "CHATHISTORY",
                            "INVALID_TARGET",
                            raw_target,
                            "Unknown target",
                        ],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                    return;
                }
            }
        };

        let dm_key = crate::db::canonical_dm_key(&requester_did, &target_did);
        (dm_key, raw_target.to_string())
    };

    let has_tags = state.cap_message_tags.lock().contains(session_id);
    let has_time = state.cap_server_time.lock().contains(session_id);
    let has_batch = state.cap_batch.lock().contains(session_id);

    // Fetch messages from DB based on subcommand
    let messages: Vec<crate::db::MessageRow> = match subcmd.as_str() {
        "BEFORE" => {
            if msg.params.len() < 4 {
                vec![]
            } else {
                let ts = parse_chathistory_ts(&msg.params[2]).unwrap_or(u64::MAX);
                let limit = msg.params[3].parse::<usize>().unwrap_or(50).min(500);
                state
                    .with_db(|db| db.get_messages(&db_key, limit, Some(ts)))
                    .unwrap_or_default()
            }
        }
        "AFTER" => {
            if msg.params.len() < 4 {
                vec![]
            } else {
                let ts = parse_chathistory_ts(&msg.params[2]).unwrap_or(0);
                let limit = msg.params[3].parse::<usize>().unwrap_or(50).min(500);
                state
                    .with_db(|db| db.get_messages_after(&db_key, ts, limit))
                    .unwrap_or_default()
            }
        }
        "LATEST" => {
            if msg.params.len() < 4 {
                vec![]
            } else {
                let limit = msg.params[3].parse::<usize>().unwrap_or(50).min(500);
                if msg.params[2] == "*" {
                    state
                        .with_db(|db| db.get_messages(&db_key, limit, None))
                        .unwrap_or_default()
                } else {
                    let ts = parse_chathistory_ts(&msg.params[2]).unwrap_or(0);
                    state
                        .with_db(|db| db.get_messages_after(&db_key, ts, limit))
                        .unwrap_or_default()
                }
            }
        }
        "BETWEEN" => {
            if msg.params.len() < 5 {
                vec![]
            } else {
                let start = parse_chathistory_ts(&msg.params[2]).unwrap_or(0);
                let end = parse_chathistory_ts(&msg.params[3]).unwrap_or(u64::MAX);
                let limit = msg.params[4].parse::<usize>().unwrap_or(50).min(500);
                state
                    .with_db(|db| db.get_messages_between(&db_key, start, end, limit))
                    .unwrap_or_default()
            }
        }
        _ => vec![],
    };

    // Send as a batch (unique ID per request)
    let batch_id = format!("ch{}", crate::msgid::generate());
    if has_batch {
        send(
            state,
            session_id,
            format!(":{server_name} BATCH +{batch_id} chathistory {target}\r\n"),
        );
    }

    // Fetch reactions for all messages in this batch
    let msgids: Vec<&str> = messages.iter().filter_map(|r| r.msgid.as_deref()).collect();
    let reactions: std::collections::HashMap<String, Vec<crate::db::ReactionRow>> =
        state.with_db(|db| db.get_reactions_for_messages(&msgids))
            .unwrap_or_default();

    for row in &messages {
        let mut tags = if has_tags {
            row.tags.clone()
        } else {
            std::collections::HashMap::new()
        };
        // Include msgid if available
        if has_tags {
            if let Some(ref mid) = row.msgid {
                tags.insert("msgid".to_string(), mid.clone());
                // Include reactions as +freeq.at/reactions tag
                // Format: emoji1:nick1,nick2;emoji2:nick3
                if let Some(reaction_rows) = reactions.get(mid) {
                    let mut by_emoji: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
                    for r in reaction_rows {
                        by_emoji.entry(&r.emoji).or_default().push(&r.reactor_nick);
                    }
                    let encoded: Vec<String> = by_emoji.iter().map(|(emoji, nicks)| {
                        format!("{}:{}", emoji, nicks.join(","))
                    }).collect();
                    if !encoded.is_empty() {
                        tags.insert("+freeq.at/reactions".to_string(), encoded.join(";"));
                    }
                }
            }
            if let Some(ref replaces) = row.replaces_msgid {
                tags.entry("+draft/edit".to_string())
                    .or_insert_with(|| replaces.clone());
            }
            if let Some(ref did) = row.sender_did {
                tags.insert("account".to_string(), did.clone());
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
            send(
                state,
                session_id,
                format!(":{} PRIVMSG {} :{}\r\n", row.sender, target, row.text),
            );
        }
    }

    if has_batch {
        send(
            state,
            session_id,
            format!(":{server_name} BATCH -{batch_id}\r\n"),
        );
    }
}

/// Handle CHATHISTORY TARGETS — list DM conversations for the authenticated user.
/// CHATHISTORY TARGETS <from_ts> <to_ts> <limit>
fn handle_chathistory_targets(
    conn: &Connection,
    msg: &irc::Message,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &dyn Fn(&Arc<SharedState>, &str, String),
) {
    // Require DID authentication
    let requester_did = match conn.authenticated_did.as_deref() {
        Some(did) => did,
        None => {
            let reply = Message::from_server(
                server_name,
                "FAIL",
                vec![
                    "CHATHISTORY",
                    "ACCOUNT_REQUIRED",
                    "*",
                    "You must be authenticated to list DM targets",
                ],
            );
            send(state, session_id, format!("{reply}\r\n"));
            return;
        }
    };

    let from_ts = if msg.params.len() > 1 {
        parse_chathistory_ts(&msg.params[1]).unwrap_or(0)
    } else {
        0
    };
    let to_ts = if msg.params.len() > 2 {
        parse_chathistory_ts(&msg.params[2]).unwrap_or(u64::MAX)
    } else {
        u64::MAX
    };
    let limit = if msg.params.len() > 3 {
        msg.params[3].parse::<usize>().unwrap_or(50).min(500)
    } else {
        50
    };

    let has_batch = state.cap_batch.lock().contains(session_id);
    let has_time = state.cap_server_time.lock().contains(session_id);

    let dm_conversations = state
        .with_db(|db| db.dm_conversations(requester_did, limit))
        .unwrap_or_default();

    let batch_id = format!("cht{}", crate::msgid::generate());
    if has_batch {
        send(
            state,
            session_id,
            format!(":{server_name} BATCH +{batch_id} draft/chathistory-targets\r\n"),
        );
    }

    for (dm_key, last_ts) in &dm_conversations {
        // Filter by timestamp range
        if *last_ts < from_ts || *last_ts > to_ts {
            continue;
        }

        // Extract partner DID from canonical key (dm:<did_a>,<did_b>)
        let partner_did = dm_key.strip_prefix("dm:").and_then(|rest| {
            let parts: Vec<&str> = rest.splitn(2, ',').collect();
            if parts.len() == 2 {
                if parts[0] == requester_did {
                    Some(parts[1])
                } else {
                    Some(parts[0])
                }
            } else {
                None
            }
        });

        if let Some(partner) = partner_did {
            // Resolve DID → nick for display via the full chain
            // (did_nicks → live session → identities table → raw DID),
            // so an offline agent with a persisted/known binding still
            // shows a name instead of the raw did:key.
            let display_nick = state.display_nick_for_did(partner);

            let mut tags = std::collections::HashMap::new();
            if has_batch {
                tags.insert("batch".to_string(), batch_id.clone());
            }
            if has_time {
                let ts_str = chrono::DateTime::from_timestamp(*last_ts as i64, 0)
                    .unwrap_or_default()
                    .format("%Y-%m-%dT%H:%M:%S.000Z")
                    .to_string();
                tags.insert("time".to_string(), ts_str);
            }

            if !tags.is_empty() {
                let tag_msg = irc::Message {
                    tags,
                    prefix: Some(server_name.to_string()),
                    command: "CHATHISTORY".to_string(),
                    params: vec!["TARGETS".to_string(), display_nick],
                };
                send(state, session_id, format!("{tag_msg}\r\n"));
            } else {
                send(
                    state,
                    session_id,
                    format!(":{server_name} CHATHISTORY TARGETS {display_nick}\r\n"),
                );
            }
        }
    }

    if has_batch {
        send(
            state,
            session_id,
            format!(":{server_name} BATCH -{batch_id}\r\n"),
        );
    }
}

// ── Message editing ─────────────────────────────────────────────────

/// Handle a PRIVMSG with +draft/edit=<msgid> tag.
/// Verifies authorship, stores the edit, and broadcasts to channel or DM recipient.
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
    let is_channel = target.starts_with('#') || target.starts_with('&');

    // Verify authorship: look up original message by msgid
    // For DMs, messages are stored under the canonical dm_key, not the nick.
    // Try the target first (works for channels), then fall back to a global lookup.
    let original = {
        let by_target = state.with_db(|db| db.get_message_by_msgid(target, original_msgid));
        match &by_target {
            Some(Some(_)) => by_target,
            _ => {
                // Channel lookup failed — try DM key if this is a DM
                if !is_channel {
                    if let Some(sender_did) = conn.authenticated_did.as_deref() {
                        if let Some(recipient_did) = state.nick_owners.lock().get(&target.to_lowercase()).cloned() {
                            let dm_key = crate::db::canonical_dm_key(sender_did, &recipient_did);
                            let by_dm = state.with_db(|db| db.get_message_by_msgid(&dm_key, original_msgid));
                            if matches!(&by_dm, Some(Some(_))) {
                                by_dm
                            } else {
                                // Final fallback: global msgid search
                                state.with_db(|db| db.find_message_by_msgid(original_msgid))
                            }
                        } else {
                            state.with_db(|db| db.find_message_by_msgid(original_msgid))
                        }
                    } else {
                        state.with_db(|db| db.find_message_by_msgid(original_msgid))
                    }
                } else {
                    by_target
                }
            }
        }
    };
    match original {
        Some(Some(row)) => {
            // Prefer DID-based authorship check to prevent nick-reuse attacks
            let is_author = if let (Some(msg_did), Some(conn_did)) = (&row.sender_did, &conn.authenticated_did) {
                msg_did == conn_did
            } else if row.sender_did.is_some() {
                // Original message was from an authenticated user but current user has no DID
                // (or has a different DID) — deny
                false
            } else {
                // Fallback to nick comparison for guest (non-DID) messages
                let original_nick = row.sender.split('!').next().unwrap_or("");
                original_nick.eq_ignore_ascii_case(nick)
            };
            if !is_author {
                let reply = Message::from_server(
                    &state.server_name,
                    "FAIL",
                    vec![
                        "EDIT",
                        "AUTHOR_MISMATCH",
                        "You can only edit your own messages",
                    ],
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
    if let Some(sig) = resolve_signature(conn, target, new_text, edit_timestamp, client_sig, state)
    {
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
    let store_tags: std::collections::HashMap<String, String> = tags
        .iter()
        .filter(|(k, _)| *k != "msgid")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // For DMs, store under the canonical dm_key (not the nick) so
    // edits appear in CHATHISTORY alongside the original message.
    let store_channel = if is_channel {
        target.to_string()
    } else if let Some(sender_did) = conn.authenticated_did.as_deref() {
        if let Some(recipient_did) = state.nick_owners.lock().get(&target.to_lowercase()).cloned() {
            crate::db::canonical_dm_key(sender_did, &recipient_did)
        } else {
            target.to_string()
        }
    } else {
        target.to_string()
    };
    let editor_did = conn.authenticated_did.as_deref();
    state.with_db(|db| {
        db.insert_edit(
            &store_channel,
            &hostmask,
            new_text,
            timestamp,
            &store_tags,
            &edit_msgid,
            original_msgid,
            editor_did,
        )
    });

    // Update in-memory history (channels only)
    // Note: we keep the original msgid stable so that subsequent edits
    // (e.g., streaming) can still find the message by original_msgid.
    if is_channel {
        let mut channels = state.channels.lock();
        if let Some(ch) = channels.get_mut(target) {
            for hist in ch.history.iter_mut() {
                if hist.msgid.as_deref() == Some(original_msgid) {
                    hist.text = new_text.to_string();
                    // Don't change hist.msgid — keep original stable for chained edits
                    break;
                }
            }
        }
    }

    // Deliver edit
    if is_channel {
        // Channel: deliver to all members
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
        for sid in &members {
            if sid == &conn.id && !echo_caps.contains(sid) {
                continue;
            }
            if let Some(tx) = conns.get(sid) {
                let line = if tag_caps.contains(sid) {
                    if time_caps.contains(sid) {
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

        // Broadcast to S2S peers
        let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
        let sig = full_tags.get("+freeq.at/sig").cloned();
        s2s_broadcast(
            state,
            crate::s2s::S2sMessage::Privmsg {
                event_id: s2s_next_event_id(state),
                from: nick.to_string(),
                target: target.to_string(),
                text: new_text.to_string(),
                origin,
                msgid: Some(edit_msgid),
                sig,
                tags: crate::s2s::relay_coordination_tags(&full_tags),
            },
        );
    } else {
        // DM: deliver to target nick and echo to sender
        use super::routing::{RouteResult, relay_to_nick};
        let from_nick = conn.nick.as_deref().unwrap_or("*").to_string();

        match relay_to_nick(state, &from_nick, target, new_text, s2s_next_event_id(state)) {
            RouteResult::Local(ref session) => {
                // Find all sessions for target's DID (multi-device support)
                let target_sessions: Vec<String> = {
                    let target_did = state.session_dids.lock().get(session).cloned();
                    if let Some(ref did) = target_did {
                        state
                            .did_sessions
                            .lock()
                            .get(did)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_else(|| vec![session.clone()])
                    } else {
                        vec![session.clone()]
                    }
                };

                let conns = state.connections.lock();
                // Deliver to all target sessions
                for target_session in &target_sessions {
                    let has_tags = state.cap_message_tags.lock().contains(target_session);
                    let has_time = state.cap_server_time.lock().contains(target_session);
                    let line = if has_tags {
                        if has_time { &tagged_line_with_time } else { &tagged_line }
                    } else {
                        &plain_line
                    };
                    if let Some(tx) = conns.get(target_session) {
                        let _ = tx.try_send(line.clone());
                    }
                }

                // Echo to sender if echo-message enabled
                if state.cap_echo_message.lock().contains(&conn.id) {
                    let has_tags = state.cap_message_tags.lock().contains(&conn.id);
                    let has_time = state.cap_server_time.lock().contains(&conn.id);
                    let line = if has_tags {
                        if has_time { &tagged_line_with_time } else { &tagged_line }
                    } else {
                        &plain_line
                    };
                    if let Some(tx) = conns.get(&conn.id) {
                        let _ = tx.try_send(line.clone());
                    }
                }
            }
            RouteResult::Relayed => {
                // Target is on a federated peer — edit was relayed
                // Echo to sender
                if state.cap_echo_message.lock().contains(&conn.id) {
                    let has_tags = state.cap_message_tags.lock().contains(&conn.id);
                    let has_time = state.cap_server_time.lock().contains(&conn.id);
                    let line = if has_tags {
                        if has_time { &tagged_line_with_time } else { &tagged_line }
                    } else {
                        &plain_line
                    };
                    if let Some(tx) = state.connections.lock().get(&conn.id) {
                        let _ = tx.try_send(line.clone());
                    }
                }
            }
            RouteResult::Unreachable => {
                // Target not found — send error
                let reply = Message::from_server(
                    &state.server_name,
                    irc::ERR_NOSUCHNICK,
                    vec![&nick, target, "No such nick"],
                );
                if let Some(tx) = state.connections.lock().get(&conn.id) {
                    let _ = tx.try_send(format!("{reply}\r\n"));
                }
            }
        }
    }
}

// ── Message deletion ────────────────────────────────────────────────

/// Handle a TAGMSG with +draft/delete=<msgid> tag.
/// Verifies authorship, soft-deletes the message, broadcasts to channel or DM recipient.
fn handle_delete(conn: &Connection, target: &str, original_msgid: &str, state: &Arc<SharedState>) {
    let hostmask = conn.hostmask();
    let nick = conn.nick_or_star();
    let is_channel = target.starts_with('#') || target.starts_with('&');

    // Verify authorship
    let original = state.with_db(|db| db.get_message_by_msgid(target, original_msgid));
    match original {
        Some(Some(row)) => {
            // Prefer DID-based authorship check to prevent nick-reuse attacks
            let is_author = if let (Some(msg_did), Some(conn_did)) = (&row.sender_did, &conn.authenticated_did) {
                msg_did == conn_did
            } else if row.sender_did.is_some() {
                // Original message was from an authenticated user but current user has no DID
                // (or has a different DID) — deny
                false
            } else {
                // Fallback to nick comparison for guest (non-DID) messages
                let original_nick = row.sender.split('!').next().unwrap_or("");
                original_nick.eq_ignore_ascii_case(nick)
            };
            if !is_author {
                // Also allow ops to delete messages (channels only)
                let is_op = is_channel && state
                    .channels
                    .lock()
                    .get(target)
                    .map(|ch| ch.ops.contains(&conn.id))
                    .unwrap_or(false);
                if !is_op {
                    let reply = Message::from_server(
                        &state.server_name,
                        "FAIL",
                        vec![
                            "DELETE",
                            "AUTHOR_MISMATCH",
                            "You can only delete your own messages",
                        ],
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

    // Remove from in-memory history and pins (channels only)
    if is_channel {
        let mut channels = state.channels.lock();
        if let Some(ch) = channels.get_mut(target) {
            ch.history
                .retain(|h| h.msgid.as_deref() != Some(original_msgid));
            ch.pins.retain(|p| p.msgid != original_msgid);
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

    // Deliver delete notification
    if is_channel {
        // Channel: deliver to tag-capable members only (plain clients can't see deletes)
        let members: Vec<String> = state
            .channels
            .lock()
            .get(target)
            .map(|ch| ch.members.iter().cloned().collect())
            .unwrap_or_default();

        let tag_caps = state.cap_message_tags.lock();
        let conns = state.connections.lock();
        for sid in &members {
            if sid == &conn.id {
                continue; // Don't echo delete back to sender
            }
            if tag_caps.contains(sid)
                && let Some(tx) = conns.get(sid)
            {
                let _ = tx.try_send(tagged_line.clone());
            }
        }
    } else {
        // DM: deliver to target nick
        // Note: We don't use relay_to_nick here since it sends PRIVMSG, but we need TAGMSG.
        // For DMs, we handle delivery manually here.
        if let Some(session) = state.nick_to_session.lock().get_session(target).map(|s| s.to_string()) {
            // Find all sessions for target's DID (multi-device support)
            let target_sessions: Vec<String> = {
                let target_did = state.session_dids.lock().get(&session).cloned();
                if let Some(ref did) = target_did {
                    state
                        .did_sessions
                        .lock()
                        .get(did)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_else(|| vec![session.clone()])
                } else {
                    vec![session.clone()]
                }
            };

            let tag_caps = state.cap_message_tags.lock();
            let conns = state.connections.lock();
            for target_session in &target_sessions {
                if tag_caps.contains(target_session) {
                    if let Some(tx) = conns.get(target_session) {
                        let _ = tx.try_send(tagged_line.clone());
                    }
                }
            }
        }
        // Note: For federated DM deletes, we'd need S2S support — not implemented yet
    }
}

// ── AV session control ─────────────────────────────────────────────

/// Send a line to a specific session.
fn send_to(state: &Arc<SharedState>, session_id: &str, line: String) {
    if let Some(tx) = state.connections.lock().get(session_id) {
        let _ = tx.try_send(line);
    }
}

/// Handle TAGMSG with +freeq.at/av-* tags (session lifecycle control).
fn handle_av_tagmsg(
    conn: &super::Connection,
    target: &str,
    tags: &std::collections::HashMap<String, String>,
    av_tag: &str,
    state: &Arc<SharedState>,
) {
    let nick = conn.nick_or_star().to_string();
    // Use DID if authenticated, otherwise use nick as fallback identity
    let did = conn.authenticated_did.clone()
        .unwrap_or_else(|| format!("guest:{nick}"));

    let session_id = tags.get("+freeq.at/av-id").cloned().unwrap_or_default();

    match av_tag {
        "+freeq.at/av-start" => {
            let title = tags.get("+freeq.at/av-title").map(|s| s.as_str());
            let instance_id = tags.get("+freeq.at/av-instance").map(String::as_str);
            let channel = if target.starts_with('#') || target.starts_with('&') {
                Some(target)
            } else {
                None
            };

            let mut mgr = state.av_sessions.lock();
            match mgr.create_session(channel, &did, &nick, title, instance_id) {
                Ok(session) => {
                    let session_id = session.id.clone();
                    let participant_count = mgr.active_participant_count(&session_id);

                    // Persist to DB
                    if let Some(s) = mgr.get(&session_id) {
                        state.with_db(|db| db.save_av_session(s));
                    }

                    drop(mgr);

                    // Record this instance against the IRC connection. Without
                    // this, av-start-only clients (the iOS path doesn't send
                    // a separate av-join after creating the session) leave
                    // av_instances_per_conn empty, and the disconnect handler
                    // falls into the legacy whole-DID cleanup which ends the
                    // session on every minor reconnect blip.
                    if let Some(inst) = instance_id {
                        state
                            .av_instances_per_conn
                            .lock()
                            .entry(conn.id.clone())
                            .or_default()
                            .insert(inst.to_string());
                    }

                    // Broadcast session start to channel
                    let title_display = title.unwrap_or("voice session");
                    broadcast_av_state(state, target, &session_id, "started", &nick, participant_count, title_display);

                    // Create iroh-live Room for native client P2P audio.
                    // Browser clients use MoQ SFU; native clients join the Room directly.
                    {
                        let backend = state.av_media.lock().clone();
                        let state2 = state.clone();
                        let sid = session_id.clone();
                        let conn_id = conn.id.clone();
                        let nick2 = nick.clone();
                        tokio::spawn(async move {
                            if let Some(backend) = backend.as_ref() {
                                match crate::av_media::MediaBackend::create_room(backend.as_ref(), &sid).await {
                                    Ok(ticket) => {
                                        // Store ticket in session
                                        let mut mgr = state2.av_sessions.lock();
                                        if let Some(s) = mgr.sessions.get_mut(&sid) {
                                            s.iroh_ticket = Some(ticket.clone());
                                        }
                                        if let Some(s) = mgr.get(&sid) {
                                            state2.with_db(|db| db.save_av_session(s));
                                        }
                                        drop(mgr);
                                        // Send ticket to creator
                                        let notice = Message::from_server(
                                            &state2.server_name,
                                            "NOTICE",
                                            vec![&nick2, &format!("AV ticket: {ticket}")],
                                        );
                                        send_to(&state2, &conn_id, format!("{notice}\r\n"));

                                        // Start the MoQ↔Room bridge
                                        #[cfg(feature = "av-native")]
                                        {
                                            if let Some((room_handle, room_events)) = backend.take_room_for_bridge(&sid) {
                                                let sfu = state2.sfu_state.lock().clone();
                                                if let Some(sfu) = sfu {
                                                    let bridge = crate::av_bridge::start_bridge(
                                                        sid.clone(),
                                                        sfu.cluster.clone(),
                                                        sfu.auth.clone(),
                                                        room_handle,
                                                        room_events,
                                                    );
                                                    // Store bridge handle to keep it alive
                                                    state2.av_bridges.lock().insert(sid.clone(), bridge);
                                                    tracing::info!(session = %sid, "MoQ↔Room bridge started");
                                                } else {
                                                    tracing::warn!(session = %sid, "SFU not available — bridge not started");
                                                }
                                            }
                                        }

                                        tracing::info!(session = %sid, "iroh-live room created");
                                    }
                                    Err(e) => {
                                        tracing::warn!(session = %sid, error = %e, "Failed to create iroh-live room");
                                    }
                                }
                            }
                        });
                    }

                    // Send session ID back to creator
                    let notice = Message::from_server(
                        &state.server_name,
                        "NOTICE",
                        vec![&nick, &format!("AV session started: {session_id}")],
                    );
                    send_to(state, &conn.id, format!("{notice}\r\n"));

                    // Broadcast via S2S
                    broadcast_av_s2s(state, "created", &session_id, channel, &did, &nick, title, None);

                    tracing::info!(session_id = %session_id, channel = ?channel, did = %did, "AV session created");
                }
                Err(e) => {
                    let reply = Message::from_server(
                        &state.server_name,
                        "NOTICE",
                        vec![&nick, &format!("Cannot start session: {e}")],
                    );
                    send_to(state, &conn.id, format!("{reply}\r\n"));
                }
            }
        }

        "+freeq.at/av-join" => {
            if session_id.is_empty() {
                // Try to join the channel's active session
                let mgr = state.av_sessions.lock();
                if let Some(s) = mgr.active_session_for_channel(target) {
                    let id = s.id.clone();
                    drop(mgr);
                    // Re-call with the session ID
                    let mut tags2 = tags.clone();
                    tags2.insert("+freeq.at/av-id".to_string(), id);
                    return handle_av_tagmsg(conn, target, &tags2, av_tag, state);
                }
                let reply = Message::from_server(
                    &state.server_name,
                    "NOTICE",
                    vec![&nick, "No active session in this channel"],
                );
                send_to(state, &conn.id, format!("{reply}\r\n"));
                return;
            }

            // Per-device suffix: clients send a short random `av-instance`
            // tag so two devices on the same DID get separate participant
            // slots and distinct MoQ broadcast paths. Older clients omit
            // it; the manager falls back to one-slot-per-DID for them.
            let instance_id = tags.get("+freeq.at/av-instance").map(String::as_str);

            tracing::info!(
                session_id = %session_id,
                did = %did,
                nick = %nick,
                conn_id = %conn.id,
                instance_id = ?instance_id,
                "av-join: handler entry"
            );

            // Before joining: reap any orphan slots in this session whose
            // owning IRC connection is gone. Live-set is built from the
            // (did, instance) pairs that current connections registered on
            // their own av-join. Without this, a refreshed/crashed tab
            // leaves a `left_at: None` ghost in the participants list and
            // peers waste subscriptions on a broadcast nobody publishes.
            let live: std::collections::HashSet<(String, Option<String>)> = {
                let per_conn = state.av_instances_per_conn.lock();
                let dids = state.session_dids.lock();
                per_conn
                    .iter()
                    .flat_map(|(sid, instances)| {
                        let did_for_sid = dids.get(sid).cloned();
                        let did_for_sid = did_for_sid.unwrap_or_default();
                        instances
                            .iter()
                            .map(move |inst| (did_for_sid.clone(), Some(inst.clone())))
                    })
                    // Always treat the joiner as live, even before we've
                    // recorded their instance.
                    .chain(std::iter::once((
                        did.clone(),
                        instance_id.map(|s| s.to_string()),
                    )))
                    .collect()
            };

            let mut mgr = state.av_sessions.lock();
            mgr.reap_orphan_slots(&session_id, &live);
            match mgr.join_session(&session_id, &did, &nick, instance_id) {
                Ok(session) => {
                    let participant_count = mgr.active_participant_count(&session_id);
                    let channel = session.channel.clone();

                    if let Some(s) = mgr.get(&session_id) {
                        state.with_db(|db| db.save_av_session(s));
                    }
                    drop(mgr);

                    // Record this instance against the IRC connection so the
                    // disconnect handler can clean only this slot — not every
                    // slot of the same DID across other tabs/devices.
                    if let Some(inst) = instance_id {
                        state
                            .av_instances_per_conn
                            .lock()
                            .entry(conn.id.clone())
                            .or_default()
                            .insert(inst.to_string());
                    }

                    // Send iroh-live RoomTicket to joiner (for native clients)
                    if let Some(ticket) = &session.iroh_ticket {
                        let ticket_notice = Message::from_server(
                            &state.server_name,
                            "NOTICE",
                            vec![&nick, &format!("AV ticket: {ticket}")],
                        );
                        send_to(state, &conn.id, format!("{ticket_notice}\r\n"));
                    }

                    // Ensure bridge is running for this session.
                    // The bridge may have been cleaned up if the session creator disconnected
                    // while other participants remained (session stayed active but bridge was orphaned).
                    #[cfg(feature = "av-native")]
                    {
                        let has_bridge = state.av_bridges.lock().contains_key(&session_id);
                        if !has_bridge {
                            let backend = state.av_media.lock().clone();
                            if let Some(backend) = backend.as_ref() {
                                if let Some((room_handle, room_events)) = backend.take_room_for_bridge(&session_id) {
                                    let sfu = state.sfu_state.lock().clone();
                                    if let Some(sfu) = sfu {
                                        let bridge = crate::av_bridge::start_bridge(
                                            session_id.clone(),
                                            sfu.cluster.clone(),
                                            sfu.auth.clone(),
                                            room_handle,
                                            room_events,
                                        );
                                        state.av_bridges.lock().insert(session_id.clone(), bridge);
                                        tracing::info!(session = %session_id, "MoQ↔Room bridge (re)started on join");
                                    }
                                }
                            }
                        }
                    }

                    // Broadcast updated state
                    broadcast_av_state(state, target, &session_id, "joined", &nick, participant_count, "");

                    // S2S
                    broadcast_av_s2s(state, "joined", &session_id, channel.as_deref(), &did, &nick, None, None);

                    tracing::info!(session_id = %session_id, did = %did, "AV session joined");
                }
                Err(e) => {
                    tracing::warn!(
                        session_id = %session_id,
                        did = %did,
                        nick = %nick,
                        instance_id = ?instance_id,
                        error = %e,
                        "av-join rejected by AvSessionManager"
                    );
                    let reply = Message::from_server(
                        &state.server_name,
                        "NOTICE",
                        vec![&nick, &format!("Cannot join session: {e}")],
                    );
                    send_to(state, &conn.id, format!("{reply}\r\n"));
                }
            }
        }

        "+freeq.at/av-leave" => {
            let instance_id = tags.get("+freeq.at/av-instance").map(String::as_str);
            // Untrack this instance against the connection so the disconnect
            // handler doesn't try to leave it a second time.
            if let Some(inst) = instance_id {
                if let Some(set) = state.av_instances_per_conn.lock().get_mut(&conn.id) {
                    set.remove(inst);
                }
            }
            let mut mgr = state.av_sessions.lock();
            match mgr.leave_session(&session_id, &did, instance_id) {
                Ok((session, should_end)) => {
                    let participant_count = if should_end { 0 } else {
                        mgr.active_participant_count(&session_id)
                    };
                    let channel = session.channel.clone();

                    if let Some(s) = mgr.get(&session_id) {
                        state.with_db(|db| db.save_av_session(s));
                    }
                    drop(mgr);

                    if should_end {
                        broadcast_av_state(state, target, &session_id, "ended", &nick, 0, "");
                        broadcast_av_s2s(state, "ended", &session_id, channel.as_deref(), &did, &nick, None, Some(&did));
                        // Close iroh-live room and bridge
                        #[cfg(feature = "av-native")]
                        { state.av_bridges.lock().remove(&session_id); }
                        let backend = state.av_media.lock().clone();
                        let sid = session_id.clone();
                        tokio::spawn(async move {
                            if let Some(backend) = backend.as_ref() {
                                let _ = crate::av_media::MediaBackend::close_room(backend.as_ref(), &sid).await;
                            }
                        });
                    } else {
                        broadcast_av_state(state, target, &session_id, "left", &nick, participant_count, "");
                        broadcast_av_s2s(state, "left", &session_id, channel.as_deref(), &did, &nick, None, None);
                    }

                    tracing::info!(session_id = %session_id, did = %did, ended = should_end, "AV session left");
                }
                Err(e) => {
                    let reply = Message::from_server(
                        &state.server_name,
                        "NOTICE",
                        vec![&nick, &format!("Cannot leave session: {e}")],
                    );
                    send_to(state, &conn.id, format!("{reply}\r\n"));
                }
            }
        }

        "+freeq.at/av-end" => {
            let mgr = state.av_sessions.lock();
            let can_end = mgr.can_end_session(&session_id, &did)
                || state.server_opers.lock().contains(&conn.id);
            // Also check if user is channel op
            let is_chan_op = if target.starts_with('#') || target.starts_with('&') {
                let channels = state.channels.lock();
                channels.get(target).map(|ch| ch.ops.contains(&conn.id) || ch.did_ops.contains(&did)).unwrap_or(false)
            } else {
                false
            };
            drop(mgr);

            if !can_end && !is_chan_op {
                let reply = Message::from_server(
                    &state.server_name,
                    "NOTICE",
                    vec![&nick, "Only the session host or channel ops can end a session"],
                );
                send_to(state, &conn.id, format!("{reply}\r\n"));
                return;
            }

            let mut mgr = state.av_sessions.lock();
            match mgr.end_session(&session_id, Some(&did)) {
                Ok(session) => {
                    let channel = session.channel.clone();
                    state.with_db(|db| db.save_av_session(&session));
                    drop(mgr);

                    broadcast_av_state(state, target, &session_id, "ended", &nick, 0, "");
                    broadcast_av_s2s(state, "ended", &session_id, channel.as_deref(), &did, &nick, None, Some(&did));

                    // Close iroh-live room and bridge
                    {
                        #[cfg(feature = "av-native")]
                        { state.av_bridges.lock().remove(&session_id); }
                        let backend = state.av_media.lock().clone();
                        let sid = session_id.clone();
                        tokio::spawn(async move {
                            if let Some(backend) = backend.as_ref() {
                                if let Err(e) = crate::av_media::MediaBackend::close_room(backend.as_ref(), &sid).await {
                                    tracing::warn!(session = %sid, error = %e, "Failed to close iroh-live room");
                                }
                            }
                        });
                    }

                    tracing::info!(session_id = %session_id, did = %did, "AV session ended");
                }
                Err(e) => {
                    drop(mgr);
                    let reply = Message::from_server(
                        &state.server_name,
                        "NOTICE",
                        vec![&nick, &format!("Cannot end session: {e}")],
                    );
                    send_to(state, &conn.id, format!("{reply}\r\n"));
                }
            }
        }

        _ => {
            tracing::debug!(tag = %av_tag, "Unknown AV tag — ignored");
        }
    }
}

/// Broadcast a plain NOTICE to all channel members (used for AV session events from S2S).
pub fn broadcast_av_notice(state: &Arc<SharedState>, channel: &str, text: &str) {
    let notice = Message::from_server(&state.server_name, "NOTICE", vec![channel, text]);
    let line = format!("{notice}\r\n");
    let members: Vec<String> = state
        .channels
        .lock()
        .get(channel)
        .map(|ch| ch.members.iter().cloned().collect())
        .unwrap_or_default();
    let conns = state.connections.lock();
    for member in &members {
        if let Some(tx) = conns.get(member) {
            let _ = tx.try_send(line.clone());
        }
    }
}

/// Broadcast AV session state to all channel members via TAGMSG (public for disconnect cleanup).
pub fn broadcast_av_state_pub(
    state: &Arc<SharedState>,
    target: &str,
    session_id: &str,
    action: &str,
    actor_nick: &str,
    participant_count: usize,
    title: &str,
) {
    broadcast_av_state(state, target, session_id, action, actor_nick, participant_count, title);
}

/// Broadcast AV session state to all channel members via TAGMSG.
fn broadcast_av_state(
    state: &Arc<SharedState>,
    target: &str,
    session_id: &str,
    action: &str,
    actor_nick: &str,
    participant_count: usize,
    title: &str,
) {
    let mut tags = std::collections::HashMap::new();
    tags.insert("+freeq.at/av-state".to_string(), action.to_string());
    tags.insert("+freeq.at/av-id".to_string(), session_id.to_string());
    tags.insert("+freeq.at/av-participants".to_string(), participant_count.to_string());
    tags.insert("+freeq.at/av-actor".to_string(), actor_nick.to_string());
    if !title.is_empty() {
        tags.insert("+freeq.at/av-title".to_string(), title.to_string());
    }
    let time_tag = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
    tags.insert("time".to_string(), time_tag);

    let tag_msg = super::super::irc::Message {
        tags,
        prefix: Some(state.server_name.clone()),
        command: "TAGMSG".to_string(),
        params: vec![target.to_string()],
    };
    let line = format!("{tag_msg}\r\n");

    // Also send a human-readable NOTICE for clients that don't parse tags
    let notice_text = match action {
        "started" => format!("{actor_nick} started a voice session{}", if title.is_empty() { String::new() } else { format!(": {title}") }),
        "joined" => format!("{actor_nick} joined the voice session ({participant_count} participants)"),
        "left" => format!("{actor_nick} left the voice session ({participant_count} participants)"),
        "ended" => format!("{actor_nick} ended the voice session"),
        _ => return,
    };
    let notice = Message::from_server(
        &state.server_name,
        "NOTICE",
        vec![target, &notice_text],
    );
    let notice_line = format!("{notice}\r\n");

    // Broadcast to channel members
    if target.starts_with('#') || target.starts_with('&') {
        let members: Vec<String> = state
            .channels
            .lock()
            .get(target)
            .map(|ch| ch.members.iter().cloned().collect())
            .unwrap_or_default();

        let tag_caps = state.cap_message_tags.lock();
        let conns = state.connections.lock();
        for member in &members {
            if let Some(tx) = conns.get(member) {
                if tag_caps.contains(member) {
                    let _ = tx.try_send(line.clone());
                } else {
                    let _ = tx.try_send(notice_line.clone());
                }
            }
        }
    }
}

/// Broadcast AV session event via S2S federation.
fn broadcast_av_s2s(
    state: &Arc<SharedState>,
    action: &str,
    session_id: &str,
    channel: Option<&str>,
    did: &str,
    nick: &str,
    title: Option<&str>,
    ended_by: Option<&str>,
) {
    let s2s = state.s2s_manager.lock();
    let Some(ref mgr) = *s2s else { return };

    let event_id = mgr.next_event_id();
    let origin = mgr.server_id.clone();

    let msg = match action {
        "created" => crate::s2s::S2sMessage::AvSessionCreated {
            event_id,
            session_id: session_id.to_string(),
            channel: channel.unwrap_or("").to_string(),
            created_by_did: did.to_string(),
            created_by_nick: nick.to_string(),
            title: title.map(|s| s.to_string()),
            iroh_ticket: None, // TODO: add when iroh-live is integrated
            origin,
        },
        "joined" => crate::s2s::S2sMessage::AvSessionJoined {
            event_id,
            session_id: session_id.to_string(),
            did: did.to_string(),
            nick: nick.to_string(),
            origin,
        },
        "left" => crate::s2s::S2sMessage::AvSessionLeft {
            event_id,
            session_id: session_id.to_string(),
            did: did.to_string(),
            origin,
        },
        "ended" => crate::s2s::S2sMessage::AvSessionEnded {
            event_id,
            session_id: session_id.to_string(),
            ended_by: ended_by.map(|s| s.to_string()),
            origin,
        },
        _ => return,
    };

    mgr.broadcast(msg);
}

#[cfg(test)]
mod av_dispatch_tests {
    //! State matrix cell #16: multiple av-* tags on one TAGMSG must
    //! dispatch on the *action* tag, not whichever parameter tag HashMap
    //! iteration happens to return first. Pre-fix (commit 8b13ccd) the
    //! code did `tags.keys().find(|k| k.starts_with("+freeq.at/av-"))`,
    //! which was non-deterministic. Now we have an explicit AV_ACTIONS
    //! list and dispatch on the first matching action tag. This test
    //! pins the order so a future "be helpful and reorder" refactor
    //! doesn't accidentally re-introduce the bug.
    use std::collections::HashMap;

    /// Mirrors the dispatch order in `process_tagmsg`. Keep in sync.
    const AV_ACTIONS: &[&str] = &[
        "+freeq.at/av-start",
        "+freeq.at/av-join",
        "+freeq.at/av-leave",
        "+freeq.at/av-end",
    ];

    fn dispatch(tags: &HashMap<String, String>) -> Option<&'static &'static str> {
        AV_ACTIONS.iter().find(|tag| tags.contains_key(**tag))
    }

    #[test]
    fn av_join_with_id_and_instance_dispatches_to_join() {
        let mut tags: HashMap<String, String> = HashMap::new();
        tags.insert("+freeq.at/av-join".into(), String::new());
        tags.insert("+freeq.at/av-id".into(), "sess-1".into());
        tags.insert("+freeq.at/av-instance".into(), "abcd1234".into());
        assert_eq!(dispatch(&tags), Some(&"+freeq.at/av-join"));
    }

    #[test]
    fn av_start_with_title_and_instance_dispatches_to_start() {
        let mut tags: HashMap<String, String> = HashMap::new();
        tags.insert("+freeq.at/av-start".into(), String::new());
        tags.insert("+freeq.at/av-title".into(), "standup".into());
        tags.insert("+freeq.at/av-instance".into(), "abcd1234".into());
        assert_eq!(dispatch(&tags), Some(&"+freeq.at/av-start"));
    }

    #[test]
    fn av_leave_with_id_dispatches_to_leave() {
        let mut tags: HashMap<String, String> = HashMap::new();
        tags.insert("+freeq.at/av-leave".into(), String::new());
        tags.insert("+freeq.at/av-id".into(), "sess-1".into());
        assert_eq!(dispatch(&tags), Some(&"+freeq.at/av-leave"));
    }

    #[test]
    fn av_id_alone_with_no_action_does_not_dispatch() {
        let mut tags: HashMap<String, String> = HashMap::new();
        tags.insert("+freeq.at/av-id".into(), "sess-1".into());
        tags.insert("+freeq.at/av-instance".into(), "abcd1234".into());
        assert_eq!(dispatch(&tags), None,
                   "parameter-only TAGMSG must not be treated as an action");
    }

    #[test]
    fn av_signal_is_not_an_action() {
        // av-signal is a relay tag (WebRTC payload) — must NOT be
        // consumed as an action.
        let mut tags: HashMap<String, String> = HashMap::new();
        tags.insert("+freeq.at/av-signal".into(), "payload".into());
        assert_eq!(dispatch(&tags), None);
    }

    #[test]
    fn priority_order_start_then_join_then_leave_then_end() {
        // The order matters: if a (malformed) message had multiple action
        // tags, we must dispatch deterministically. Tests pin the order.
        let mut both = HashMap::new();
        both.insert("+freeq.at/av-start".into(), String::new());
        both.insert("+freeq.at/av-join".into(), String::new());
        assert_eq!(dispatch(&both), Some(&"+freeq.at/av-start"),
                   "av-start wins over av-join when both are present");

        let mut both = HashMap::new();
        both.insert("+freeq.at/av-join".into(), String::new());
        both.insert("+freeq.at/av-leave".into(), String::new());
        assert_eq!(dispatch(&both), Some(&"+freeq.at/av-join"));

        let mut both = HashMap::new();
        both.insert("+freeq.at/av-leave".into(), String::new());
        both.insert("+freeq.at/av-end".into(), String::new());
        assert_eq!(dispatch(&both), Some(&"+freeq.at/av-leave"));
    }
}
