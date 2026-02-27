#![allow(clippy::too_many_arguments)]
//! Per-client connection handler.
//!
//! Each TCP connection gets a [`Connection`] that manages:
//! - IRC registration (NICK/USER)
//! - CAP capability negotiation
//! - SASL authentication flow
//! - Message routing post-registration
//! - WHOIS with DID information
//!
//! The handler is split into submodules for readability:
//! - [`cap`] — CAP negotiation and SASL authentication
//! - [`registration`] — IRC registration completion
//! - [`channel`] — JOIN, PART, MODE, TOPIC, KICK, INVITE, NAMES, LIST
//! - [`messaging`] — PRIVMSG, NOTICE, TAGMSG, CHATHISTORY
//! - [`queries`] — WHOIS, WHO, LUSERS, AWAY
//! - [`helpers`] — S2S broadcast, channel delivery, utility functions

mod cap;
mod channel;
pub mod helpers;
mod messaging;
mod policy_cmd;
mod queries;
mod registration;
pub(crate) mod routing;

use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::irc::{self, Message};
use crate::server::SharedState;

use cap::{handle_authenticate, handle_cap};
use channel::{
    handle_invite, handle_join, handle_kick, handle_list, handle_mode, handle_names, handle_part,
    handle_topic,
};
use helpers::{normalize_channel, s2s_broadcast, s2s_next_event_id};
use messaging::{handle_chathistory, handle_privmsg, handle_tagmsg};
use queries::{handle_away, handle_lusers, handle_who, handle_whois};
use policy_cmd::handle_policy;
use registration::try_complete_registration;

// Re-export items used by other modules in the crate

/// State of a single client connection.
pub struct Connection {
    pub id: String,
    pub nick: Option<String>,
    pub user: Option<String>,
    pub realname: Option<String>,
    pub authenticated_did: Option<String>,
    pub registered: bool,

    /// Iroh endpoint ID of the remote peer (if connected via iroh).
    /// This is a cryptographic public key, giving us verified identity.
    pub iroh_endpoint_id: Option<String>,

    // CAP negotiation state
    pub(crate) cap_negotiating: bool,
    pub(crate) cap_sasl_requested: bool,
    pub(crate) cap_message_tags: bool,
    pub(crate) cap_multi_prefix: bool,
    pub(crate) cap_echo_message: bool,
    pub(crate) cap_server_time: bool,
    pub(crate) cap_batch: bool,
    pub(crate) cap_chathistory: bool,
    pub(crate) cap_account_notify: bool,
    pub(crate) cap_extended_join: bool,
    pub(crate) cap_away_notify: bool,
    /// Client understands E2EE messages (won't get synthetic notices instead).
    #[allow(dead_code)]
    pub(crate) cap_e2ee: bool,
    /// Server operator (OPER) status.
    pub(crate) is_oper: bool,

    // SASL state
    pub(crate) sasl_in_progress: bool,
    pub(crate) sasl_failures: u8,
}

impl Connection {
    fn new(id: String) -> Self {
        Self {
            id,
            nick: None,
            user: None,
            realname: None,
            authenticated_did: None,
            registered: false,
            iroh_endpoint_id: None,
            cap_negotiating: false,
            cap_sasl_requested: false,
            cap_message_tags: false,
            cap_multi_prefix: false,
            cap_echo_message: false,
            cap_server_time: false,
            cap_batch: false,
            cap_chathistory: false,
            cap_account_notify: false,
            cap_extended_join: false,
            cap_away_notify: false,
            cap_e2ee: false,
            is_oper: false,
            sasl_in_progress: false,
            sasl_failures: 0,
        }
    }

    pub(crate) fn nick_or_star(&self) -> &str {
        self.nick.as_deref().unwrap_or("*")
    }

    pub(crate) fn hostmask(&self) -> String {
        let nick = self.nick.as_deref().unwrap_or("*");
        let user = self.user.as_deref().unwrap_or("~u");
        let host = self.cloaked_host();
        format!("{nick}!{user}@{host}")
    }

    /// Generate a cloaked hostname.
    /// Authenticated users: shortened DID (e.g. "did/plc/4qsy..xmns")
    /// Guests: "freeq/guest"
    pub(crate) fn cloaked_host(&self) -> String {
        if let Some(ref did) = self.authenticated_did {
            // e.g. did:plc:4qsyxmnsblo4luuycm3572bq → plc/4qsyxmns
            let short = did.strip_prefix("did:").unwrap_or(did);
            let parts: Vec<&str> = short.splitn(2, ':').collect();
            if parts.len() == 2 {
                let method = parts[0];
                let id = &parts[1][..parts[1].len().min(8)];
                format!("freeq/{method}/{id}")
            } else {
                "freeq/did".to_string()
            }
        } else {
            "freeq/guest".to_string()
        }
    }
}

/// Handle a plain TCP connection.
pub async fn handle(stream: TcpStream, state: Arc<SharedState>) -> Result<()> {
    let peer = stream.peer_addr()?;
    let session_id = format!("{peer}");
    tracing::info!(%session_id, "New connection (plain)");
    let (reader, writer) = tokio::io::split(stream);
    handle_io(BufReader::new(reader), writer, session_id, state).await
}

/// Handle a generic async stream (for TLS, WebSocket, or other wrappers).
pub async fn handle_generic<S>(stream: S, state: Arc<SharedState>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    handle_generic_with_meta(stream, state, None).await
}

/// Handle a generic async stream with optional connection metadata.
///
/// `iroh_endpoint_id` is set when the connection comes via iroh transport,
/// providing cryptographic identity for the remote peer.
pub async fn handle_generic_with_meta<S>(
    stream: S,
    state: Arc<SharedState>,
    iroh_endpoint_id: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let session_id = format!("stream-{id}");
    tracing::info!(%session_id, iroh_id = ?iroh_endpoint_id, "New connection (generic stream)");
    let (reader, writer) = tokio::io::split(stream);
    handle_io_with_meta(BufReader::new(reader), writer, session_id, state, iroh_endpoint_id).await
}

async fn handle_io<R, W>(
    reader: BufReader<R>,
    writer: W,
    session_id: String,
    state: Arc<SharedState>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    handle_io_with_meta(reader, writer, session_id, state, None).await
}

async fn handle_io_with_meta<R, W>(
    mut reader: BufReader<R>,
    writer: W,
    session_id: String,
    state: Arc<SharedState>,
    iroh_endpoint_id: Option<String>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut conn = Connection::new(session_id.clone());
    conn.iroh_endpoint_id = iroh_endpoint_id;

    // Plugin on_connect hook
    state.plugin_manager.on_connect(&crate::plugin::ConnectEvent {
        session_id: session_id.clone(),
        remote_addr: session_id.clone(),
    });

    // Channel for sending messages TO this client
    let (tx, mut rx) = mpsc::channel::<String>(4096);
    state
        .connections
        .lock()
        .insert(session_id.clone(), tx);

    let server_name = state.server_name.clone();

    // Spawn writer task
    let write_session_id = session_id.clone();
    let mut write_half = writer;
    let write_handle = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Some(line) = rx.recv().await {
            // Write the first message
            if let Err(e) = write_half.write_all(line.as_bytes()).await {
                tracing::warn!(session_id = %write_session_id, "Write error: {e}");
                break;
            }
            // Drain any queued messages and batch-write them (reduces syscalls)
            let mut batch_count = 0;
            while let Ok(queued) = rx.try_recv() {
                if let Err(e) = write_half.write_all(queued.as_bytes()).await {
                    tracing::warn!(session_id = %write_session_id, "Write error: {e}");
                    return;
                }
                batch_count += 1;
                if batch_count >= 64 { break; } // cap batch size
            }
            // Flush after the batch
            if let Err(e) = write_half.flush().await {
                tracing::warn!(session_id = %write_session_id, "Flush error: {e}");
                break;
            }
        }
    });

    // Track whether our own send channel is healthy
    let send_healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let send_healthy_ref = send_healthy.clone();
    let send = move |state: &Arc<SharedState>, session_id: &str, msg: String| {
        if let Some(tx) = state.connections.lock().get(session_id) {
            if tx.try_send(msg).is_err() {
                tracing::warn!(session_id, "Send buffer full or closed");
                send_healthy_ref.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
    };

    let mut line_buf = String::new();
    let mut last_activity = tokio::time::Instant::now();
    let ping_interval = tokio::time::Duration::from_secs(30);
    let ping_timeout = tokio::time::Duration::from_secs(60);
    let mut awaiting_pong = false;

    // Rate limiting: max 10 commands per second, token bucket
    let mut rate_tokens: f64 = 10.0;
    let mut rate_last = tokio::time::Instant::now();
    let rate_max: f64 = 10.0;
    let rate_refill: f64 = 10.0; // tokens per second

    loop {
        // Check if our send channel is dead (buffer full = stuck client)
        if !send_healthy.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::info!(%session_id, "Send channel unhealthy, disconnecting");
            break;
        }

        line_buf.clear();
        // Cap line length to 8KB to prevent OOM from malicious clients
        const MAX_LINE_LEN: usize = 8192;
        let read_result = tokio::time::timeout(
            ping_interval,
            reader.read_line(&mut line_buf),
        ).await;
        if line_buf.len() > MAX_LINE_LEN {
            tracing::warn!(%session_id, len = line_buf.len(), "Line too long, dropping");
            let reply = Message::from_server(&server_name, "417", vec!["*", "Input line was too long"]);
            send(&state, &session_id, format!("{reply}\r\n"));
            continue;
        }

        match read_result {
            Ok(Ok(0)) | Ok(Err(_)) => break, // EOF or error
            Err(_) => {
                // Timeout — no data received, send PING or check PONG
                if awaiting_pong {
                    if last_activity.elapsed() > ping_timeout {
                        tracing::info!(%session_id, "Ping timeout");
                        break;
                    }
                } else {
                    let ping = Message::from_server(&server_name, "PING", vec![&server_name]);
                    send(&state, &session_id, format!("{ping}\r\n"));
                    awaiting_pong = true;
                }
                continue;
            }
            Ok(Ok(_)) => {}
        }

        last_activity = tokio::time::Instant::now();

        let Some(msg) = Message::parse(&line_buf) else {
            continue;
        };

        // Rate limiting (skip during registration — clients burst on connect)
        if conn.registered {
            let now = tokio::time::Instant::now();
            let elapsed = now.duration_since(rate_last).as_secs_f64();
            rate_tokens = (rate_tokens + elapsed * rate_refill).min(rate_max);
            rate_last = now;
            if rate_tokens < 1.0 {
                tracing::debug!(%session_id, "Rate limited");
                // Warn the user (only once per burst)
                if rate_tokens > -1.0 {
                    let notice = Message::from_server(&server_name, "NOTICE", vec!["*", "Flood protection: you are sending commands too fast"]);
                    send(&state, &session_id, format!("{notice}\r\n"));
                }
                continue;
            }
            rate_tokens -= 1.0;
        }

        tracing::debug!(%session_id, "<- {}", line_buf.trim());

        match msg.command.as_str() {
            "CAP" => {
                handle_cap(&mut conn, &msg, &state, &server_name, &session_id, &send);
            }
            "AUTHENTICATE" => {
                handle_authenticate(
                    &mut conn,
                    &msg,
                    &state,
                    &server_name,
                    &session_id,
                    &send,
                )
                .await;
            }
            "NICK" => {
                if let Some(nick) = msg.params.first() {
                    // Validate nick: 1-64 chars, allowed chars for IRC + AT handles
                    if nick.is_empty() || nick.len() > 64
                        || nick.contains(|c: char| c.is_control() || c == ' ' || c == '\0' || c == '\r' || c == '\n' || c == ',' || c == '*' || c == '?' || c == '!' || c == '@' || c == '#' || c == '&' || c == ':')
                    {
                        let reply = Message::from_server(
                            &server_name, "432",
                            vec![conn.nick_or_star(), nick, "Erroneous Nickname"],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                        continue;
                    }
                    let nick_lower = nick.to_lowercase();
                    let in_use_by_session = state.nick_to_session.lock()
                        .get_session(&nick)
                        .map(|s| s.to_string());
                    let in_use = in_use_by_session.is_some();

                    // Check if the nick is in use by the same DID (multi-device OK)
                    let in_use_by_same_did = in_use_by_session.as_ref().map_or(false, |sid| {
                        let session_dids = state.session_dids.lock();
                        let my_did = conn.authenticated_did.as_deref();
                        match (session_dids.get(sid), my_did) {
                            (Some(other_did), Some(my)) => other_did == my,
                            _ => false,
                        }
                    });

                    let owner_did = state.nick_owners.lock().get(&nick_lower).cloned();
                    let my_did = conn.authenticated_did.as_deref();
                    let nick_stolen = if conn.cap_negotiating || conn.sasl_in_progress {
                        false
                    } else {
                        owner_did.as_ref().is_some_and(|owner| {
                            my_did.is_none_or(|my| my != owner)
                        })
                    };

                    if in_use && !in_use_by_same_did {
                        // During CAP/SASL negotiation, allow the nick if it's owned
                        // by a DID (attach_same_did will handle multi-device at SASL success).
                        let allow_during_negotiation = (conn.cap_negotiating || conn.sasl_in_progress)
                            && owner_did.is_some();
                        if !allow_during_negotiation {
                            let reply = Message::from_server(
                                &server_name,
                                irc::ERR_NICKNAMEINUSE,
                                vec![conn.nick_or_star(), nick, "Nickname is already in use"],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        } else {
                            // Stash desired nick — don't insert into nick_to_session yet.
                            // attach_same_did will handle at SASL success.
                            conn.nick = Some(nick.clone());
                        }
                    } else if in_use && in_use_by_same_did {
                        // Same DID, multi-device — allow the nick, just stash it
                        conn.nick = Some(nick.clone());
                    } else if nick_stolen {
                        let reply = Message::from_server(
                            &server_name,
                            irc::ERR_NICKNAMEINUSE,
                            vec![conn.nick_or_star(), nick, "Nickname is registered to another identity"],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                    } else {
                        let old_nick = conn.nick.clone();
                        if let Some(ref old) = old_nick {
                            state.nick_to_session.lock().remove_by_nick(old);
                        }
                        state.nick_to_session.lock().insert(&nick, &session_id);
                        conn.nick = Some(nick.clone());

                        if conn.registered {
                            let hostmask = if let Some(ref old) = old_nick {
                                format!("{old}!~{}@{}", conn.user.as_deref().unwrap_or("u"), conn.cloaked_host())
                            } else {
                                conn.hostmask()
                            };
                            let nick_msg = format!(":{hostmask} NICK :{nick}\r\n");
                            send(&state, &session_id, nick_msg.clone());

                            let mut notified = std::collections::HashSet::new();
                            notified.insert(session_id.clone());
                            let channels = state.channels.lock();
                            let conns = state.connections.lock();
                            for ch in channels.values() {
                                if ch.members.contains(&session_id) {
                                    for member in &ch.members {
                                        if notified.insert(member.clone()) {
                                            if let Some(tx) = conns.get(member) {
                                                let _ = tx.try_send(nick_msg.clone());
                                            }
                                        }
                                    }
                                }
                            }
                            drop(conns);
                            drop(channels);

                            // Plugin on_nick_change hook
                            if let Some(ref old) = old_nick {
                                state.plugin_manager.on_nick_change(&crate::plugin::NickChangeEvent {
                                    old_nick: old.clone(),
                                    new_nick: nick.clone(),
                                    did: conn.authenticated_did.clone(),
                                    session_id: session_id.clone(),
                                });
                            }

                            // Broadcast to S2S
                            if let Some(ref old) = old_nick {
                                let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
                                s2s_broadcast(&state, crate::s2s::S2sMessage::NickChange {
                                    event_id: s2s_next_event_id(&state),
                                    old: old.clone(),
                                    new: nick.clone(),
                                    origin,
                                });
                            }
                        } else {
                            try_complete_registration(
                                &mut conn,
                                &state,
                                &server_name,
                                &session_id,
                                &send,
                            );
                        }
                    }
                }
            }
            "USER" => {
                if msg.params.len() >= 4 {
                    conn.user = Some(msg.params[0].clone());
                    conn.realname = Some(msg.params[3].clone());
                    try_complete_registration(
                        &mut conn,
                        &state,
                        &server_name,
                        &session_id,
                        &send,
                    );
                }
            }
            "PING" => {
                let token = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                let reply =
                    Message::from_server(&server_name, "PONG", vec![&server_name, token]);
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "PONG" => {
                awaiting_pong = false;
            }
            "JOIN" => {
                if !conn.registered {
                    let reply = Message::from_server(
                        &server_name,
                        irc::ERR_NOTREGISTERED,
                        vec![conn.nick_or_star(), "You have not registered"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                if let Some(channels) = msg.params.first() {
                    let keys: Vec<&str> = msg.params.get(1)
                        .map(|k| k.split(',').collect())
                        .unwrap_or_default();
                    for (i, channel) in channels.split(',').enumerate() {
                        let key = keys.get(i).copied();
                        let channel = normalize_channel(channel);
                        handle_join(
                            &conn,
                            &channel,
                            key,
                            &state,
                            &server_name,
                            &session_id,
                            &send,
                        );
                    }
                }
            }
            "PART" => {
                if !conn.registered { continue; }
                if let Some(channels) = msg.params.first() {
                    for channel in channels.split(',') {
                        let channel = normalize_channel(channel);
                        handle_part(&conn, &channel, &state, &session_id);
                    }
                }
            }
            "MODE" => {
                if !conn.registered { continue; }
                if let Some(target) = msg.params.first() {
                    if target.starts_with('#') || target.starts_with('&') {
                        let target = normalize_channel(target);
                        let mode_str = msg.params.get(1).map(|s| s.as_str());
                        let mode_arg = msg.params.get(2).map(|s| s.as_str());
                        handle_mode(
                            &conn, &target, mode_str, mode_arg,
                            &state, &server_name, &session_id, &send,
                        );
                    } else {
                        let reply = Message::from_server(
                            &server_name, "221",
                            vec![conn.nick_or_star(), "+"],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                    }
                }
            }
            "INVITE" => {
                if !conn.registered { continue; }
                if msg.params.len() >= 2 {
                    let target_nick = &msg.params[0];
                    let channel = normalize_channel(&msg.params[1]);
                    handle_invite(
                        &conn, target_nick, &channel,
                        &state, &server_name, &session_id, &send,
                    );
                }
            }
            "KICK" => {
                if !conn.registered { continue; }
                if msg.params.len() >= 2 {
                    let channel = normalize_channel(&msg.params[0]);
                    let target_nick = &msg.params[1];
                    let reason = msg.params.get(2).map(|s| s.as_str()).unwrap_or(conn.nick_or_star());
                    handle_kick(
                        &conn, &channel, target_nick, reason,
                        &state, &server_name, &session_id, &send,
                    );
                }
            }
            "TOPIC" => {
                if !conn.registered { continue; }
                if let Some(channel) = msg.params.first() {
                    let channel = normalize_channel(channel);
                    let new_topic = msg.params.get(1).map(|s| s.as_str());
                    handle_topic(
                        &conn, &channel, new_topic,
                        &state, &server_name, &session_id, &send,
                    );
                }
            }
            "PIN" | "UNPIN" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                if msg.params.len() < 2 {
                    let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                        vec![nick, &msg.command, "Not enough parameters"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                let channel = normalize_channel(&msg.params[0]);
                let msgid = &msg.params[1];
                let is_pin = msg.command == "PIN";

                // Check op status (or server oper)
                let is_op = state.channels.lock()
                    .get(&channel)
                    .map(|ch| ch.ops.contains(&session_id))
                    .unwrap_or(false);
                let is_server_oper = state.server_opers.lock().contains(&session_id);
                if !is_op && !is_server_oper {
                    let reply = Message::from_server(&server_name, irc::ERR_CHANOPRIVSNEEDED,
                        vec![nick, &channel, "You're not channel operator"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }

                let mut channels = state.channels.lock();
                if let Some(ch) = channels.get_mut(&channel) {
                    if is_pin {
                        if ch.pins.iter().any(|p| p.msgid == *msgid) {
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![nick, &format!("Message {msgid} is already pinned in {channel}")]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        } else {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default().as_secs();
                            ch.pins.insert(0, crate::server::PinnedMessage {
                                msgid: msgid.to_string(),
                                pinned_by: nick.to_string(),
                                pinned_at: now,
                            });
                            // Cap at 50 pins
                            ch.pins.truncate(50);
                            drop(channels);
                            // Notify channel
                            let notice = format!(
                                ":{nick}!~u@host NOTICE {channel} :\x01ACTION pinned a message\x01\r\n"
                            );
                            helpers::broadcast_to_channel(&state, &channel, &notice);
                        }
                    } else {
                        let before = ch.pins.len();
                        ch.pins.retain(|p| p.msgid != *msgid);
                        if ch.pins.len() < before {
                            drop(channels);
                            let notice = format!(
                                ":{nick}!~u@host NOTICE {channel} :\x01ACTION unpinned a message\x01\r\n"
                            );
                            helpers::broadcast_to_channel(&state, &channel, &notice);
                        } else {
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![nick, &format!("Message {msgid} is not pinned in {channel}")]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }
                }
            }
            "PINS" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                if let Some(channel) = msg.params.first() {
                    let channel = normalize_channel(channel);
                    let channels = state.channels.lock();
                    if let Some(ch) = channels.get(&channel) {
                        if ch.pins.is_empty() {
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![nick, &format!("No pinned messages in {channel}")]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        } else {
                            for pin in &ch.pins {
                                let reply = Message::from_server(&server_name, "NOTICE",
                                    vec![nick, &format!("PIN {} {} {} {}", channel, pin.msgid, pin.pinned_by, pin.pinned_at)]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                            }
                        }
                    }
                }
            }
            "NAMES" => {
                if !conn.registered { continue; }
                if let Some(channel) = msg.params.first() {
                    let channel = normalize_channel(channel);
                    handle_names(&conn, &channel, &state, &server_name, &session_id, &send);
                }
            }
            "WHOIS" => {
                if !conn.registered { continue; }
                if let Some(target_nick) = msg.params.first() {
                    handle_whois(
                        &conn, target_nick,
                        &state, &server_name, &session_id, &send,
                    );
                }
            }
            "MSGSIG" => {
                // Client registers its session message-signing public key.
                // Usage: MSGSIG <base64url-ed25519-pubkey>
                if !conn.registered { continue; }
                if conn.authenticated_did.is_none() {
                    let reply = irc::Message::from_server(
                        &server_name, "FAIL",
                        vec!["MSGSIG", "NOT_AUTHENTICATED", "Must be DID-authenticated to register a signing key"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                if let Some(pubkey_b64) = msg.params.first() {
                    use base64::Engine;
                    match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(pubkey_b64) {
                        Ok(bytes) if bytes.len() == 32 => {
                            match ed25519_dalek::VerifyingKey::from_bytes(
                                bytes.as_slice().try_into().unwrap()
                            ) {
                                Ok(vk) => {
                                    state.session_msg_keys.lock().insert(session_id.clone(), vk);
                                    if let Some(ref did) = conn.authenticated_did {
                                        state.did_msg_keys.lock().insert(did.clone(), pubkey_b64.clone());
                                    }
                                    tracing::info!(
                                        session = %session_id,
                                        did = ?conn.authenticated_did,
                                        "Client registered message signing key"
                                    );
                                    let reply = irc::Message::from_server(
                                        &server_name, "MSGSIG",
                                        vec!["OK"],
                                    );
                                    send(&state, &session_id, format!("{reply}\r\n"));
                                }
                                Err(_) => {
                                    let reply = irc::Message::from_server(
                                        &server_name, "FAIL",
                                        vec!["MSGSIG", "INVALID_KEY", "Invalid ed25519 public key"],
                                    );
                                    send(&state, &session_id, format!("{reply}\r\n"));
                                }
                            }
                        }
                        _ => {
                            let reply = irc::Message::from_server(
                                &server_name, "FAIL",
                                vec!["MSGSIG", "INVALID_KEY", "Expected 32-byte base64url-encoded ed25519 public key"],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }
                }
            }
            "PRIVMSG" | "NOTICE" => {
                if !conn.registered { continue; }
                if let (Some(target), Some(text)) = (msg.params.first(), msg.params.get(1)) {
                    let target = if target.starts_with('#') || target.starts_with('&') {
                        normalize_channel(target)
                    } else {
                        target.clone()
                    };
                    handle_privmsg(&conn, &msg.command, &target, text, &msg.tags, &state);
                }
            }
            "TAGMSG" => {
                if !conn.registered { continue; }
                if let Some(target) = msg.params.first() {
                    handle_tagmsg(&conn, target, &msg.tags, &state);
                }
            }
            "LIST" => {
                if !conn.registered { continue; }
                handle_list(&conn, &state, &server_name, &session_id, &send);
            }
            "WHO" => {
                if !conn.registered { continue; }
                let target = msg.params.first().map(|s| s.as_str()).unwrap_or("*");
                handle_who(&conn, target, &state, &server_name, &session_id, &send);
            }
            "AWAY" => {
                if !conn.registered { continue; }
                let away_msg = msg.params.first().map(|s| s.as_str());
                handle_away(&conn, away_msg, &state, &server_name, &session_id, &send);
            }
            "MOTD" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                if let Some(ref motd) = state.config.motd {
                    let start = Message::from_server(
                        &server_name, irc::RPL_MOTDSTART,
                        vec![nick, &format!("- {} Message of the day -", server_name)],
                    );
                    send(&state, &session_id, format!("{start}\r\n"));
                    for line in motd.lines() {
                        let motd_line = Message::from_server(
                            &server_name, irc::RPL_MOTD,
                            vec![nick, &format!("- {line}")],
                        );
                        send(&state, &session_id, format!("{motd_line}\r\n"));
                    }
                    let end = Message::from_server(
                        &server_name, irc::RPL_ENDOFMOTD,
                        vec![nick, "End of /MOTD command"],
                    );
                    send(&state, &session_id, format!("{end}\r\n"));
                } else {
                    let no_motd = Message::from_server(
                        &server_name, irc::ERR_NOMOTD,
                        vec![nick, "MOTD File is missing"],
                    );
                    send(&state, &session_id, format!("{no_motd}\r\n"));
                }
            }
            "CHATHISTORY" => {
                if !conn.registered { continue; }
                handle_chathistory(&conn, &msg, &state, &server_name, &session_id, &send);
            }
            "VERSION" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &server_name, irc::RPL_VERSION,
                    vec![nick, "freeq-0.1.0", &server_name, "AT Protocol SASL, IRCv3, iroh QUIC, S2S federation"],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "TIME" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                let now = chrono::Utc::now().format("%a %b %d %Y %H:%M:%S UTC").to_string();
                let reply = Message::from_server(
                    &server_name, irc::RPL_TIME,
                    vec![nick, &server_name, &now],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "LUSERS" => {
                if !conn.registered { continue; }
                handle_lusers(&conn, &state, &server_name, &session_id, &send);
            }
            "USERHOST" => {
                if !conn.registered { continue; }
                let mut replies = Vec::new();
                for nick in msg.params.iter().take(5) {
                    let n2s = state.nick_to_session.lock();
                    if let Some(sid) = n2s.get_session(nick) {
                        let sid = sid.to_string();
                        let is_op = {
                            let channels = state.channels.lock();
                            channels.values().any(|ch| ch.ops.contains(&sid))
                        };
                        let prefix = if is_op { "*" } else { "" };
                        let did = state.session_dids.lock().get(&sid).cloned();
                        let host = helpers::cloaked_host_for_did(did.as_deref());
                        replies.push(format!("{nick}{prefix}=+{nick}@{host}"));
                    }
                }
                let reply = Message::from_server(
                    &server_name, irc::RPL_USERHOST,
                    vec![conn.nick_or_star(), &replies.join(" ")],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "ISON" => {
                if !conn.registered { continue; }
                let n2s = state.nick_to_session.lock();
                let online: Vec<&str> = msg.params.iter()
                    .filter(|nick| n2s.contains_nick(nick))
                    .map(|s| s.as_str())
                    .collect();
                let reply = Message::from_server(
                    &server_name, irc::RPL_ISON,
                    vec![conn.nick_or_star(), &online.join(" ")],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "ADMIN" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                let r1 = Message::from_server(&server_name, irc::RPL_ADMINME, vec![nick, &server_name, "Administrative info"]);
                let r2 = Message::from_server(&server_name, irc::RPL_ADMINLOC1, vec![nick, "freeq IRC server"]);
                let r3 = Message::from_server(&server_name, irc::RPL_ADMINLOC2, vec![nick, "AT Protocol authenticated IRC"]);
                let r4 = Message::from_server(&server_name, irc::RPL_ADMINEMAIL, vec![nick, "https://freeq.at"]);
                for r in [r1, r2, r3, r4] {
                    send(&state, &session_id, format!("{r}\r\n"));
                }
            }
            "INFO" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star();
                let lines = [
                    "freeq - IRC with AT Protocol identity",
                    "",
                    "https://freeq.at",
                    "https://github.com/chad/freeq",
                    "",
                    "SASL ATPROTO-CHALLENGE authentication",
                    "IRCv3 capabilities, E2EE channels, iroh QUIC transport",
                    "Server-to-server federation with CRDT convergence",
                ];
                for line in &lines {
                    let r = Message::from_server(&server_name, irc::RPL_INFO, vec![nick, line]);
                    send(&state, &session_id, format!("{r}\r\n"));
                }
                let end = Message::from_server(&server_name, irc::RPL_ENDOFINFO, vec![nick, "End of /INFO list"]);
                send(&state, &session_id, format!("{end}\r\n"));
            }
            "POLICY" => {
                if !conn.registered { continue; }
                handle_policy(&conn, &msg, &state, &server_name, &session_id, &send);
            }
            "OPER" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                if msg.params.len() < 2 {
                    let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                        vec![&nick, "OPER", "Not enough parameters"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                let _name = &msg.params[0]; // oper name (unused — we just check password)
                let password = &msg.params[1];
                let granted = if let Some(ref oper_pw) = state.config.oper_password {
                    password == oper_pw
                } else {
                    false
                };
                if granted {
                    conn.is_oper = true;
                    state.server_opers.lock().insert(session_id.clone());
                    let reply = Message::from_server(&server_name, "381",
                        vec![&nick, "You are now an IRC operator"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    tracing::info!(nick = %nick, session = %session_id, "OPER granted");
                } else {
                    let reply = Message::from_server(&server_name, "464",
                        vec![&nick, "Password incorrect"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    tracing::warn!(nick = %nick, session = %session_id, "OPER failed: bad password");
                }
            }
            "QUIT" => {
                break;
            }
            _ => {
                if conn.registered {
                    let reply = Message::from_server(
                        &server_name,
                        irc::ERR_UNKNOWNCOMMAND,
                        vec![conn.nick_or_star(), &msg.command, "Unknown command"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                }
            }
        }
    }

    // Check if this DID has other active sessions (multi-device)
    let did = conn.authenticated_did.as_deref();
    let is_last_session_for_did = if let Some(d) = did {
        let mut ds = state.did_sessions.lock();
        if let Some(sessions) = ds.get_mut(d) {
            sessions.remove(&session_id);
            let remaining = sessions.len();
            if sessions.is_empty() { ds.remove(d); }
            remaining == 0
        } else {
            true
        }
    } else {
        true // Guest sessions are always "last"
    };

    // Only broadcast QUIT and remove nick if this is the last session for the DID
    if let Some(ref nick) = conn.nick {
        if is_last_session_for_did {
            let hostmask = conn.hostmask();
            let quit_msg = format!(":{hostmask} QUIT :Connection closed\r\n");
            let channels = state.channels.lock();
            let conns = state.connections.lock();
            for ch in channels.values() {
                if ch.members.contains(&session_id) {
                    for member in &ch.members {
                        if member != &session_id
                            && let Some(tx) = conns.get(member) {
                                let _ = tx.try_send(quit_msg.clone());
                            }
                    }
                }
            }
            drop(conns);
            drop(channels);
            state.nick_to_session.lock().remove_by_nick(nick);

            // Broadcast QUIT to S2S peers only for last session
            let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
            s2s_broadcast(&state, crate::s2s::S2sMessage::Quit {
                event_id: s2s_next_event_id(&state),
                nick: nick.clone(),
                reason: "Connection closed".to_string(),
                origin,
            });
        } else {
            tracing::info!(
                %session_id,
                nick = %nick,
                "Session closed but other sessions remain for DID"
            );
        }
    }

    tracing::info!(
        %session_id,
        nick = conn.nick.as_deref().unwrap_or("-"),
        did = conn.authenticated_did.as_deref().unwrap_or("-"),
        last_session = is_last_session_for_did,
        "Connection closed"
    );
    state.connections.lock().remove(&session_id);
    state.session_dids.lock().remove(&session_id);
    state.session_handles.lock().remove(&session_id);
    state.session_iroh_ids.lock().remove(&session_id);
    state.session_away.lock().remove(&session_id);
    state.msg_timestamps.lock().remove(&session_id);
    state.session_msg_keys.lock().remove(&session_id);
    state.cap_message_tags.lock().remove(&session_id);
    state.cap_multi_prefix.lock().remove(&session_id);
    state.cap_echo_message.lock().remove(&session_id);
    state.cap_server_time.lock().remove(&session_id);
    state.cap_batch.lock().remove(&session_id);
    state.cap_account_notify.lock().remove(&session_id);
    state.cap_extended_join.lock().remove(&session_id);
    state.cap_away_notify.lock().remove(&session_id);
    state.server_opers.lock().remove(&session_id);
    {
        let mut channels = state.channels.lock();
        for ch in channels.values_mut() {
            ch.members.remove(&session_id);
            ch.ops.remove(&session_id);
            ch.voiced.remove(&session_id);
            ch.halfops.remove(&session_id);
        }
        channels.retain(|_, ch| {
            !ch.members.is_empty()
                || !ch.remote_members.is_empty()
                || ch.founder_did.is_some()
                || ch.topic.is_some()
                || !ch.bans.is_empty()
        });
    }

    write_handle.abort();
    Ok(())
}
