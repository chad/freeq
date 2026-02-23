#![allow(clippy::too_many_arguments)]
//! Channel operations: join, part, mode, topic, kick, invite, names, list.

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::server::SharedState;
use super::Connection;
use super::helpers::{s2s_broadcast, s2s_broadcast_mode, s2s_next_event_id, broadcast_to_channel, make_standard_join, make_extended_join};

pub(super) fn handle_join(
    conn: &Connection,
    channel: &str,
    supplied_key: Option<&str>,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick.as_deref().unwrap();
    let hostmask = conn.hostmask();
    let did = conn.authenticated_did.as_deref();

    // A channel is "new" only if it doesn't exist at all — not locally,
    // not via S2S. If remote members are present (from S2S sync), the
    // channel already exists on the federation and the joining user
    // should NOT get auto-ops (unless they have DID-based authority).
    let is_new_channel = {
        let channels = state.channels.lock().unwrap();
        match channels.get(channel) {
            None => true,
            Some(ch) => {
                // Channel entry exists but has nobody and no persistent state —
                // treat as effectively new (e.g. leftover from cleanup)
                ch.members.is_empty()
                    && ch.remote_members.is_empty()
                    && ch.founder_did.is_none()
                    && ch.topic.is_none()
                    && ch.ops.is_empty()
            }
        }
    };

    if !is_new_channel {
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(channel) {
            // Check channel key (+k)
            if let Some(ref key) = ch.key
                && supplied_key != Some(key.as_str()) {
                    let reply = Message::from_server(
                        server_name,
                        irc::ERR_BADCHANNELKEY,
                        vec![nick, channel, "Cannot join channel (+k)"],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                    return;
                }
            // Check bans
            if ch.is_banned(&hostmask, did) {
                let reply = Message::from_server(
                    server_name,
                    irc::ERR_BANNEDFROMCHAN,
                    vec![nick, channel, "Cannot join channel (+b)"],
                );
                send(state, session_id, format!("{reply}\r\n"));
                return;
            }
            // Check invite-only
            if ch.invite_only {
                let has_invite = ch.invites.contains(session_id)
                    || did.is_some_and(|d| ch.invites.contains(d))
                    || ch.invites.contains(&format!("nick:{nick}"));
                if !has_invite {
                    let reply = Message::from_server(
                        server_name,
                        irc::ERR_INVITEONLYCHAN,
                        vec![nick, channel, "Cannot join channel (+i)"],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                    return;
                }
                // Consume the invite (all forms: session, DID, nick)
                drop(channels);
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel) {
                    ch.invites.remove(session_id);
                    if let Some(d) = did {
                        ch.invites.remove(d);
                    }
                    ch.invites.remove(&format!("nick:{nick}"));
                }
            }
        }
    }

    // ─── Policy check ─────────────────────────────────────────────────
    // If the channel has a policy, check if the user has a valid attestation.
    // Channels without policies are open (backwards compatible).
    // `policy_role` captures the attestation role for mode mapping after join.
    let mut policy_role: Option<String> = None;
    if let Some(ref engine) = state.policy_engine {
        if let Ok(Some(_policy)) = engine.get_policy(channel) {
            // Channel has a policy — user must have a valid attestation
            match did {
                Some(user_did) => {
                    match engine.check_membership(channel, user_did) {
                        Ok(Some(attestation)) => {
                            // Valid attestation — allow join, capture role
                            policy_role = Some(attestation.role.clone());
                        }
                        Ok(None) => {
                            // No attestation — reject with informative message
                            let reply = Message::from_server(
                                server_name,
                                "477", // ERR_NEEDREGGEDNICK (repurposed: need policy acceptance)
                                vec![
                                    nick,
                                    channel,
                                    "This channel requires policy acceptance — use POLICY <channel> ACCEPT",
                                ],
                            );
                            send(state, session_id, format!("{reply}\r\n"));
                            return;
                        }
                        Err(e) => {
                            tracing::warn!(channel, did = user_did, error = %e, "Policy check failed");
                            // Fail-open on engine errors (don't break IRC)
                        }
                    }
                }
                None => {
                    // Guest user (no DID) — check if policy allows unauthenticated join
                    // For now, guests cannot join policy-gated channels
                    let reply = Message::from_server(
                        server_name,
                        "477",
                        vec![
                            nick,
                            channel,
                            "This channel requires authentication — sign in to join",
                        ],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                    return;
                }
            }
        }
    }

    {
        let mut channels = state.channels.lock().unwrap();
        let ch = channels.entry(channel.to_string()).or_default();
        ch.members.insert(session_id.to_string());
        // NOTE: Presence is NOT in CRDT (avoids ghost users on crash).
        // It's tracked by S2S events + periodic resync only.

        if is_new_channel {
            // New channel: set founder if authenticated
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            ch.created_at = now;
            if let Some(d) = did {
                ch.founder_did = Some(d.to_string());
                ch.did_ops.insert(d.to_string());
                // CRDT updates (async) — spawn to avoid blocking
                let state_c = Arc::clone(state);
                let channel_c = channel.to_string();
                let did_c = d.to_string();
                tokio::spawn(async move {
                    state_c.crdt_set_founder(&channel_c, &did_c).await;
                    state_c.crdt_grant_op(&channel_c, &did_c, None).await;
                });
            }
            ch.ops.insert(session_id.to_string());
            // Default channel modes: +nt (standard IRC behavior)
            // +n = no external messages (only members can send)
            // +t = only ops can change topic
            ch.no_ext_msg = true;
            ch.topic_locked = true;
            let ch_clone = ch.clone();
            drop(channels);
            state.with_db(|db| db.save_channel(channel, &ch_clone));
        } else {
            // Existing channel: auto-op if user's DID has persistent ops
            let should_op = did.is_some_and(|d| {
                ch.founder_did.as_deref() == Some(d) || ch.did_ops.contains(d)
            });
            // Auto-op the first user to join a truly empty channel (e.g. after
            // server restart when the channel was loaded from DB with no members).
            // This prevents orphaned channels where nobody has ops.
            // BUT: if there are remote members (from S2S), the channel isn't
            // orphaned — someone else already has ops on another server.
            let has_any_ops = !ch.ops.is_empty()
                || ch.remote_members.values().any(|rm| rm.is_op);
            let is_truly_empty = ch.members.len() == 1
                && ch.remote_members.is_empty()
                && !has_any_ops;
            if should_op || is_truly_empty {
                ch.ops.insert(session_id.to_string());
            }
        }
    }

    // ─── Policy role → IRC mode mapping ────────────────────────────────
    // If user joined via policy and has an elevated role, grant IRC modes.
    if let Some(ref role) = policy_role {
        let mut channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get_mut(channel) {
            match role.as_str() {
                "op" | "admin" | "owner" | "moderator" => {
                    ch.ops.insert(session_id.to_string());
                    if let Some(d) = did {
                        ch.did_ops.insert(d.to_string());
                    }
                }
                "voice" | "voiced" | "speaker" => {
                    ch.voiced.insert(session_id.to_string());
                }
                _ => {} // "member" gets no special mode
            }
        }
    }

    // Broadcast MODE +o to existing channel members if the joiner was auto-opped
    {
        let is_op = state.channels.lock().unwrap()
            .get(channel)
            .map(|ch| ch.ops.contains(session_id))
            .unwrap_or(false);
        if is_op {
            let mode_msg = format!(":{server_name} MODE {channel} +o {nick}\r\n");
            let channels = state.channels.lock().unwrap();
            if let Some(ch) = channels.get(channel) {
                let members: Vec<String> = ch.members.iter().cloned().collect();
                drop(channels);
                let conns = state.connections.lock().unwrap();
                for member_session in &members {
                    if let Some(tx) = conns.get(member_session) {
                        let _ = tx.try_send(mode_msg.clone());
                    }
                }
            }
        }
    }

    // Plugin on_join hook
    state.plugin_manager.on_join(&crate::plugin::JoinEvent {
        nick: nick.to_string(),
        channel: channel.to_string(),
        did: did.map(|d| d.to_string()),
        session_id: session_id.to_string(),
        is_new_channel,
    });

    let std_join = make_standard_join(&hostmask, channel);
    let realname = conn.realname.as_deref().unwrap_or(nick);
    let ext_join = make_extended_join(&hostmask, channel, did, realname);

    let members: Vec<String> = state
        .channels
        .lock()
        .unwrap()
        .get(channel)
        .map(|ch| ch.members.iter().cloned().collect())
        .unwrap_or_default();

    let ext_set = state.cap_extended_join.lock().unwrap();
    let conns = state.connections.lock().unwrap();
    for member_session in &members {
        if let Some(tx) = conns.get(member_session) {
            if ext_set.contains(member_session) {
                let _ = tx.try_send(ext_join.clone());
            } else {
                let _ = tx.try_send(std_join.clone());
            }
        }
    }
    drop(conns);
    drop(ext_set);

    // Broadcast JOIN to S2S peers
    let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
    // Look up AT handle for the joining user
    let handle = state.session_handles.lock().unwrap().get(session_id).cloned();
    let user_is_op = state.channels.lock().unwrap()
        .get(channel)
        .map(|ch| ch.ops.contains(session_id))
        .unwrap_or(false);
    s2s_broadcast(state, crate::s2s::S2sMessage::Join {
        event_id: s2s_next_event_id(state),
        nick: nick.to_string(),
        channel: channel.to_string(),
        did: did.map(|d| d.to_string()),
        handle,
        is_op: user_is_op,
        origin: origin.clone(),
    });

    // If this was a new channel creation, broadcast founder info
    if is_new_channel {
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(channel) {
            s2s_broadcast(state, crate::s2s::S2sMessage::ChannelCreated {
                event_id: s2s_next_event_id(state),
                channel: channel.to_string(),
                founder_did: ch.founder_did.clone(),
                did_ops: ch.did_ops.iter().cloned().collect(),
                created_at: ch.created_at,
                origin: origin.clone(),
            });
        }
    }

    // Send topic if set (332 + 333)
    {
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(channel)
            && let Some(ref topic) = ch.topic {
                let rpl_topic = Message::from_server(
                    server_name,
                    irc::RPL_TOPIC,
                    vec![nick, channel, &topic.text],
                );
                send(state, session_id, format!("{rpl_topic}\r\n"));

                let rpl_topicwhotime = Message::from_server(
                    server_name,
                    irc::RPL_TOPICWHOTIME,
                    vec![nick, channel, &topic.set_by, &topic.set_at.to_string()],
                );
                send(state, session_id, format!("{rpl_topicwhotime}\r\n"));
            }
    }

    // Replay recent message history with server-time + batch when supported
    {
        let has_tags_cap = state.cap_message_tags.lock().unwrap().contains(session_id);
        let has_time_cap = state.cap_server_time.lock().unwrap().contains(session_id);
        let has_batch_cap = state.cap_batch.lock().unwrap().contains(session_id);
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(channel)
            && !ch.history.is_empty()
        {
            // Start batch if client supports it
            let batch_id = format!("hist{}", session_id.len());
            if has_batch_cap {
                let batch_start = format!(
                    ":{server_name} BATCH +{batch_id} chathistory {channel}\r\n"
                );
                send(state, session_id, batch_start);
            }

            for hist in &ch.history {
                let mut msg_tags = if has_tags_cap { hist.tags.clone() } else { std::collections::HashMap::new() };

                // Add msgid tag if available
                if has_tags_cap {
                    if let Some(ref mid) = hist.msgid {
                        msg_tags.insert("msgid".to_string(), mid.clone());
                    }
                }

                // Add server-time tag
                if has_time_cap {
                    let ts = chrono::DateTime::from_timestamp(hist.timestamp as i64, 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%dT%H:%M:%S.000Z")
                        .to_string();
                    msg_tags.insert("time".to_string(), ts);
                }

                // Add batch tag
                if has_batch_cap {
                    msg_tags.insert("batch".to_string(), batch_id.clone());
                }

                if !msg_tags.is_empty() && has_tags_cap {
                    let tag_msg = irc::Message {
                        tags: msg_tags,
                        prefix: Some(hist.from.clone()),
                        command: "PRIVMSG".to_string(),
                        params: vec![channel.to_string(), hist.text.clone()],
                    };
                    send(state, session_id, format!("{tag_msg}\r\n"));
                } else {
                    let line = format!(":{} PRIVMSG {} :{}\r\n", hist.from, channel, hist.text);
                    send(state, session_id, line);
                }
            }

            // End batch
            if has_batch_cap {
                let batch_end = format!(":{server_name} BATCH -{batch_id}\r\n");
                send(state, session_id, batch_end);
            }
        }
    }

    let nick_list: Vec<String> = {
        let channels = state.channels.lock().unwrap();
        let (member_sessions, remote_members, ops, voiced) = match channels.get(channel) {
            Some(ch) => (ch.members.clone(), ch.remote_members.clone(), ch.ops.clone(), ch.voiced.clone()),
            None => Default::default(),
        };
        drop(channels);
        // Local members: look up nick from session ID
        let nicks = state.nick_to_session.lock().unwrap();
        let reverse: std::collections::HashMap<&String, &String> =
            nicks.iter().map(|(n, s)| (s, n)).collect();
        let mut list: Vec<String> = member_sessions
            .iter()
            .filter_map(|s| {
                reverse.get(s).map(|n| {
                    let prefix = if ops.contains(s) {
                        "@"
                    } else if voiced.contains(s) {
                        "+"
                    } else {
                        ""
                    };
                    format!("{prefix}{n}")
                })
            })
            .collect();
        // Remote members from S2S peers (with @ prefix if op on home server or DID-based)
        let channels_lock = state.channels.lock().unwrap();
        let ch_state = channels_lock.get(channel);
        for (nick, rm) in &remote_members {
            let is_op = rm.is_op || rm.did.as_ref().is_some_and(|d| {
                ch_state.is_some_and(|ch| {
                    ch.founder_did.as_deref() == Some(d.as_str()) || ch.did_ops.contains(d)
                })
            });
            let prefix = if is_op { "@" } else { "" };
            list.push(format!("{prefix}{nick}"));
        }
        drop(channels_lock);
        list
    };

    let names = Message::from_server(
        server_name,
        irc::RPL_NAMREPLY,
        vec![nick, "=", channel, &nick_list.join(" ")],
    );
    let end_names = Message::from_server(
        server_name,
        irc::RPL_ENDOFNAMES,
        vec![nick, channel, "End of /NAMES list"],
    );
    send(state, session_id, format!("{names}\r\n"));
    send(state, session_id, format!("{end_names}\r\n"));
}


pub(super) fn handle_mode(
    conn: &Connection,
    channel: &str,
    mode_str: Option<&str>,
    mode_arg: Option<&str>,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();

    // Verify user is in the channel
    let in_channel = state
        .channels
        .lock()
        .unwrap()
        .get(channel)
        .map(|ch| ch.members.contains(session_id))
        .unwrap_or(false);

    if !in_channel {
        let reply = Message::from_server(
            server_name,
            irc::ERR_NOTONCHANNEL,
            vec![nick, channel, "You're not on that channel"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    let Some(mode_str) = mode_str else {
        // Query channel modes
        let channels = state.channels.lock().unwrap();
        let modes = if let Some(ch) = channels.get(channel) {
            let mut m = String::from("+");
            if ch.no_ext_msg { m.push('n'); }
            if ch.topic_locked { m.push('t'); }
            if ch.invite_only { m.push('i'); }
            if ch.moderated { m.push('m'); }
            if ch.key.is_some() { m.push('k'); }
            m
        } else {
            "+".to_string()
        };
        let reply = Message::from_server(
            server_name,
            irc::RPL_CHANNELMODEIS,
            vec![nick, channel, &modes],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    };

    // Only ops can change modes
    let is_op = state
        .channels
        .lock()
        .unwrap()
        .get(channel)
        .map(|ch| ch.ops.contains(session_id))
        .unwrap_or(false);

    if !is_op {
        let reply = Message::from_server(
            server_name,
            irc::ERR_CHANOPRIVSNEEDED,
            vec![nick, channel, "You're not channel operator"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    // Parse mode string: +o, -o, +v, -v, +t, -t
    let mut adding = true;
    for ch in mode_str.chars() {
        match ch {
            '+' => adding = true,
            '-' => adding = false,
            'o' | 'v' => {
                let Some(target_nick) = mode_arg else {
                    let reply = Message::from_server(
                        server_name,
                        irc::ERR_NEEDMOREPARAMS,
                        vec![nick, "MODE", "Not enough parameters"],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                    return;
                };

                // Resolve target via federated channel roster (local + remote)
                use super::helpers::{resolve_channel_target, ChannelTarget};
                match resolve_channel_target(state, channel, target_nick) {
                    ChannelTarget::Local { session_id: target_session } => {
                        // Apply the mode locally
                        {
                            let mut channels = state.channels.lock().unwrap();
                            if let Some(chan) = channels.get_mut(channel) {
                                let set = if ch == 'o' { &mut chan.ops } else { &mut chan.voiced };
                                if adding {
                                    set.insert(target_session.clone());
                                } else {
                                    set.remove(&target_session);
                                }

                                // DID-based persistent ops: +o/-o on an authenticated
                                // user also updates did_ops, so ops survive reconnects
                                // and work across S2S servers.
                                if ch == 'o' {
                                    let target_did = state.session_dids.lock().unwrap()
                                        .get(&target_session).cloned();
                                    if let Some(did) = target_did {
                                        // Don't allow de-opping the founder
                                        if !adding && chan.founder_did.as_deref() == Some(&did) {
                                            // Silently ignore — founder can't be de-opped
                                        } else if adding {
                                            chan.did_ops.insert(did.clone());
                                            // CRDT grant so it propagates across federation
                                            let granter_did = state.session_dids.lock().unwrap()
                                                .get(session_id).cloned();
                                            let state_clone = Arc::clone(state);
                                            let channel_name = channel.to_string();
                                            tokio::spawn(async move {
                                                state_clone.crdt_grant_op(&channel_name, &did, granter_did.as_deref()).await;
                                                state_clone.crdt_broadcast_sync().await;
                                            });
                                        } else {
                                            chan.did_ops.remove(&did);
                                            let state_clone = Arc::clone(state);
                                            let channel_name = channel.to_string();
                                            let did_clone = did.clone();
                                            tokio::spawn(async move {
                                                state_clone.crdt_revoke_op(&channel_name, &did_clone).await;
                                                state_clone.crdt_broadcast_sync().await;
                                            });
                                        }
                                        // Persist the updated DID ops
                                        let ch_clone = chan.clone();
                                        let channel_name = channel.to_string();
                                        drop(channels);
                                        state.with_db(|db| db.save_channel(&channel_name, &ch_clone));
                                    }
                                }
                            }
                        }

                        // Broadcast mode change to local channel + S2S
                        let sign = if adding { "+" } else { "-" };
                        let hostmask = conn.hostmask();
                        let mode_msg = format!(":{hostmask} MODE {channel} {sign}{ch} {target_nick}\r\n");
                        broadcast_to_channel(state, channel, &mode_msg);
                        s2s_broadcast_mode(state, conn, channel, &format!("{sign}{ch}"), Some(target_nick));
                    }

                    ChannelTarget::Remote(rm) => {
                        // Apply ephemeral op/voice on the remote member locally
                        {
                            let mut channels = state.channels.lock().unwrap();
                            if let Some(chan) = channels.get_mut(channel) {
                                if ch == 'o' {
                                    if let Some(remote) = chan.remote_members.get_mut(target_nick) {
                                        remote.is_op = adding;
                                    }
                                }
                                // +v: no is_voiced on RemoteMember, but we still
                                // broadcast the mode so the remote server can apply it.
                            }
                        }

                        // If the user has a DID, also update did_ops for persistence + CRDT
                        if ch == 'o' {
                            if let Some(ref did) = rm.did {
                                {
                                    let mut channels = state.channels.lock().unwrap();
                                    if let Some(chan) = channels.get_mut(channel) {
                                        if !adding && chan.founder_did.as_deref() == Some(did.as_str()) {
                                            // Founder can't be de-opped
                                        } else if adding {
                                            chan.did_ops.insert(did.clone());
                                        } else {
                                            chan.did_ops.remove(did);
                                        }
                                        let ch_clone = chan.clone();
                                        let channel_name = channel.to_string();
                                        drop(channels);
                                        state.with_db(|db| db.save_channel(&channel_name, &ch_clone));
                                    }
                                }

                                // CRDT propagation (persistent)
                                let granter_did = state.session_dids.lock().unwrap()
                                    .get(session_id).cloned();
                                let state_clone = Arc::clone(state);
                                let channel_name = channel.to_string();
                                let did_clone = did.clone();
                                tokio::spawn(async move {
                                    if adding {
                                        state_clone.crdt_grant_op(&channel_name, &did_clone, granter_did.as_deref()).await;
                                    } else {
                                        state_clone.crdt_revoke_op(&channel_name, &did_clone).await;
                                    }
                                    state_clone.crdt_broadcast_sync().await;
                                });
                            }
                            // Guest without DID: ephemeral op still applied above
                            // (is_op flag on remote_members). Won't survive reconnect
                            // but works for the session — same as regular IRC.
                        }

                        // Broadcast mode change to local channel + S2S
                        let sign = if adding { "+" } else { "-" };
                        let hostmask = conn.hostmask();
                        let mode_msg = format!(":{hostmask} MODE {channel} {sign}{ch} {target_nick}\r\n");
                        broadcast_to_channel(state, channel, &mode_msg);
                        s2s_broadcast_mode(state, conn, channel, &format!("{sign}{ch}"), Some(target_nick));
                    }

                    ChannelTarget::NotPresent => {
                        let reply = Message::from_server(
                            server_name,
                            irc::ERR_USERNOTINCHANNEL,
                            vec![nick, target_nick, channel, "They aren't on that channel"],
                        );
                        send(state, session_id, format!("{reply}\r\n"));
                        return;
                    }
                }
            }
            'b' => {
                use crate::server::BanEntry;

                if !adding && mode_arg.is_none() {
                    // -b with no arg is invalid, ignore
                    return;
                }

                if adding && mode_arg.is_none() {
                    // +b with no arg: list bans
                    let channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get(channel) {
                        for ban in &chan.bans {
                            let reply = Message::from_server(
                                server_name,
                                irc::RPL_BANLIST,
                                vec![nick, channel, &ban.mask, &ban.set_by, &ban.set_at.to_string()],
                            );
                            send(state, session_id, format!("{reply}\r\n"));
                        }
                    }
                    let end = Message::from_server(
                        server_name,
                        irc::RPL_ENDOFBANLIST,
                        vec![nick, channel, "End of channel ban list"],
                    );
                    send(state, session_id, format!("{end}\r\n"));
                    return;
                }

                let mask = mode_arg.unwrap();
                if adding {
                    let entry = BanEntry::new(mask.to_string(), conn.hostmask());
                    let mut channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get_mut(channel) {
                        // Don't duplicate
                        if !chan.bans.iter().any(|b| b.mask == mask) {
                            chan.bans.push(entry.clone());
                            drop(channels);
                            state.with_db(|db| db.add_ban(channel, &entry));
                        }
                    }
                } else {
                    let mut channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get_mut(channel) {
                        chan.bans.retain(|b| b.mask != mask);
                    }
                    drop(channels);
                    state.with_db(|db| db.remove_ban(channel, mask));
                }

                let sign = if adding { "+" } else { "-" };
                let hostmask = conn.hostmask();
                let mode_msg = format!(":{hostmask} MODE {channel} {sign}b {mask}\r\n");
                broadcast_to_channel(state, channel, &mode_msg);
            }
            'i' => {
                {
                    let mut channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get_mut(channel) {
                        chan.invite_only = adding;
                        if !adding {
                            chan.invites.clear();
                        }
                        let ch_clone = chan.clone();
                        drop(channels);
                        state.with_db(|db| db.save_channel(channel, &ch_clone));
                    }
                }
                let sign = if adding { "+" } else { "-" };
                let hostmask = conn.hostmask();
                let mode_msg = format!(":{hostmask} MODE {channel} {sign}i\r\n");
                broadcast_to_channel(state, channel, &mode_msg);
                s2s_broadcast_mode(state, conn, channel, &format!("{sign}i"), None);
            }
            't' => {
                {
                    let mut channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get_mut(channel) {
                        chan.topic_locked = adding;
                        let ch_clone = chan.clone();
                        drop(channels);
                        state.with_db(|db| db.save_channel(channel, &ch_clone));
                    }
                }
                let sign = if adding { "+" } else { "-" };
                let hostmask = conn.hostmask();
                let mode_msg = format!(":{hostmask} MODE {channel} {sign}t\r\n");
                broadcast_to_channel(state, channel, &mode_msg);
                s2s_broadcast_mode(state, conn, channel, &format!("{sign}t"), None);
            }
            'k' => {
                if adding {
                    let Some(key) = mode_arg else {
                        let reply = Message::from_server(
                            server_name,
                            irc::ERR_NEEDMOREPARAMS,
                            vec![nick, "MODE", "Not enough parameters"],
                        );
                        send(state, session_id, format!("{reply}\r\n"));
                        return;
                    };
                    {
                        let mut channels = state.channels.lock().unwrap();
                        if let Some(chan) = channels.get_mut(channel) {
                            chan.key = Some(key.to_string());
                            let ch_clone = chan.clone();
                            drop(channels);
                            state.with_db(|db| db.save_channel(channel, &ch_clone));
                        }
                    }
                    let hostmask = conn.hostmask();
                    let mode_msg = format!(":{hostmask} MODE {channel} +k {key}\r\n");
                    broadcast_to_channel(state, channel, &mode_msg);
                    s2s_broadcast_mode(state, conn, channel, "+k", Some(key));
                } else {
                    let old_key = {
                        let mut channels = state.channels.lock().unwrap();
                        if let Some(chan) = channels.get_mut(channel) {
                            let k = chan.key.take();
                            let ch_clone = chan.clone();
                            drop(channels);
                            state.with_db(|db| db.save_channel(channel, &ch_clone));
                            k
                        } else {
                            None
                        }
                    };
                    if let Some(key) = old_key {
                        let hostmask = conn.hostmask();
                        let mode_msg = format!(":{hostmask} MODE {channel} -k {key}\r\n");
                        broadcast_to_channel(state, channel, &mode_msg);
                        s2s_broadcast_mode(state, conn, channel, "-k", Some(&key));
                    }
                }
            }
            'n' => {
                {
                    let mut channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get_mut(channel) {
                        chan.no_ext_msg = adding;
                        let ch_clone = chan.clone();
                        drop(channels);
                        state.with_db(|db| db.save_channel(channel, &ch_clone));
                    }
                }
                let sign = if adding { "+" } else { "-" };
                let hostmask = conn.hostmask();
                let mode_msg = format!(":{hostmask} MODE {channel} {sign}n\r\n");
                broadcast_to_channel(state, channel, &mode_msg);
                s2s_broadcast_mode(state, conn, channel, &format!("{sign}n"), None);
            }
            'm' => {
                {
                    let mut channels = state.channels.lock().unwrap();
                    if let Some(chan) = channels.get_mut(channel) {
                        chan.moderated = adding;
                        let ch_clone = chan.clone();
                        drop(channels);
                        state.with_db(|db| db.save_channel(channel, &ch_clone));
                    }
                }
                let sign = if adding { "+" } else { "-" };
                let hostmask = conn.hostmask();
                let mode_msg = format!(":{hostmask} MODE {channel} {sign}m\r\n");
                broadcast_to_channel(state, channel, &mode_msg);
                s2s_broadcast_mode(state, conn, channel, &format!("{sign}m"), None);
            }
            _ => {
                let mode_char = ch.to_string();
                let reply = Message::from_server(
                    server_name,
                    irc::ERR_UNKNOWNMODE,
                    vec![nick, &mode_char, "is unknown mode char to me"],
                );
                send(state, session_id, format!("{reply}\r\n"));
            }
        }
    }
}


pub(super) fn handle_kick(
    conn: &Connection,
    channel: &str,
    target_nick: &str,
    reason: &str,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();

    // Verify kicker is in the channel and is an op
    let (in_channel, is_op) = state
        .channels
        .lock()
        .unwrap()
        .get(channel)
        .map(|ch| (ch.members.contains(session_id), ch.ops.contains(session_id)))
        .unwrap_or((false, false));

    if !in_channel {
        let reply = Message::from_server(
            server_name,
            irc::ERR_NOTONCHANNEL,
            vec![nick, channel, "You're not on that channel"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    if !is_op {
        let reply = Message::from_server(
            server_name,
            irc::ERR_CHANOPRIVSNEEDED,
            vec![nick, channel, "You're not channel operator"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    // Resolve target via federated channel roster
    use super::helpers::{resolve_channel_target, ChannelTarget};
    match resolve_channel_target(state, channel, target_nick) {
        ChannelTarget::Local { session_id: target_session } => {
            // Broadcast KICK, then remove from channel
            let hostmask = conn.hostmask();
            let kick_msg = format!(":{hostmask} KICK {channel} {target_nick} :{reason}\r\n");
            broadcast_to_channel(state, channel, &kick_msg);

            // Remove target from channel
            {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel) {
                    ch.members.remove(&target_session);
                    ch.ops.remove(&target_session);
                    ch.voiced.remove(&target_session);
                }
            }
        }

        ChannelTarget::Remote(_rm) => {
            // Broadcast KICK locally so local users see it
            let hostmask = conn.hostmask();
            let kick_msg = format!(":{hostmask} KICK {channel} {target_nick} :{reason}\r\n");
            broadcast_to_channel(state, channel, &kick_msg);

            // Remove from our remote_members tracking (case-insensitive)
            {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel) {
                    ch.remove_remote_member(target_nick);
                }
            }

            // Relay as a proper S2S Kick so remote server can enforce it
            // (carries kick reason, kicker identity — not a generic Part)
            let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
            s2s_broadcast(state, crate::s2s::S2sMessage::Kick {
                event_id: s2s_next_event_id(state),
                nick: target_nick.to_string(),
                channel: channel.to_string(),
                by: conn.nick.as_deref().unwrap_or("*").to_string(),
                reason: reason.to_string(),
                origin,
            });
        }

        ChannelTarget::NotPresent => {
            let reply = Message::from_server(
                server_name,
                irc::ERR_USERNOTINCHANNEL,
                vec![nick, target_nick, channel, "They aren't on that channel"],
            );
            send(state, session_id, format!("{reply}\r\n"));
        }
    }
}

/// Broadcast a raw message to all members of a channel.

pub(super) fn handle_invite(
    conn: &Connection,
    target_nick: &str,
    channel: &str,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();

    // Verify inviter is in the channel and is an op
    let (in_channel, is_op, is_invite_only) = state
        .channels
        .lock()
        .unwrap()
        .get(channel)
        .map(|ch| (
            ch.members.contains(session_id),
            ch.ops.contains(session_id),
            ch.invite_only,
        ))
        .unwrap_or((false, false, false));

    if !in_channel {
        let reply = Message::from_server(
            server_name,
            irc::ERR_NOTONCHANNEL,
            vec![nick, channel, "You're not on that channel"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    // If channel is +i, only ops can invite
    if is_invite_only && !is_op {
        let reply = Message::from_server(
            server_name,
            irc::ERR_CHANOPRIVSNEEDED,
            vec![nick, channel, "You're not channel operator"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    // Resolve target via federated network roster.
    // INVITE doesn't require the target to be in the channel — they just
    // need to exist somewhere (locally or as a known remote user).
    use super::helpers::{resolve_network_target, NetworkTarget};
    match resolve_network_target(state, target_nick) {
        NetworkTarget::Local { session_id: target_sid } => {
            // Add invite by session ID + DID
            {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel) {
                    ch.invites.insert(target_sid.clone());
                    if let Some(did) = state.session_dids.lock().unwrap().get(&target_sid) {
                        ch.invites.insert(did.clone());
                    }
                }
            }

            // Notify inviter
            let reply = Message::from_server(server_name, "341", vec![nick, target_nick, channel]);
            send(state, session_id, format!("{reply}\r\n"));

            // Notify target
            let hostmask = conn.hostmask();
            let invite_msg = format!(":{hostmask} INVITE {target_nick} {channel}\r\n");
            if let Some(tx) = state.connections.lock().unwrap().get(&target_sid) {
                let _ = tx.try_send(invite_msg);
            }
        }

        NetworkTarget::Remote(rm) => {
            // Add invite by DID if available (so it survives reconnect/rejoin)
            {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel) {
                    if let Some(ref did) = rm.did {
                        ch.invites.insert(did.clone());
                    }
                    // Also store by nick as a fallback for guests without DID
                    ch.invites.insert(format!("nick:{target_nick}"));
                }
            }

            // Notify inviter (remote target can't be notified directly)
            let reply = Message::from_server(server_name, "341", vec![nick, target_nick, channel]);
            send(state, session_id, format!("{reply}\r\n"));
        }

        NetworkTarget::Unknown => {
            let reply = Message::from_server(
                server_name,
                irc::ERR_NOSUCHNICK,
                vec![nick, target_nick, "No such nick"],
            );
            send(state, session_id, format!("{reply}\r\n"));
        }
    }
}

/// Broadcast an S2S message to all peer servers (if S2S is active).
/// This is fire-and-forget: spawns a task so we don't block the caller.

pub(super) fn handle_topic(
    conn: &Connection,
    channel: &str,
    new_topic: Option<&str>,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    use crate::server::TopicInfo;

    let nick = conn.nick_or_star();

    // Verify user is in the channel
    let in_channel = state
        .channels
        .lock()
        .unwrap()
        .get(channel)
        .map(|ch| ch.members.contains(session_id))
        .unwrap_or(false);

    if !in_channel {
        let reply = Message::from_server(
            server_name,
            irc::ERR_NOTONCHANNEL,
            vec![nick, channel, "You're not on that channel"],
        );
        send(state, session_id, format!("{reply}\r\n"));
        return;
    }

    match new_topic {
        Some(text) => {
            // Check +t: if topic_locked, only ops can set topic
            let (is_op, is_locked) = {
                let channels = state.channels.lock().unwrap();
                channels.get(channel).map(|ch| {
                    (ch.ops.contains(session_id), ch.topic_locked)
                }).unwrap_or((false, false))
            };
            if is_locked && !is_op {
                let reply = Message::from_server(
                    server_name,
                    irc::ERR_CHANOPRIVSNEEDED,
                    vec![nick, channel, "You're not channel operator"],
                );
                send(state, session_id, format!("{reply}\r\n"));
                return;
            }

            // Set the topic
            let topic = TopicInfo::new(text.to_string(), conn.hostmask());

            // Store it
            state
                .channels
                .lock()
                .unwrap()
                .entry(channel.to_string())
                .and_modify(|ch| {
                    ch.topic = Some(topic);
                });

            // CRDT update (async, source of truth for topic convergence)
            {
                let state_c = Arc::clone(state);
                let channel_c = channel.to_string();
                let text_c = text.to_string();
                let nick_c = nick.to_string();
                let did_c = state.session_dids.lock().unwrap().get(session_id).cloned();
                tokio::spawn(async move {
                    state_c.crdt_set_topic(&channel_c, &text_c, &nick_c, did_c.as_deref()).await;
                });
            }

            // Persist channel state
            {
                let channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get(channel) {
                    let ch_clone = ch.clone();
                    drop(channels);
                    state.with_db(|db| db.save_channel(channel, &ch_clone));
                }
            }

            // Broadcast TOPIC change to all channel members
            let hostmask = conn.hostmask();
            let topic_msg = format!(":{hostmask} TOPIC {channel} :{text}\r\n");

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
                    let _ = tx.try_send(topic_msg.clone());
                }
            }

            // Broadcast TOPIC to S2S peers
            let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
            s2s_broadcast(state, crate::s2s::S2sMessage::Topic {
                event_id: s2s_next_event_id(state),
                channel: channel.to_string(),
                topic: text.to_string(),
                set_by: conn.nick.as_deref().unwrap_or("*").to_string(),
                origin,
            });
        }
        None => {
            // Query the topic
            let channels = state.channels.lock().unwrap();
            if let Some(ch) = channels.get(channel) {
                if let Some(ref topic) = ch.topic {
                    let rpl = Message::from_server(
                        server_name,
                        irc::RPL_TOPIC,
                        vec![nick, channel, &topic.text],
                    );
                    send(state, session_id, format!("{rpl}\r\n"));

                    let rpl_who = Message::from_server(
                        server_name,
                        irc::RPL_TOPICWHOTIME,
                        vec![nick, channel, &topic.set_by, &topic.set_at.to_string()],
                    );
                    send(state, session_id, format!("{rpl_who}\r\n"));
                } else {
                    let rpl = Message::from_server(
                        server_name,
                        irc::RPL_NOTOPIC,
                        vec![nick, channel, "No topic is set"],
                    );
                    send(state, session_id, format!("{rpl}\r\n"));
                }
            }
        }
    }
}


pub(super) fn handle_part(
    conn: &Connection,
    channel: &str,
    state: &Arc<SharedState>,
    session_id: &str,
) {
    let hostmask = conn.hostmask();
    let part_msg = format!(":{hostmask} PART {channel}\r\n");

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
            let _ = tx.try_send(part_msg.clone());
        }
    }
    drop(conns);

    state
        .channels
        .lock()
        .unwrap()
        .entry(channel.to_string())
        .and_modify(|ch| {
            ch.members.remove(session_id);
        });

    // NOTE: Presence is NOT in CRDT (avoids ghost users on crash)

    // Broadcast PART to S2S peers
    let event_id = s2s_next_event_id(state);
    let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
    s2s_broadcast(state, crate::s2s::S2sMessage::Part {
        event_id,
        nick: conn.nick.as_deref().unwrap_or("*").to_string(),
        channel: channel.to_string(),
        origin,
    });
}


pub(super) fn handle_names(
    conn: &Connection,
    channel: &str,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();
    let multi_prefix = state.cap_multi_prefix.lock().unwrap().contains(session_id);

    let nick_list: Vec<String> = {
        let channels = state.channels.lock().unwrap();
        let (member_sessions, remote_members, ops, voiced) = match channels.get(channel) {
            Some(ch) => (ch.members.clone(), ch.remote_members.clone(), ch.ops.clone(), ch.voiced.clone()),
            None => Default::default(),
        };
        drop(channels);
        let nicks = state.nick_to_session.lock().unwrap();
        let reverse: std::collections::HashMap<&String, &String> =
            nicks.iter().map(|(n, s)| (s, n)).collect();
        let mut list: Vec<String> = member_sessions
            .iter()
            .filter_map(|s| {
                reverse.get(s).map(|n| {
                    let prefix = if multi_prefix {
                        // multi-prefix: show all applicable prefixes
                        let mut p = String::new();
                        if ops.contains(s) { p.push('@'); }
                        if voiced.contains(s) { p.push('+'); }
                        p
                    } else {
                        // Standard: show highest prefix only
                        if ops.contains(s) { "@".to_string() }
                        else if voiced.contains(s) { "+".to_string() }
                        else { String::new() }
                    };
                    format!("{prefix}{n}")
                })
            })
            .collect();
        let channels_lock = state.channels.lock().unwrap();
        let ch_state = channels_lock.get(channel);
        for (nick, rm) in &remote_members {
            let is_op = rm.is_op || rm.did.as_ref().is_some_and(|d| {
                ch_state.is_some_and(|ch| {
                    ch.founder_did.as_deref() == Some(d.as_str()) || ch.did_ops.contains(d)
                })
            });
            let prefix = if is_op { "@" } else { "" };
            list.push(format!("{prefix}{nick}"));
        }
        drop(channels_lock);
        list
    };

    let names = irc::Message::from_server(
        server_name,
        irc::RPL_NAMREPLY,
        vec![nick, "=", channel, &nick_list.join(" ")],
    );
    let end_names = irc::Message::from_server(
        server_name,
        irc::RPL_ENDOFNAMES,
        vec![nick, channel, "End of /NAMES list"],
    );
    send(state, session_id, format!("{names}\r\n"));
    send(state, session_id, format!("{end_names}\r\n"));
}


pub(super) fn handle_list(
    conn: &Connection,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();
    let channels = state.channels.lock().unwrap();
    for (name, ch) in channels.iter() {
        let count = ch.members.len() + ch.remote_members.len();
        let topic = ch.topic.as_ref().map(|t| t.text.as_str()).unwrap_or("");
        let reply = Message::from_server(
            server_name,
            irc::RPL_LIST,
            vec![nick, name, &count.to_string(), topic],
        );
        send(state, session_id, format!("{reply}\r\n"));
    }
    let end = Message::from_server(
        server_name,
        irc::RPL_LISTEND,
        vec![nick, "End of /LIST"],
    );
    send(state, session_id, format!("{end}\r\n"));
}

// ── WHO command ─────────────────────────────────────────────────────

