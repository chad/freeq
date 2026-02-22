//! IRC POLICY command handler.
//!
//! POLICY <channel> SET <rules_text>   — Create/update ACCEPT-only policy
//! POLICY <channel> INFO               — Show current policy
//! POLICY <channel> ACCEPT             — Accept current policy (join flow)
//! POLICY <channel> CLEAR              — Remove policy (ops only)

use crate::irc::Message;
use crate::policy::canonical;
use crate::policy::eval::UserEvidence;
use crate::policy::types::*;
use crate::server::SharedState;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use super::helpers::{s2s_broadcast, s2s_next_event_id};

pub(super) fn handle_policy(
    conn: &super::Connection,
    msg: &Message,
    state: &Arc<SharedState>,
    server_name: &str,
    session_id: &str,
    send_fn: &impl Fn(&Arc<SharedState>, &str, String),
) {
    let nick = conn.nick_or_star();

    let engine = match &state.policy_engine {
        Some(e) => e,
        None => {
            let reply = Message::from_server(
                server_name,
                "NOTICE",
                vec![nick, "Policy framework is not enabled on this server"],
            );
            send_fn(state, session_id, format!("{reply}\r\n"));
            return;
        }
    };

    if msg.params.len() < 2 {
        let reply = Message::from_server(
            server_name,
            "NOTICE",
            vec![nick, "Usage: POLICY <channel> SET|INFO|ACCEPT|CLEAR"],
        );
        send_fn(state, session_id, format!("{reply}\r\n"));
        return;
    }

    let channel = &msg.params[0];
    let subcommand = msg.params[1].to_uppercase();

    if !channel.starts_with('#') {
        let reply = Message::from_server(
            server_name,
            "NOTICE",
            vec![nick, "POLICY only applies to channels"],
        );
        send_fn(state, session_id, format!("{reply}\r\n"));
        return;
    }

    match subcommand.as_str() {
        "SET" => {
            // Require ops
            if !is_channel_op(state, channel, session_id, conn.authenticated_did.as_deref()) {
                let reply = Message::from_server(
                    server_name,
                    "482", // ERR_CHANOPRIVSNEEDED
                    vec![nick, channel, "You're not channel operator"],
                );
                send_fn(state, session_id, format!("{reply}\r\n"));
                return;
            }

            // Get rules text from remaining params
            let rules_text = if msg.params.len() > 2 {
                msg.params[2..].join(" ")
            } else {
                let reply = Message::from_server(
                    server_name,
                    "NOTICE",
                    vec![nick, "Usage: POLICY <channel> SET <rules text>"],
                );
                send_fn(state, session_id, format!("{reply}\r\n"));
                return;
            };

            let rules_hash = canonical::sha256_hex(rules_text.as_bytes());

            // Check if channel already has a policy
            let result = match engine.get_policy(channel) {
                Ok(Some(_)) => {
                    // Update existing
                    engine.update_channel_policy(
                        channel,
                        Requirement::Accept {
                            hash: rules_hash.clone(),
                        },
                        BTreeMap::new(),
                    )
                }
                Ok(None) => {
                    // Create new
                    engine
                        .create_channel_policy(
                            channel,
                            Requirement::Accept {
                                hash: rules_hash.clone(),
                            },
                            BTreeMap::new(),
                        )
                        .map(|(p, _)| p)
                }
                Err(e) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Policy error: {e}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                    return;
                }
            };

            match result {
                Ok(policy) => {
                    let pid = policy.policy_id.as_deref().unwrap_or("?");
                    let notice = format!(
                        "Policy set for {channel} (version {}, rules_hash={}, policy_id={})",
                        policy.version,
                        &rules_hash[..12],
                        &pid[..12.min(pid.len())]
                    );
                    let reply =
                        Message::from_server(server_name, "NOTICE", vec![nick, &notice]);
                    send_fn(state, session_id, format!("{reply}\r\n"));

                    // Auto-attest the op who set the policy
                    if let Some(did) = &conn.authenticated_did {
                        let evidence = UserEvidence {
                            accepted_hashes: HashSet::from([rules_hash]),
                            credentials: vec![],
                            proofs: HashSet::new(),
                        };
                        let _ = engine.process_join(channel, did, &evidence);
                    }

                    // Broadcast policy to S2S peers
                    let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
                    let auth_set_json = engine.store()
                        .get_authority_set(&policy.authority_set_hash)
                        .ok()
                        .flatten()
                        .and_then(|a| serde_json::to_string(&a).ok());
                    s2s_broadcast(state, crate::s2s::S2sMessage::PolicySync {
                        event_id: s2s_next_event_id(state),
                        channel: channel.to_string(),
                        policy_json: serde_json::to_string(&policy).ok(),
                        authority_set_json: auth_set_json,
                        origin,
                    });
                }
                Err(e) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Failed to set policy: {e}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
            }
        }

        "INFO" => {
            match engine.get_policy(channel) {
                Ok(Some(policy)) => {
                    let pid = policy.policy_id.as_deref().unwrap_or("unknown");
                    let lines = [
                        format!("Policy for {channel}:"),
                        format!("  Version: {}", policy.version),
                        format!("  Policy ID: {pid}"),
                        format!("  Effective: {}", policy.effective_at),
                        format!("  Validity: {:?}", policy.validity_model),
                        format!("  Requirement: {}", describe_requirement(&policy.requirements)),
                    ];
                    for line in &lines {
                        let reply =
                            Message::from_server(server_name, "NOTICE", vec![nick, line]);
                        send_fn(state, session_id, format!("{reply}\r\n"));
                    }
                    if !policy.role_requirements.is_empty() {
                        for (role, req) in &policy.role_requirements {
                            let desc = format!(
                                "  Role '{}': {}",
                                role,
                                describe_requirement(req)
                            );
                            let reply =
                                Message::from_server(server_name, "NOTICE", vec![nick, &desc]);
                            send_fn(state, session_id, format!("{reply}\r\n"));
                        }
                    }
                }
                Ok(None) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("{channel} has no policy (open join)")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
                Err(e) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Policy error: {e}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
            }
        }

        "ACCEPT" => {
            let did = match &conn.authenticated_did {
                Some(d) => d.clone(),
                None => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, "You must be authenticated to accept a policy"],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                    return;
                }
            };

            // Get current policy and its rules hash
            let policy = match engine.get_policy(channel) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("{channel} has no policy — just JOIN it")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                    return;
                }
                Err(e) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Policy error: {e}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                    return;
                }
            };

            // Extract the ACCEPT hash from the requirement
            let accepted_hash = extract_accept_hash(&policy.requirements);
            let evidence = UserEvidence {
                accepted_hashes: accepted_hash.into_iter().collect(),
                credentials: vec![],
                proofs: HashSet::new(),
            };

            match engine.process_join(channel, &did, &evidence) {
                Ok(crate::policy::JoinResult::Confirmed { attestation, .. }) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![
                            nick,
                            &format!(
                                "Policy accepted for {channel} — role: {}. You may now JOIN.",
                                attestation.role
                            ),
                        ],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
                Ok(crate::policy::JoinResult::Failed(reason)) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Policy acceptance failed: {reason}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
                Ok(_) => {}
                Err(e) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Policy error: {e}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
            }
        }

        "CLEAR" => {
            // Require ops
            if !is_channel_op(state, channel, session_id, conn.authenticated_did.as_deref()) {
                let reply = Message::from_server(
                    server_name,
                    "482",
                    vec![nick, channel, "You're not channel operator"],
                );
                send_fn(state, session_id, format!("{reply}\r\n"));
                return;
            }

            match engine.remove_policy(channel) {
                Ok(true) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Policy removed from {channel} — channel is now open join")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));

                    // Broadcast clear to S2S peers
                    let origin = state.server_iroh_id.lock().unwrap().clone().unwrap_or_default();
                    s2s_broadcast(state, crate::s2s::S2sMessage::PolicySync {
                        event_id: s2s_next_event_id(state),
                        channel: channel.to_string(),
                        policy_json: None,
                        authority_set_json: None,
                        origin,
                    });
                }
                Ok(false) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("{channel} has no policy to remove")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
                Err(e) => {
                    let reply = Message::from_server(
                        server_name,
                        "NOTICE",
                        vec![nick, &format!("Failed to remove policy: {e}")],
                    );
                    send_fn(state, session_id, format!("{reply}\r\n"));
                }
            }
        }

        _ => {
            let reply = Message::from_server(
                server_name,
                "NOTICE",
                vec![nick, "Usage: POLICY <channel> SET|INFO|ACCEPT|CLEAR"],
            );
            send_fn(state, session_id, format!("{reply}\r\n"));
        }
    }
}

/// Check if session is a channel op.
fn is_channel_op(
    state: &SharedState,
    channel: &str,
    session_id: &str,
    did: Option<&str>,
) -> bool {
    let channels = state.channels.lock().unwrap();
    if let Some(ch) = channels.get(channel) {
        if ch.ops.contains(session_id) {
            return true;
        }
        if let Some(d) = did {
            if ch.did_ops.contains(d) || ch.founder_did.as_deref() == Some(d) {
                return true;
            }
        }
    }
    false
}

/// Extract ACCEPT hash from a requirement tree (for simple ACCEPT-only policies).
fn extract_accept_hash(req: &Requirement) -> HashSet<String> {
    let mut hashes = HashSet::new();
    match req {
        Requirement::Accept { hash } => {
            hashes.insert(hash.clone());
        }
        Requirement::All { requirements } | Requirement::Any { requirements } => {
            for r in requirements {
                hashes.extend(extract_accept_hash(r));
            }
        }
        Requirement::Not { requirement } => {
            hashes.extend(extract_accept_hash(requirement));
        }
        _ => {}
    }
    hashes
}

/// Human-readable description of a requirement.
fn describe_requirement(req: &Requirement) -> String {
    match req {
        Requirement::Accept { hash } => format!("ACCEPT({}...)", &hash[..12.min(hash.len())]),
        Requirement::Present {
            credential_type,
            issuer,
        } => match issuer {
            Some(iss) => format!("PRESENT({credential_type}, issuer={iss})"),
            None => format!("PRESENT({credential_type})"),
        },
        Requirement::Prove { proof_type } => format!("PROVE({proof_type})"),
        Requirement::All { requirements } => {
            let inner: Vec<_> = requirements.iter().map(describe_requirement).collect();
            format!("ALL({})", inner.join(", "))
        }
        Requirement::Any { requirements } => {
            let inner: Vec<_> = requirements.iter().map(describe_requirement).collect();
            format!("ANY({})", inner.join(", "))
        }
        Requirement::Not { requirement } => {
            format!("NOT({})", describe_requirement(requirement))
        }
    }
}
