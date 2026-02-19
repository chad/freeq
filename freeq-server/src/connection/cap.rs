#![allow(clippy::too_many_arguments)]
//! CAP capability negotiation and SASL authentication.

use std::sync::Arc;
use crate::irc::{self, Message};
use crate::sasl;
use crate::server::SharedState;
use super::Connection;
use super::helpers::broadcast_account_notify;
use super::registration::try_complete_registration;

pub(super) fn handle_cap(
    conn: &mut Connection,
    msg: &Message,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let subcmd = msg.params.first().map(|s| s.to_ascii_uppercase());
    match subcmd.as_deref() {
        Some("LS") => {
            conn.cap_negotiating = true;
            // Build capability list, including iroh endpoint ID if available
            let mut caps = String::from("sasl message-tags multi-prefix echo-message server-time batch draft/chathistory account-notify extended-join away-notify");
            if let Some(ref iroh_id) = *state.server_iroh_id.lock().unwrap() {
                caps.push_str(&format!(" iroh={iroh_id}"));
            }
            let reply = Message::from_server(
                server_name,
                "CAP",
                vec![conn.nick_or_star(), "LS", &caps],
            );
            send(state, session_id, format!("{reply}\r\n"));
        }
        Some("REQ") => {
            if let Some(caps) = msg.params.get(1) {
                let requested: Vec<&str> = caps.split_whitespace().collect();
                let mut acked = Vec::new();
                let mut all_ok = true;

                for cap in &requested {
                    match cap.to_ascii_lowercase().as_str() {
                        "sasl" => {
                            conn.cap_sasl_requested = true;
                            acked.push("sasl");
                        }
                        "message-tags" => {
                            conn.cap_message_tags = true;
                            state.cap_message_tags.lock().unwrap().insert(session_id.to_string());
                            acked.push("message-tags");
                        }
                        "multi-prefix" => {
                            conn.cap_multi_prefix = true;
                            state.cap_multi_prefix.lock().unwrap().insert(session_id.to_string());
                            acked.push("multi-prefix");
                        }
                        "echo-message" => {
                            conn.cap_echo_message = true;
                            state.cap_echo_message.lock().unwrap().insert(session_id.to_string());
                            acked.push("echo-message");
                        }
                        "server-time" => {
                            conn.cap_server_time = true;
                            state.cap_server_time.lock().unwrap().insert(session_id.to_string());
                            acked.push("server-time");
                        }
                        "batch" => {
                            conn.cap_batch = true;
                            state.cap_batch.lock().unwrap().insert(session_id.to_string());
                            acked.push("batch");
                        }
                        "draft/chathistory" => {
                            conn.cap_chathistory = true;
                            acked.push("draft/chathistory");
                        }
                        "account-notify" => {
                            conn.cap_account_notify = true;
                            state.cap_account_notify.lock().unwrap().insert(session_id.to_string());
                            acked.push("account-notify");
                        }
                        "extended-join" => {
                            conn.cap_extended_join = true;
                            state.cap_extended_join.lock().unwrap().insert(session_id.to_string());
                            acked.push("extended-join");
                        }
                        "away-notify" => {
                            conn.cap_away_notify = true;
                            state.cap_away_notify.lock().unwrap().insert(session_id.to_string());
                            acked.push("away-notify");
                        }
                        _ => { all_ok = false; }
                    }
                }

                if all_ok && !acked.is_empty() {
                    let ack_str = acked.join(" ");
                    let reply = Message::from_server(
                        server_name,
                        "CAP",
                        vec![conn.nick_or_star(), "ACK", &ack_str],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                } else {
                    let reply = Message::from_server(
                        server_name,
                        "CAP",
                        vec![conn.nick_or_star(), "NAK", caps],
                    );
                    send(state, session_id, format!("{reply}\r\n"));
                }
            }
        }
        Some("END") => {
            conn.cap_negotiating = false;
            try_complete_registration(conn, state, server_name, session_id, send);
        }
        _ => {}
    }
}


pub(super) async fn handle_authenticate(
    conn: &mut Connection,
    msg: &Message,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let param = msg.params.first().map(|s| s.as_str()).unwrap_or("");

    if param == "*" {
        // SASL abort — client is cancelling the authentication attempt
        conn.sasl_in_progress = false;
        let fail = Message::from_server(
            server_name,
            irc::ERR_SASLFAIL,
            vec![conn.nick_or_star(), "SASL authentication aborted"],
        );
        send(state, session_id, format!("{fail}\r\n"));
        return;
    }

    if param.eq_ignore_ascii_case("ATPROTO-CHALLENGE") {
        conn.sasl_in_progress = true;
        let encoded = state.challenge_store.create(session_id);
        let reply = Message::new("AUTHENTICATE", vec![&encoded]);
        send(state, session_id, format!("{reply}\r\n"));
    } else if conn.sasl_in_progress {
        if let Some(response) = sasl::decode_response(param) {
            let taken = state.challenge_store.take(session_id);
            match taken {
                Some((challenge, challenge_bytes)) => {
                    match sasl::verify_response(
                        &challenge,
                        &challenge_bytes,
                        &response,
                        &state.did_resolver,
                    )
                    .await
                    {
                        Ok(did) => {
                            conn.authenticated_did = Some(did.clone());
                            conn.sasl_in_progress = false;
                            state
                                .session_dids
                                .lock()
                                .unwrap()
                                .insert(session_id.to_string(), did.clone());

                            // Bind nick to DID (persistent identity-nick)
                            if let Some(ref nick) = conn.nick {
                                let nick_lower = nick.to_lowercase();
                                state.did_nicks.lock().unwrap().insert(did.clone(), nick_lower.clone());
                                state.nick_owners.lock().unwrap().insert(nick_lower.clone(), did.clone());
                                let nick_l = nick_lower.clone();
                                let did_c = did.clone();
                                let state_c = Arc::clone(state);
                                tokio::spawn(async move {
                                    state_c.crdt_set_nick_owner(&nick_l, &did_c).await;
                                });
                                state.with_db(|db| db.save_identity(&did, &nick.to_lowercase()));
                            }

                            // Resolve handle from DID document for WHOIS display,
                            // then run plugins with the resolved handle.
                            {
                                let did_clone = did.clone();
                                let state_clone = Arc::clone(state);
                                let sid = session_id.to_string();
                                let nick_for_plugin = conn.nick.clone().unwrap_or_default();
                                tokio::spawn(async move {
                                    let mut resolved_handle: Option<String> = None;
                                    if let Ok(doc) = state_clone.did_resolver.resolve(&did_clone).await {
                                        for aka in &doc.also_known_as {
                                            if let Some(handle) = aka.strip_prefix("at://") {
                                                resolved_handle = Some(handle.to_string());
                                                state_clone.session_handles.lock().unwrap()
                                                    .insert(sid.clone(), handle.to_string());
                                                break;
                                            }
                                        }
                                    }

                                    // Run plugins after handle resolution
                                    let auth_event = crate::plugin::AuthEvent {
                                        did: did_clone.clone(),
                                        handle: resolved_handle,
                                        nick: nick_for_plugin,
                                        session_id: sid.clone(),
                                    };
                                    let result = state_clone.plugin_manager.on_auth(&auth_event);
                                    if let Some(override_did) = result.override_did {
                                        state_clone.session_dids.lock().unwrap()
                                            .insert(sid.clone(), override_did);
                                    }
                                    if let Some(override_handle) = result.override_handle {
                                        state_clone.session_handles.lock().unwrap()
                                            .insert(sid.clone(), override_handle);
                                    }
                                });
                            }

                            let nick = conn.nick_or_star();
                            let hostmask = conn.hostmask();
                            let logged_in = Message::from_server(
                                server_name,
                                irc::RPL_LOGGEDIN,
                                vec![
                                    nick,
                                    &hostmask,
                                    &did,
                                    &format!("You are now logged in as {did}"),
                                ],
                            );
                            send(state, session_id, format!("{logged_in}\r\n"));

                            let success = Message::from_server(
                                server_name,
                                irc::RPL_SASLSUCCESS,
                                vec![nick, "SASL authentication successful"],
                            );
                            send(state, session_id, format!("{success}\r\n"));

                            // Broadcast account-notify to shared channels
                            broadcast_account_notify(state, session_id, nick, &did);
                        }
                        Err(reason) => {
                            tracing::warn!(%session_id, "SASL auth failed: {reason}");
                            conn.sasl_in_progress = false;
                            let fail = Message::from_server(
                                server_name,
                                irc::ERR_SASLFAIL,
                                vec![conn.nick_or_star(), "SASL authentication failed"],
                            );
                            send(state, session_id, format!("{fail}\r\n"));
                        }
                    }
                }
                None => {
                    conn.sasl_in_progress = false;
                    let fail = Message::from_server(
                        server_name,
                        irc::ERR_SASLFAIL,
                        vec![
                            conn.nick_or_star(),
                            "SASL authentication failed (no challenge)",
                        ],
                    );
                    send(state, session_id, format!("{fail}\r\n"));
                }
            }
        } else {
            conn.sasl_in_progress = false;
            let fail = Message::from_server(
                server_name,
                irc::ERR_SASLFAIL,
                vec![
                    conn.nick_or_star(),
                    "SASL authentication failed (bad response)",
                ],
            );
            send(state, session_id, format!("{fail}\r\n"));
        }
    } else {
        let fail = Message::from_server(
            server_name,
            irc::ERR_SASLFAIL,
            vec![conn.nick_or_star(), "Unsupported SASL mechanism"],
        );
        send(state, session_id, format!("{fail}\r\n"));
    }
}

