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
pub(crate) mod login;
pub mod helpers;
mod messaging;
mod policy_cmd;
mod queries;
mod registration;
pub(crate) mod routing;

use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use base64::Engine;
use crate::irc::{self, Message};
use crate::server::SharedState;

use cap::{handle_authenticate, handle_cap};
use channel::{
    handle_invite, handle_join, handle_kick, handle_list, handle_mode, handle_names, handle_part,
    handle_topic,
};
use helpers::{normalize_channel, s2s_broadcast, s2s_next_event_id};
use messaging::{handle_chathistory, handle_privmsg, handle_tagmsg};
use policy_cmd::handle_policy;
use queries::{handle_away, handle_lusers, handle_who, handle_whois};
use registration::try_complete_registration;

// Re-export items used by other modules in the crate

/// State of a single client connection.
/// Actor class — distinguishes humans from agents in the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorClass {
    Human,
    Agent,
    ExternalAgent,
}

impl Default for ActorClass {
    fn default() -> Self {
        ActorClass::Human
    }
}

impl std::fmt::Display for ActorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActorClass::Human => write!(f, "human"),
            ActorClass::Agent => write!(f, "agent"),
            ActorClass::ExternalAgent => write!(f, "external_agent"),
        }
    }
}

/// Structured agent presence state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentPresence {
    pub state: PresenceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub updated_at: i64,
}

/// Operational state for agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Online,
    Idle,
    Active,
    Executing,
    WaitingForInput,
    BlockedOnPermission,
    BlockedOnBudget,
    Degraded,
    Paused,
    Sandboxed,
    RateLimited,
    Revoked,
    Offline,
}

impl std::fmt::Display for PresenceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}"));
        write!(f, "{s}")
    }
}

impl std::str::FromStr for PresenceState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| format!("Unknown presence state: {s}"))
    }
}

impl std::str::FromStr for ActorClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "human" => Ok(ActorClass::Human),
            "agent" => Ok(ActorClass::Agent),
            "external_agent" => Ok(ActorClass::ExternalAgent),
            _ => Err(format!("Unknown actor class: {s}")),
        }
    }
}

pub struct Connection {
    pub id: String,
    pub nick: Option<String>,
    pub user: Option<String>,
    pub realname: Option<String>,
    pub authenticated_did: Option<String>,
    pub registered: bool,
    /// Actor class: human (default), agent, or external_agent.
    pub(crate) actor_class: ActorClass,

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
    /// Client software identifier (derived from USER realname).
    pub(crate) client_info: Option<String>,
    /// Channels reclaimed from a ghost session, pending synthetic state after registration.
    pub(crate) ghost_channels: Option<Vec<String>>,

    // SASL state
    pub(crate) sasl_in_progress: bool,
    pub(crate) sasl_failures: u8,
    pub(crate) dpop_retries: u8,
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
            actor_class: ActorClass::Human,
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
            client_info: None,
            ghost_channels: None,
            sasl_in_progress: false,
            sasl_failures: 0,
            dpop_retries: 0,
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
    handle_io_with_meta(
        BufReader::new(reader),
        writer,
        session_id,
        state,
        iroh_endpoint_id,
    )
    .await
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
    state
        .plugin_manager
        .on_connect(&crate::plugin::ConnectEvent {
            session_id: session_id.clone(),
            remote_addr: session_id.clone(),
        });

    // Channel for sending messages TO this client
    let (tx, mut rx) = mpsc::channel::<String>(4096);
    state.connections.lock().insert(session_id.clone(), tx);

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
                if batch_count >= 64 {
                    break;
                } // cap batch size
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
        if let Some(tx) = state.connections.lock().get(session_id)
            && tx.try_send(msg).is_err()
        {
            tracing::warn!(session_id, "Send buffer full or closed");
            send_healthy_ref.store(false, std::sync::atomic::Ordering::Relaxed);
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
        let read_result =
            tokio::time::timeout(ping_interval, reader.read_line(&mut line_buf)).await;
        if line_buf.len() > MAX_LINE_LEN {
            tracing::warn!(%session_id, len = line_buf.len(), "Line too long, dropping");
            let reply =
                Message::from_server(&server_name, "417", vec!["*", "Input line was too long"]);
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
        // Exempt read-only and join commands — they burst legitimately on connect
        // when auto-rejoin + client-side JOIN overlap.
        let exempt_from_rate_limit = matches!(
            msg.command.as_str(),
            "JOIN" | "CHATHISTORY" | "WHOIS" | "PING" | "PONG" | "MODE" | "WHO" | "NAMES" | "LOGIN"
        );
        if conn.registered && !exempt_from_rate_limit {
            let now = tokio::time::Instant::now();
            let elapsed = now.duration_since(rate_last).as_secs_f64();
            rate_tokens = (rate_tokens + elapsed * rate_refill).min(rate_max);
            rate_last = now;
            if rate_tokens < 1.0 {
                tracing::debug!(%session_id, "Rate limited");
                // Warn the user (only once per burst)
                if rate_tokens > -1.0 {
                    let notice = Message::from_server(
                        &server_name,
                        "NOTICE",
                        vec!["*", "Flood protection: you are sending commands too fast"],
                    );
                    send(&state, &session_id, format!("{notice}\r\n"));
                }
                continue;
            }
            rate_tokens -= 1.0;
        }

        tracing::debug!(%session_id, "<- {}", line_buf.trim());

        // Check for pending LOGIN completion (from browser OAuth callback)
        if conn.authenticated_did.is_none() {
            if let Some(completion) = state.login_completions.lock().remove(&session_id) {
                conn.authenticated_did = Some(completion.did.clone());
                // Trigger auto-op etc. in channels (already handled by complete_irc_login)
            }
        }

        match msg.command.as_str() {
            "CAP" => {
                handle_cap(&mut conn, &msg, &state, &server_name, &session_id, &send);
            }
            "AUTHENTICATE" => {
                handle_authenticate(&mut conn, &msg, &state, &server_name, &session_id, &send)
                    .await;
            }
            "NICK" => {
                if let Some(nick) = msg.params.first() {
                    // Validate nick: 1-64 chars, allowed chars for IRC + AT handles
                    if nick.is_empty()
                        || nick.len() > 64
                        || nick.contains(|c: char| {
                            c.is_control()
                                || c == ' '
                                || c == '\0'
                                || c == '\r'
                                || c == '\n'
                                || c == ','
                                || c == '*'
                                || c == '?'
                                || c == '!'
                                || c == '@'
                                || c == '#'
                                || c == '&'
                                || c == ':'
                        })
                    {
                        let reply = Message::from_server(
                            &server_name,
                            "432",
                            vec![conn.nick_or_star(), nick, "Erroneous Nickname"],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                        continue;
                    }
                    let nick_lower = nick.to_lowercase();
                    let in_use_by_session = state
                        .nick_to_session
                        .lock()
                        .get_session(nick)
                        .map(|s| s.to_string());
                    let in_use = in_use_by_session.is_some();

                    // Check if the nick is in use by the same DID (multi-device OK)
                    let in_use_by_same_did = in_use_by_session.as_ref().is_some_and(|sid| {
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
                        owner_did
                            .as_ref()
                            .is_some_and(|owner| my_did.is_none_or(|my| my != owner))
                    };

                    if in_use && !in_use_by_same_did {
                        // During CAP/SASL negotiation, allow the nick if it's owned
                        // by a DID (attach_same_did will handle multi-device at SASL success).
                        let allow_during_negotiation =
                            (conn.cap_negotiating || conn.sasl_in_progress) && owner_did.is_some();
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
                            vec![
                                conn.nick_or_star(),
                                nick,
                                "Nickname is registered to another identity",
                            ],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                    } else {
                        let old_nick = conn.nick.clone();
                        if let Some(ref old) = old_nick {
                            state.nick_to_session.lock().remove_by_nick(old);
                        }
                        state.nick_to_session.lock().insert(nick, &session_id);
                        conn.nick = Some(nick.clone());

                        if conn.registered {
                            let hostmask = if let Some(ref old) = old_nick {
                                format!(
                                    "{old}!~{}@{}",
                                    conn.user.as_deref().unwrap_or("u"),
                                    conn.cloaked_host()
                                )
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
                                        if notified.insert(member.clone())
                                            && let Some(tx) = conns.get(member)
                                        {
                                            let _ = tx.try_send(nick_msg.clone());
                                        }
                                    }
                                }
                            }
                            drop(conns);
                            drop(channels);

                            // Plugin on_nick_change hook
                            if let Some(ref old) = old_nick {
                                state.plugin_manager.on_nick_change(
                                    &crate::plugin::NickChangeEvent {
                                        old_nick: old.clone(),
                                        new_nick: nick.clone(),
                                        did: conn.authenticated_did.clone(),
                                        session_id: session_id.clone(),
                                    },
                                );
                            }

                            // Broadcast to S2S
                            if let Some(ref old) = old_nick {
                                let origin =
                                    state.server_iroh_id.lock().clone().unwrap_or_default();
                                s2s_broadcast(
                                    &state,
                                    crate::s2s::S2sMessage::NickChange {
                                        event_id: s2s_next_event_id(&state),
                                        old: old.clone(),
                                        new: nick.clone(),
                                        origin,
                                    },
                                );
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
                    let realname = msg.params[3].clone();
                    // Derive client info from realname
                    let client = detect_client(&realname);
                    state
                        .session_client_info
                        .lock()
                        .insert(session_id.clone(), client.clone());
                    conn.client_info = Some(client);
                    conn.realname = Some(realname);
                    try_complete_registration(&mut conn, &state, &server_name, &session_id, &send);
                }
            }
            "PING" => {
                let token = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                let reply = Message::from_server(&server_name, "PONG", vec![&server_name, token]);
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
                    let keys: Vec<&str> = msg
                        .params
                        .get(1)
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
                if !conn.registered {
                    continue;
                }
                if let Some(channels) = msg.params.first() {
                    for channel in channels.split(',') {
                        let channel = normalize_channel(channel);
                        handle_part(&conn, &channel, &state, &session_id);
                    }
                }
            }
            "MODE" => {
                if !conn.registered {
                    continue;
                }
                if let Some(target) = msg.params.first() {
                    if target.starts_with('#') || target.starts_with('&') {
                        let target = normalize_channel(target);
                        let mode_str = msg.params.get(1).map(|s| s.as_str());
                        let mode_arg = msg.params.get(2).map(|s| s.as_str());
                        handle_mode(
                            &conn,
                            &target,
                            mode_str,
                            mode_arg,
                            &state,
                            &server_name,
                            &session_id,
                            &send,
                        );
                    } else {
                        let reply = Message::from_server(
                            &server_name,
                            "221",
                            vec![conn.nick_or_star(), "+"],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                    }
                }
            }
            "INVITE" => {
                if !conn.registered {
                    continue;
                }
                if msg.params.len() >= 2 {
                    let target_nick = &msg.params[0];
                    let channel = normalize_channel(&msg.params[1]);
                    handle_invite(
                        &conn,
                        target_nick,
                        &channel,
                        &state,
                        &server_name,
                        &session_id,
                        &send,
                    );
                }
            }
            "KICK" => {
                if !conn.registered {
                    continue;
                }
                if msg.params.len() >= 2 {
                    let channel = normalize_channel(&msg.params[0]);
                    let target_nick = &msg.params[1];
                    let reason = msg
                        .params
                        .get(2)
                        .map(|s| s.as_str())
                        .unwrap_or(conn.nick_or_star());
                    handle_kick(
                        &conn,
                        &channel,
                        target_nick,
                        reason,
                        &state,
                        &server_name,
                        &session_id,
                        &send,
                    );
                }
            }
            "TOPIC" => {
                if !conn.registered {
                    continue;
                }
                if let Some(channel) = msg.params.first() {
                    let channel = normalize_channel(channel);
                    let new_topic = msg.params.get(1).map(|s| s.as_str());
                    handle_topic(
                        &conn,
                        &channel,
                        new_topic,
                        &state,
                        &server_name,
                        &session_id,
                        &send,
                    );
                }
            }
            "PIN" | "UNPIN" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star();
                if msg.params.len() < 2 {
                    let reply = Message::from_server(
                        &server_name,
                        irc::ERR_NEEDMOREPARAMS,
                        vec![nick, &msg.command, "Not enough parameters"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                let channel = normalize_channel(&msg.params[0]);
                let msgid = &msg.params[1];
                let is_pin = msg.command == "PIN";

                // Check op status (or server oper)
                let is_op = state
                    .channels
                    .lock()
                    .get(&channel)
                    .map(|ch| ch.ops.contains(&session_id))
                    .unwrap_or(false);
                let is_server_oper = state.server_opers.lock().contains(&session_id);
                if !is_op && !is_server_oper {
                    let reply = Message::from_server(
                        &server_name,
                        irc::ERR_CHANOPRIVSNEEDED,
                        vec![nick, &channel, "You're not channel operator"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }

                let mut channels = state.channels.lock();
                if let Some(ch) = channels.get_mut(&channel) {
                    if is_pin {
                        if ch.pins.iter().any(|p| p.msgid == *msgid) {
                            let reply = Message::from_server(
                                &server_name,
                                "NOTICE",
                                vec![
                                    nick,
                                    &format!("Message {msgid} is already pinned in {channel}"),
                                ],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        } else {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            ch.pins.insert(
                                0,
                                crate::server::PinnedMessage {
                                    msgid: msgid.to_string(),
                                    pinned_by: nick.to_string(),
                                    pinned_at: now,
                                },
                            );
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
                            let reply = Message::from_server(
                                &server_name,
                                "NOTICE",
                                vec![nick, &format!("Message {msgid} is not pinned in {channel}")],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }
                }
            }
            "PINS" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star();
                if let Some(channel) = msg.params.first() {
                    let channel = normalize_channel(channel);
                    let channels = state.channels.lock();
                    if let Some(ch) = channels.get(&channel) {
                        if ch.pins.is_empty() {
                            let reply = Message::from_server(
                                &server_name,
                                "NOTICE",
                                vec![nick, &format!("No pinned messages in {channel}")],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        } else {
                            for pin in &ch.pins {
                                let reply = Message::from_server(
                                    &server_name,
                                    "NOTICE",
                                    vec![
                                        nick,
                                        &format!(
                                            "PIN {} {} {} {}",
                                            channel, pin.msgid, pin.pinned_by, pin.pinned_at
                                        ),
                                    ],
                                );
                                send(&state, &session_id, format!("{reply}\r\n"));
                            }
                        }
                    }
                }
            }
            "NAMES" => {
                if !conn.registered {
                    continue;
                }
                if let Some(channel) = msg.params.first() {
                    let channel = normalize_channel(channel);
                    handle_names(&conn, &channel, &state, &server_name, &session_id, &send);
                }
            }
            "WHOIS" => {
                if !conn.registered {
                    continue;
                }
                if let Some(target_nick) = msg.params.first() {
                    handle_whois(&conn, target_nick, &state, &server_name, &session_id, &send);
                }
            }
            "MSGSIG" => {
                // Client registers its session message-signing public key.
                // Usage: MSGSIG <base64url-ed25519-pubkey>
                if !conn.registered {
                    continue;
                }
                if conn.authenticated_did.is_none() {
                    let reply = irc::Message::from_server(
                        &server_name,
                        "FAIL",
                        vec![
                            "MSGSIG",
                            "NOT_AUTHENTICATED",
                            "Must be DID-authenticated to register a signing key",
                        ],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                if let Some(pubkey_b64) = msg.params.first() {
                    use base64::Engine;
                    match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(pubkey_b64) {
                        Ok(bytes) if bytes.len() == 32 => {
                            match ed25519_dalek::VerifyingKey::from_bytes(
                                bytes.as_slice().try_into().unwrap(),
                            ) {
                                Ok(vk) => {
                                    state.session_msg_keys.lock().insert(session_id.clone(), vk);
                                    if let Some(ref did) = conn.authenticated_did {
                                        state
                                            .did_msg_keys
                                            .lock()
                                            .insert(did.clone(), pubkey_b64.clone());
                                    }
                                    tracing::info!(
                                        session = %session_id,
                                        did = ?conn.authenticated_did,
                                        "Client registered message signing key"
                                    );
                                    let reply = irc::Message::from_server(
                                        &server_name,
                                        "MSGSIG",
                                        vec!["OK"],
                                    );
                                    send(&state, &session_id, format!("{reply}\r\n"));
                                }
                                Err(_) => {
                                    let reply = irc::Message::from_server(
                                        &server_name,
                                        "FAIL",
                                        vec!["MSGSIG", "INVALID_KEY", "Invalid ed25519 public key"],
                                    );
                                    send(&state, &session_id, format!("{reply}\r\n"));
                                }
                            }
                        }
                        _ => {
                            let reply = irc::Message::from_server(
                                &server_name,
                                "FAIL",
                                vec![
                                    "MSGSIG",
                                    "INVALID_KEY",
                                    "Expected 32-byte base64url-encoded ed25519 public key",
                                ],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }
                }
            }
            "PRIVMSG" | "NOTICE" => {
                if !conn.registered {
                    continue;
                }
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
                if !conn.registered {
                    continue;
                }
                if let Some(target) = msg.params.first() {
                    handle_tagmsg(&conn, target, &msg.tags, &state);
                }
            }
            "LIST" => {
                if !conn.registered {
                    continue;
                }
                handle_list(&conn, &state, &server_name, &session_id, &send);
            }
            "WHO" => {
                if !conn.registered {
                    continue;
                }
                let target = msg.params.first().map(|s| s.as_str()).unwrap_or("*");
                handle_who(&conn, target, &state, &server_name, &session_id, &send);
            }
            "AWAY" => {
                if !conn.registered {
                    continue;
                }
                let away_msg = msg.params.first().map(|s| s.as_str());
                handle_away(&conn, away_msg, &state, &server_name, &session_id, &send);
            }
            "MOTD" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star();
                if let Some(ref motd) = state.config.motd {
                    let start = Message::from_server(
                        &server_name,
                        irc::RPL_MOTDSTART,
                        vec![nick, &format!("- {} Message of the day -", server_name)],
                    );
                    send(&state, &session_id, format!("{start}\r\n"));
                    for line in motd.lines() {
                        let motd_line = Message::from_server(
                            &server_name,
                            irc::RPL_MOTD,
                            vec![nick, &format!("- {line}")],
                        );
                        send(&state, &session_id, format!("{motd_line}\r\n"));
                    }
                    let end = Message::from_server(
                        &server_name,
                        irc::RPL_ENDOFMOTD,
                        vec![nick, "End of /MOTD command"],
                    );
                    send(&state, &session_id, format!("{end}\r\n"));
                } else {
                    let no_motd = Message::from_server(
                        &server_name,
                        irc::ERR_NOMOTD,
                        vec![nick, "MOTD File is missing"],
                    );
                    send(&state, &session_id, format!("{no_motd}\r\n"));
                }
            }
            "CHATHISTORY" => {
                if !conn.registered {
                    continue;
                }
                handle_chathistory(&conn, &msg, &state, &server_name, &session_id, &send);
            }
            "VERSION" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star();
                let reply = Message::from_server(
                    &server_name,
                    irc::RPL_VERSION,
                    vec![
                        nick,
                        "freeq-0.1.0",
                        &server_name,
                        "AT Protocol SASL, IRCv3, iroh QUIC, S2S federation",
                    ],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "TIME" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star();
                let now = chrono::Utc::now()
                    .format("%a %b %d %Y %H:%M:%S UTC")
                    .to_string();
                let reply = Message::from_server(
                    &server_name,
                    irc::RPL_TIME,
                    vec![nick, &server_name, &now],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "LUSERS" => {
                if !conn.registered {
                    continue;
                }
                handle_lusers(&conn, &state, &server_name, &session_id, &send);
            }
            "USERHOST" => {
                if !conn.registered {
                    continue;
                }
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
                    &server_name,
                    irc::RPL_USERHOST,
                    vec![conn.nick_or_star(), &replies.join(" ")],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "ISON" => {
                if !conn.registered {
                    continue;
                }
                let n2s = state.nick_to_session.lock();
                let online: Vec<&str> = msg
                    .params
                    .iter()
                    .filter(|nick| n2s.contains_nick(nick))
                    .map(|s| s.as_str())
                    .collect();
                let reply = Message::from_server(
                    &server_name,
                    irc::RPL_ISON,
                    vec![conn.nick_or_star(), &online.join(" ")],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
            }
            "ADMIN" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star();
                let r1 = Message::from_server(
                    &server_name,
                    irc::RPL_ADMINME,
                    vec![nick, &server_name, "Administrative info"],
                );
                let r2 = Message::from_server(
                    &server_name,
                    irc::RPL_ADMINLOC1,
                    vec![nick, "freeq IRC server"],
                );
                let r3 = Message::from_server(
                    &server_name,
                    irc::RPL_ADMINLOC2,
                    vec![nick, "AT Protocol authenticated IRC"],
                );
                let r4 = Message::from_server(
                    &server_name,
                    irc::RPL_ADMINEMAIL,
                    vec![nick, "https://freeq.at"],
                );
                for r in [r1, r2, r3, r4] {
                    send(&state, &session_id, format!("{r}\r\n"));
                }
            }
            "INFO" => {
                if !conn.registered {
                    continue;
                }
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
                let end = Message::from_server(
                    &server_name,
                    irc::RPL_ENDOFINFO,
                    vec![nick, "End of /INFO list"],
                );
                send(&state, &session_id, format!("{end}\r\n"));
            }
            "LOGIN" => {
                if !conn.registered {
                    continue;
                }
                let handle = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                login::handle_login(&mut conn, handle, &state, &server_name, &session_id, &send);
            }
            "POLICY" => {
                if !conn.registered {
                    continue;
                }
                handle_policy(&conn, &msg, &state, &server_name, &session_id, &send);
            }
            "OPER" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star().to_string();
                if msg.params.len() < 2 {
                    let reply = Message::from_server(
                        &server_name,
                        irc::ERR_NEEDMOREPARAMS,
                        vec![&nick, "OPER", "Not enough parameters"],
                    );
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
                    let reply = Message::from_server(
                        &server_name,
                        "381",
                        vec![&nick, "You are now an IRC operator"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    tracing::info!(nick = %nick, session = %session_id, "OPER granted");
                } else {
                    let reply = Message::from_server(
                        &server_name,
                        "464",
                        vec![&nick, "Password incorrect"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    tracing::warn!(nick = %nick, session = %session_id, "OPER failed: bad password");
                }
            }
            // AGENT command — register as an agent or manage agent state.
            // Usage: AGENT REGISTER :class=agent
            "AGENT" => {
                if !conn.registered {
                    continue;
                }
                let nick = conn.nick_or_star().to_string();
                if msg.params.is_empty() {
                    let reply = Message::from_server(
                        &server_name,
                        irc::ERR_NEEDMOREPARAMS,
                        vec![&nick, "AGENT", "Not enough parameters"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                let subcmd = msg.params[0].to_uppercase();
                match subcmd.as_str() {
                    "REGISTER" => {
                        // Parse class from trailing param: "class=agent"
                        let class_str = msg.params.get(1)
                            .and_then(|p| p.strip_prefix("class="))
                            .unwrap_or("agent");
                        match class_str.parse::<ActorClass>() {
                            Ok(class) => {
                                conn.actor_class = class;
                                // Store in shared state for WHOIS / member list lookups
                                state.session_actor_class.lock().insert(
                                    session_id.clone(),
                                    class,
                                );
                                let reply = Message::from_server(
                                    &server_name,
                                    "NOTICE",
                                    vec![&nick, &format!("Agent registered as {class}")],
                                );
                                send(&state, &session_id, format!("{reply}\r\n"));
                                tracing::info!(nick = %nick, session = %session_id, actor_class = %class, "AGENT REGISTER");

                                // Broadcast actor class to shared channels if they support the cap
                                if conn.cap_message_tags {
                                    let hostmask = conn.hostmask();
                                    // Collect targets first, then send (avoid holding multiple locks)
                                    let targets: Vec<(String, Vec<String>)> = {
                                        let channels = state.channels.lock();
                                        channels.iter()
                                            .filter(|(_, ch)| ch.members.contains(&session_id))
                                            .map(|(ch_name, ch)| {
                                                let members: Vec<String> = ch.members.iter()
                                                    .filter(|sid| *sid != &session_id)
                                                    .cloned()
                                                    .collect();
                                                (ch_name.clone(), members)
                                            })
                                            .collect()
                                    };
                                    let conns = state.connections.lock();
                                    for (ch_name, members) in &targets {
                                        let msg_line = format!(
                                            "@+freeq.at/actor-class={class} :{hostmask} NOTICE {ch_name} :registered as {class}\r\n"
                                        );
                                        for member_sid in members {
                                            if let Some(tx) = conns.get(member_sid) {
                                                let _ = tx.try_send(msg_line.clone());
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let reply = Message::from_server(
                                    &server_name,
                                    "NOTICE",
                                    vec![&nick, &format!("Invalid actor class: {e}")],
                                );
                                send(&state, &session_id, format!("{reply}\r\n"));
                            }
                        }
                    }
                    // ── Phase 2: Governance signals ──────────────────
                    // AGENT PAUSE <nick> [reason]
                    "PAUSE" | "RESUME" | "REVOKE" => {
                        let target_nick = match msg.params.get(1) {
                            Some(n) => n.clone(),
                            None => {
                                let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                    vec![&nick, "AGENT", "Usage: AGENT PAUSE/RESUME/REVOKE <nick> [reason]"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };
                        let reason = msg.params.get(2).cloned();
                        let action = subcmd.to_lowercase();

                        // Find target session
                        let target_session = state.nick_to_session.lock().get_session(&target_nick).map(|s| s.to_string());
                        let target_session = match target_session {
                            Some(s) => s,
                            None => {
                                let reply = Message::from_server(&server_name, irc::ERR_NOSUCHNICK,
                                    vec![&nick, &target_nick, "No such nick"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };

                        // Verify sender is op in a shared channel OR server oper
                        let is_oper = conn.is_oper;
                        let is_op_in_shared = {
                            let channels = state.channels.lock();
                            channels.values().any(|ch| {
                                ch.members.contains(&session_id)
                                    && ch.members.contains(&target_session)
                                    && ch.ops.contains(&session_id)
                            })
                        };
                        if !is_oper && !is_op_in_shared {
                            let reply = Message::from_server(&server_name, "482",
                                vec![&nick, "You must be an op in a shared channel"]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                            continue;
                        }

                        // Send governance TAGMSG to the target agent
                        let reason_tag = reason.as_deref().map(|r| format!(";+freeq.at/reason={r}")).unwrap_or_default();
                        let hostmask = conn.hostmask();
                        let gov_msg = format!(
                            "@+freeq.at/governance={action};+freeq.at/issued-by={}{reason_tag} :{hostmask} TAGMSG {target_nick}\r\n",
                            nick
                        );
                        if let Some(tx) = state.connections.lock().get(&target_session) {
                            let _ = tx.try_send(gov_msg);
                        }

                        // Broadcast human-readable notice to shared channels
                        let emoji = match action.as_str() {
                            "pause" => "⏸",
                            "resume" => "▶",
                            "revoke" => "❌",
                            _ => "🔧",
                        };
                        let reason_str = reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default();
                        let notice_text = format!("{emoji} {target_nick} {action}d by {nick}{reason_str}");
                        {
                            let shared_channels: Vec<String> = {
                                let channels = state.channels.lock();
                                channels.iter()
                                    .filter(|(_, ch)| ch.members.contains(&session_id) && ch.members.contains(&target_session))
                                    .map(|(name, _)| name.clone())
                                    .collect()
                            };
                            for ch_name in &shared_channels {
                                helpers::broadcast_to_channel(
                                    &state, ch_name,
                                    &format!(":{server_name} NOTICE {ch_name} :{notice_text}\r\n"),
                                );
                            }
                        }

                        // Log to DB
                        let target_did = state.session_dids.lock().get(&target_session).cloned();
                        if let Some(ref did) = target_did {
                            let issuer_did = conn.authenticated_did.as_deref().unwrap_or(&nick);
                            state.with_db(|db| db.log_governance(None, did, &action, issuer_did, reason.as_deref()));
                        }

                        // For REVOKE: also revoke all capabilities and force part
                        if action == "revoke" {
                            if let Some(ref did) = target_did {
                                let channels: Vec<String> = {
                                    let chs = state.channels.lock();
                                    chs.iter()
                                        .filter(|(_, ch)| ch.members.contains(&target_session))
                                        .map(|(name, _)| name.clone())
                                        .collect()
                                };
                                for ch in &channels {
                                    state.with_db(|db| db.revoke_all_capabilities(ch, did));
                                }
                            }
                            // Send ERROR to force disconnect
                            if let Some(tx) = state.connections.lock().get(&target_session) {
                                let _ = tx.try_send(format!("ERROR :Revoked by {nick}{reason_str}\r\n"));
                            }
                        }

                        tracing::info!(action = %action, target = %target_nick, by = %nick, "AGENT governance");
                    }

                    // AGENT APPROVE <nick> <capability>
                    "APPROVE" => {
                        let (target_nick, capability) = match (msg.params.get(1), msg.params.get(2)) {
                            (Some(n), Some(c)) => (n.clone(), c.clone()),
                            _ => {
                                let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                    vec![&nick, "AGENT", "Usage: AGENT APPROVE <nick> <capability>"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };

                        let target_did = {
                            let ts = state.nick_to_session.lock().get_session(&target_nick).map(|s| s.to_string());
                            ts.and_then(|sid| state.session_dids.lock().get(&sid).cloned())
                        };

                        if let Some(ref did) = target_did {
                            let issuer_did = conn.authenticated_did.as_deref().unwrap_or(&nick);
                            // Find pending approval in any shared channel
                            let approval = {
                                let shared: Vec<String> = {
                                    let channels = state.channels.lock();
                                    channels.keys().cloned().collect()
                                };
                                shared.into_iter().find_map(|ch| {
                                    state.with_db(|db| Ok(db.find_pending_approval_for_agent(&ch, did, &capability))).flatten()
                                })
                            };

                            if let Some(approval) = approval {
                                let granted = state.with_db(|db| db.grant_approval(&approval.id, issuer_did))
                                    .unwrap_or(false);
                                if granted {
                                    // Notify agent
                                    let target_session = state.nick_to_session.lock().get_session(&target_nick).map(|s| s.to_string());
                                    if let Some(ref ts) = target_session {
                                        let line = format!(
                                            "@+freeq.at/governance=approval_granted;+freeq.at/capability={capability} :{server_name} TAGMSG {target_nick}\r\n"
                                        );
                                        if let Some(tx) = state.connections.lock().get(ts) {
                                            let _ = tx.try_send(line);
                                        }
                                    }
                                    // Notify channel
                                    let notice = format!(":{server_name} NOTICE {} :✅ {nick} approved '{capability}' for {target_nick}\r\n", approval.channel);
                                    helpers::broadcast_to_channel(&state, &approval.channel, &notice);

                                    state.with_db(|db| db.log_governance(Some(&approval.channel), did, "approve", issuer_did, Some(&capability)));
                                    tracing::info!(target = %target_nick, capability = %capability, by = %nick, "Approval granted");
                                }
                            } else {
                                let reply = Message::from_server(&server_name, "NOTICE",
                                    vec![&nick, &format!("No pending approval for {target_nick}/{capability}")]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                            }
                        } else {
                            let reply = Message::from_server(&server_name, irc::ERR_NOSUCHNICK,
                                vec![&nick, &target_nick, "No such nick or not authenticated"]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }

                    // AGENT DENY <nick> <capability> [reason]
                    "DENY" => {
                        let (target_nick, capability) = match (msg.params.get(1), msg.params.get(2)) {
                            (Some(n), Some(c)) => (n.clone(), c.clone()),
                            _ => {
                                let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                    vec![&nick, "AGENT", "Usage: AGENT DENY <nick> <capability> [reason]"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };
                        let reason = msg.params.get(3).cloned();
                        let target_did = {
                            let ts = state.nick_to_session.lock().get_session(&target_nick).map(|s| s.to_string());
                            ts.and_then(|sid| state.session_dids.lock().get(&sid).cloned())
                        };

                        if let Some(ref did) = target_did {
                            let issuer_did = conn.authenticated_did.as_deref().unwrap_or(&nick);
                            let shared: Vec<String> = state.channels.lock().keys().cloned().collect();
                            let approval = shared.into_iter().find_map(|ch| {
                                state.with_db(|db| Ok(db.find_pending_approval_for_agent(&ch, did, &capability))).flatten()
                            });

                            if let Some(approval) = approval {
                                let denied = state.with_db(|db| db.deny_approval(&approval.id, issuer_did, reason.as_deref()))
                                    .unwrap_or(false);
                                if denied {
                                    let target_session = state.nick_to_session.lock().get_session(&target_nick).map(|s| s.to_string());
                                    if let Some(ref ts) = target_session {
                                        let reason_tag = reason.as_deref().map(|r| format!(";+freeq.at/reason={r}")).unwrap_or_default();
                                        let line = format!(
                                            "@+freeq.at/governance=approval_denied;+freeq.at/capability={capability}{reason_tag} :{server_name} TAGMSG {target_nick}\r\n"
                                        );
                                        if let Some(tx) = state.connections.lock().get(ts) {
                                            let _ = tx.try_send(line);
                                        }
                                    }
                                    let reason_str = reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default();
                                    let notice = format!(":{server_name} NOTICE {} :❌ {nick} denied '{capability}' for {target_nick}{reason_str}\r\n", approval.channel);
                                    helpers::broadcast_to_channel(&state, &approval.channel, &notice);
                                    tracing::info!(target = %target_nick, capability = %capability, by = %nick, "Approval denied");
                                }
                            }
                        }
                    }

                    // ── Phase 4: Manifests ──────────────────────────
                    // AGENT MANIFEST <url-or-inline-toml>
                    "MANIFEST" => {
                        let raw = msg.params.get(1).cloned().unwrap_or_default();
                        if raw.is_empty() {
                            let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                vec![&nick, "AGENT", "Usage: AGENT MANIFEST <url-or-inline-toml>"]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                            continue;
                        }

                        // Try to parse as TOML first, then as URL
                        let manifest_result = if raw.starts_with("http://") || raw.starts_with("https://") {
                            // URL — we don't fetch in tests, just store the URL
                            Err("URL manifests require REST API".to_string())
                        } else {
                            // Inline TOML (base64-encoded to survive IRC)
                            use base64::Engine;
                            match base64::engine::general_purpose::STANDARD.decode(&raw) {
                                Ok(bytes) => match String::from_utf8(bytes) {
                                    Ok(toml_str) => crate::manifest::AgentManifest::from_toml(&toml_str)
                                        .map_err(|e| format!("Invalid TOML: {e}")),
                                    Err(e) => Err(format!("Invalid UTF-8: {e}")),
                                },
                                Err(_) => crate::manifest::AgentManifest::from_toml(&raw)
                                    .map_err(|e| format!("Invalid TOML: {e}")),
                            }
                        };

                        match manifest_result {
                            Ok(manifest) => {
                                if let Err(e) = manifest.validate() {
                                    let reply = Message::from_server(&server_name, "NOTICE",
                                        vec![&nick, &format!("Invalid manifest: {e}")]);
                                    send(&state, &session_id, format!("{reply}\r\n"));
                                    continue;
                                }
                                let agent_did = conn.authenticated_did.as_deref().unwrap_or("unknown");
                                let registrar = conn.authenticated_did.as_deref().unwrap_or(&nick);
                                let manifest_json = manifest.to_json();
                                state.with_db(|db| db.save_manifest(agent_did, &manifest_json, None, registrar));

                                // Auto-apply: set actor class + provenance
                                if let Ok(class) = manifest.agent.actor_class.parse::<ActorClass>() {
                                    conn.actor_class = class;
                                    state.session_actor_class.lock().insert(session_id.clone(), class);
                                }

                                let caps = manifest.capabilities.default.join(", ");
                                let reply = Message::from_server(&server_name, "NOTICE",
                                    vec![&nick, &format!("✅ Manifest registered for {}. Capabilities: {caps}", manifest.agent.display_name)]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                tracing::info!(nick = %nick, display_name = %manifest.agent.display_name, "AGENT MANIFEST registered");
                            }
                            Err(e) => {
                                let reply = Message::from_server(&server_name, "NOTICE",
                                    vec![&nick, &format!("Manifest error: {e}")]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                            }
                        }
                    }

                    // ── Phase 4: Spawn ──────────────────────────────
                    // AGENT SPAWN #channel :nick=child;capabilities=cap1,cap2;ttl=300;task=01JQXYZ
                    "SPAWN" => {
                        let channel = match msg.params.get(1) {
                            Some(c) if c.starts_with('#') => c.clone(),
                            _ => {
                                let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                    vec![&nick, "AGENT", "Usage: AGENT SPAWN #channel :nick=name;capabilities=a,b;ttl=300;task=id"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };
                        let params_str = msg.params.get(2).cloned().unwrap_or_default();
                        let mut child_nick = String::new();
                        let mut capabilities: Vec<String> = Vec::new();
                        let mut ttl: Option<u64> = None;
                        let mut task_ref: Option<String> = None;
                        for part in params_str.split(';') {
                            if let Some((k, v)) = part.split_once('=') {
                                match k.trim() {
                                    "nick" => child_nick = v.trim().to_string(),
                                    "capabilities" => capabilities = v.split(',').map(|s| s.trim().to_string()).collect(),
                                    "ttl" => ttl = v.trim().parse().ok(),
                                    "task" => task_ref = Some(v.trim().to_string()),
                                    _ => {}
                                }
                            }
                        }
                        if child_nick.is_empty() {
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![&nick, "SPAWN requires nick= parameter"]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                            continue;
                        }

                        // Check nick availability
                        if state.nick_to_session.lock().get_session(&child_nick).is_some() {
                            let reply = Message::from_server(&server_name, "433",
                                vec![&nick, &child_nick, "Nickname already in use"]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                            continue;
                        }

                        // Generate session-scoped DID for child
                        let child_did = format!("did:freeq:spawn:{}", crate::msgid::generate());
                        let parent_did = conn.authenticated_did.clone().unwrap_or_else(|| nick.clone());

                        // Record spawn
                        state.with_db(|db| db.record_spawn(
                            &child_did, &parent_did, &session_id,
                            &child_nick, &channel, &capabilities,
                            ttl, task_ref.as_deref(),
                        ));

                        // Store in spawned_agents map (in-memory for fast lookup)
                        state.spawned_agents.lock().insert(child_did.clone(), crate::server::SpawnedAgent {
                            child_did: child_did.clone(),
                            parent_did: parent_did.clone(),
                            parent_session: session_id.clone(),
                            nick: child_nick.clone(),
                            channel: channel.clone(),
                            capabilities: capabilities.clone(),
                            ttl,
                            task_ref: task_ref.clone(),
                            spawned_at: chrono::Utc::now().timestamp(),
                        });

                        // Broadcast JOIN for child to channel
                        let child_hostmask = format!("{child_nick}!spawn@freeq/spawn/{}", &child_did[child_did.len().saturating_sub(8)..]);
                        let join_line = format!(
                            "@+freeq.at/actor-class=agent;+freeq.at/parent={} :{child_hostmask} JOIN {channel}\r\n",
                            nick
                        );
                        helpers::broadcast_to_channel(&state, &channel, &join_line);

                        // Set up TTL expiry
                        if let Some(ttl_secs) = ttl {
                            let state_clone = Arc::clone(&state);
                            let child_did_clone = child_did.clone();
                            let child_nick_clone = child_nick.clone();
                            let channel_clone = channel.clone();
                            let server_name_clone = server_name.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(ttl_secs)).await;
                                // Auto-despawn
                                if state_clone.spawned_agents.lock().remove(&child_did_clone).is_some() {
                                    let quit_line = format!(
                                        ":{child_nick_clone}!spawn@freeq/spawn QUIT :TTL expired\r\n"
                                    );
                                    helpers::broadcast_to_channel(&state_clone, &channel_clone, &quit_line);
                                    let notice = format!(
                                        ":{server_name_clone} NOTICE {channel_clone} :⏱ Spawned agent {child_nick_clone} expired (TTL)\r\n"
                                    );
                                    helpers::broadcast_to_channel(&state_clone, &channel_clone, &notice);
                                    state_clone.with_db(|db| db.record_despawn(&child_did_clone));
                                    tracing::info!(child = %child_nick_clone, "Spawned agent TTL expired");
                                }
                            });
                        }

                        let caps_str = if capabilities.is_empty() { "none".to_string() } else { capabilities.join(", ") };
                        let ttl_str = ttl.map(|t| format!(", TTL: {t}s")).unwrap_or_default();
                        let reply = Message::from_server(&server_name, "NOTICE",
                            vec![&nick, &format!("✅ Spawned {child_nick} in {channel} (caps: {caps_str}{ttl_str})")]);
                        send(&state, &session_id, format!("{reply}\r\n"));
                        tracing::info!(parent = %nick, child = %child_nick, channel = %channel, "AGENT SPAWN");
                    }

                    // AGENT DESPAWN <nick>
                    "DESPAWN" => {
                        let child_nick = match msg.params.get(1) {
                            Some(n) => n.clone(),
                            None => {
                                let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                    vec![&nick, "AGENT", "Usage: AGENT DESPAWN <nick>"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };

                        // Find the spawned agent owned by this session
                        let removed = {
                            let mut spawned = state.spawned_agents.lock();
                            let found = spawned.iter()
                                .find(|(_, sa)| sa.nick.eq_ignore_ascii_case(&child_nick) && sa.parent_session == session_id)
                                .map(|(k, v)| (k.clone(), v.clone()));
                            if let Some((key, _)) = &found {
                                spawned.remove(key);
                            }
                            found
                        };

                        if let Some((child_did, sa)) = removed {
                            let quit_line = format!(
                                ":{child_nick}!spawn@freeq/spawn QUIT :Despawned by {nick}\r\n"
                            );
                            helpers::broadcast_to_channel(&state, &sa.channel, &quit_line);
                            state.with_db(|db| db.record_despawn(&child_did));

                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![&nick, &format!("✅ Despawned {child_nick}")]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                            tracing::info!(parent = %nick, child = %child_nick, "AGENT DESPAWN");
                        } else {
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![&nick, &format!("No spawned agent '{child_nick}' owned by you")]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }

                    // AGENT MSG <child-nick> #channel :text — send message as spawned child
                    "MSG" => {
                        let (child_nick, target, text) = match (msg.params.get(1), msg.params.get(2), msg.params.get(3)) {
                            (Some(cn), Some(t), Some(txt)) => (cn.clone(), t.clone(), txt.clone()),
                            _ => {
                                let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                                    vec![&nick, "AGENT", "Usage: AGENT MSG <child-nick> #channel :text"]);
                                send(&state, &session_id, format!("{reply}\r\n"));
                                continue;
                            }
                        };

                        // Verify child is owned by this session
                        let child_exists = state.spawned_agents.lock().values()
                            .any(|sa| sa.nick.eq_ignore_ascii_case(&child_nick) && sa.parent_session == session_id);

                        if child_exists {
                            let child_hostmask = format!("{child_nick}!spawn@freeq/spawn");
                            let msgid = crate::msgid::generate();
                            let line = format!(
                                "@msgid={msgid};+freeq.at/actor-class=agent;+freeq.at/parent={nick} :{child_hostmask} PRIVMSG {target} :{text}\r\n"
                            );
                            helpers::broadcast_to_channel(&state, &target, &line);
                        } else {
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![&nick, &format!("No spawned agent '{child_nick}' owned by you")]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }

                    _ => {
                        let reply = Message::from_server(
                            &server_name,
                            "NOTICE",
                            vec![&nick, &format!("Unknown AGENT subcommand: {subcmd}. Use: REGISTER, PAUSE, RESUME, REVOKE, APPROVE, DENY, MANIFEST, SPAWN, DESPAWN, MSG")],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                    }
                }
            }

            // APPROVAL_REQUEST — agent requests approval for a capability.
            // Usage: APPROVAL_REQUEST #channel :capability;resource=description
            "APPROVAL_REQUEST" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                let channel = match msg.params.first() {
                    Some(c) if c.starts_with('#') => c.clone(),
                    _ => {
                        let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                            vec![&nick, "APPROVAL_REQUEST", "Usage: APPROVAL_REQUEST #channel :capability;resource=desc"]);
                        send(&state, &session_id, format!("{reply}\r\n"));
                        continue;
                    }
                };
                let raw_params = msg.params.get(1).cloned().unwrap_or_default();
                let mut capability = raw_params.as_str();
                let mut resource: Option<String> = None;
                if let Some((cap, rest)) = raw_params.split_once(';') {
                    capability = cap;
                    for part in rest.split(';') {
                        if let Some((k, v)) = part.split_once('=') {
                            if k.trim() == "resource" { resource = Some(v.trim().to_string()); }
                        }
                    }
                }

                if let Some(ref did) = conn.authenticated_did {
                    let approval_id = crate::msgid::generate();
                    state.with_db(|db| db.create_approval(&approval_id, &channel, did, capability, resource.as_deref()));

                    // Notify channel ops
                    let resource_str = resource.as_deref().map(|r| format!(" on {r}")).unwrap_or_default();
                    let notice = format!(
                        ":{server_name} NOTICE {channel} :🔔 {nick} requests approval for '{capability}'{resource_str}. Use: AGENT APPROVE {nick} {capability}\r\n"
                    );
                    helpers::broadcast_to_channel(&state, &channel, &notice);

                    // Send structured TAGMSG for rich clients
                    let tagmsg = format!(
                        "@+freeq.at/event=approval_request;+freeq.at/approval-id={approval_id};+freeq.at/capability={capability} :{} TAGMSG {channel}\r\n",
                        conn.hostmask()
                    );
                    helpers::broadcast_to_channel(&state, &channel, &tagmsg);

                    let reply = Message::from_server(&server_name, "NOTICE",
                        vec![&nick, &format!("Approval requested: {capability} in {channel} (id: {approval_id})")]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    tracing::info!(nick = %nick, channel = %channel, capability = %capability, "Approval requested");
                } else {
                    let reply = Message::from_server(&server_name, "NOTICE",
                        vec![&nick, "Must be authenticated to request approval"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                }
            }

            // SPEND command — agent reports spend for budget tracking.
            // Usage: SPEND #channel :amount=0.03;unit=usd;desc=claude-sonnet-4-20250514: 1.2k tokens;task=01JQXYZ
            "SPEND" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                let channel = match msg.params.first() {
                    Some(c) if c.starts_with('#') => c.clone(),
                    _ => {
                        let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                            vec![&nick, "SPEND", "Usage: SPEND #channel :amount=0.03;unit=usd;desc=...;task=..."]);
                        send(&state, &session_id, format!("{reply}\r\n"));
                        continue;
                    }
                };
                let raw_params = msg.params.get(1).cloned().unwrap_or_default();
                let mut amount: f64 = 0.0;
                let mut unit = "usd".to_string();
                let mut description: Option<String> = None;
                let mut task_ref: Option<String> = None;
                for part in raw_params.split(';') {
                    if let Some((k, v)) = part.split_once('=') {
                        match k.trim() {
                            "amount" => amount = v.trim().parse().unwrap_or(0.0),
                            "unit" => unit = v.trim().to_string(),
                            "desc" | "description" => description = Some(v.trim().to_string()),
                            "task" => task_ref = Some(v.trim().to_string()),
                            _ => {}
                        }
                    }
                }

                if amount <= 0.0 {
                    let reply = Message::from_server(&server_name, "NOTICE",
                        vec![&nick, "SPEND requires positive amount"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }

                if let Some(ref did) = conn.authenticated_did {
                    // Record spend
                    state.with_db(|db| db.record_spend(&channel, did, amount, &unit, description.as_deref(), task_ref.as_deref()));

                    // Check budget and enforce
                    let budget_json = state.with_db(|db| Ok(db.get_budget(&channel, Some(did)))).flatten();
                    if let Some(ref bj) = budget_json {
                        if let Ok(budget) = serde_json::from_str::<crate::policy::types::BudgetPolicy>(bj) {
                            let period_start = budget_period_start(&budget.period);
                            let total_spent = state.with_db(|db| Ok(db.sum_spend(&channel, Some(did), &budget.unit, period_start)))
                                .unwrap_or(0.0);
                            let ratio = total_spent / budget.max_amount;
                            let prev_ratio = (total_spent - amount) / budget.max_amount;

                            // Warn at threshold (first crossing)
                            if ratio >= budget.warn_threshold && prev_ratio < budget.warn_threshold {
                                let warn = format!(
                                    ":{server_name} NOTICE {channel} :⚠ Budget {:.0}% used by {nick} ({:.2}/{:.2} {unit})\r\n",
                                    ratio * 100.0, total_spent, budget.max_amount
                                );
                                helpers::broadcast_to_channel(&state, &channel, &warn);
                                tracing::info!(channel = %channel, agent = %nick, pct = ratio * 100.0, "Budget warning threshold hit");
                            }

                            // Block at limit
                            if ratio >= 1.0 && budget.hard_limit {
                                let block = format!(
                                    ":{server_name} NOTICE {channel} :🛑 {nick} blocked: budget exceeded ({:.2}/{:.2} {unit})\r\n",
                                    total_spent, budget.max_amount
                                );
                                helpers::broadcast_to_channel(&state, &channel, &block);

                                // Send governance signal to agent
                                let gov_line = format!(
                                    "@+freeq.at/governance=budget_exceeded;+freeq.at/spent={:.2};+freeq.at/limit={:.2};+freeq.at/unit={unit} :{server_name} TAGMSG {nick}\r\n",
                                    total_spent, budget.max_amount
                                );
                                send(&state, &session_id, gov_line);
                            }
                        }
                    }

                    let reply = Message::from_server(&server_name, "NOTICE",
                        vec![&nick, &format!("💰 Recorded: {amount:.4} {unit}")]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                } else {
                    let reply = Message::from_server(&server_name, "NOTICE",
                        vec![&nick, "Must be authenticated to report spend"]);
                    send(&state, &session_id, format!("{reply}\r\n"));
                }
            }

            // BUDGET command — set or query channel budget.
            // Usage: BUDGET #channel — query
            // Usage: BUDGET #channel :max=50;unit=usd;period=per_day;sponsor=did:plc:xxx;warn=0.8;hard=true
            "BUDGET" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                let channel = match msg.params.first() {
                    Some(c) if c.starts_with('#') => c.clone(),
                    _ => {
                        let reply = Message::from_server(&server_name, irc::ERR_NEEDMOREPARAMS,
                            vec![&nick, "BUDGET", "Usage: BUDGET #channel [:max=50;unit=usd;period=per_day;...]"]);
                        send(&state, &session_id, format!("{reply}\r\n"));
                        continue;
                    }
                };

                if let Some(raw) = msg.params.get(1) {
                    // Set budget — require op or oper
                    let is_op = {
                        let channels = state.channels.lock();
                        channels.get(&channel.to_lowercase())
                            .map(|ch| ch.ops.contains(&session_id))
                            .unwrap_or(false)
                    };
                    if !is_op && !conn.is_oper {
                        let reply = Message::from_server(&server_name, "482",
                            vec![&nick, &channel, "You must be a channel operator to set budgets"]);
                        send(&state, &session_id, format!("{reply}\r\n"));
                        continue;
                    }

                    let mut max_amount: f64 = 50.0;
                    let mut unit_str = "usd".to_string();
                    let mut period_str = "per_day".to_string();
                    let mut sponsor = conn.authenticated_did.clone().unwrap_or_else(|| nick.clone());
                    let mut warn: f64 = 0.8;
                    let mut hard = true;
                    for part in raw.split(';') {
                        if let Some((k, v)) = part.split_once('=') {
                            match k.trim() {
                                "max" | "max_amount" => max_amount = v.trim().parse().unwrap_or(50.0),
                                "unit" => unit_str = v.trim().to_string(),
                                "period" => period_str = v.trim().to_string(),
                                "sponsor" => sponsor = v.trim().to_string(),
                                "warn" | "warn_threshold" => warn = v.trim().parse().unwrap_or(0.8),
                                "hard" | "hard_limit" => hard = v.trim() == "true",
                                _ => {}
                            }
                        }
                    }

                    let budget = crate::policy::types::BudgetPolicy {
                        unit: unit_str.clone(),
                        max_amount,
                        period: match period_str.as_str() {
                            "per_hour" => crate::policy::types::BudgetPeriod::PerHour,
                            "per_week" => crate::policy::types::BudgetPeriod::PerWeek,
                            "per_task" => crate::policy::types::BudgetPeriod::PerTask,
                            _ => crate::policy::types::BudgetPeriod::PerDay,
                        },
                        sponsor_did: sponsor,
                        warn_threshold: warn,
                        hard_limit: hard,
                        approval_threshold: None,
                    };
                    let budget_json = serde_json::to_string(&budget).unwrap_or_default();
                    let issuer = conn.authenticated_did.as_deref().unwrap_or(&nick);
                    state.with_db(|db| db.set_budget(&channel, None, &budget_json, issuer));

                    let reply = Message::from_server(&server_name, "NOTICE",
                        vec![&nick, &format!("💰 Budget set for {channel}: {max_amount:.2} {unit_str}/{period_str} (warn: {:.0}%, hard: {hard})", warn * 100.0)]);
                    send(&state, &session_id, format!("{reply}\r\n"));

                    // Broadcast to channel
                    let notice = format!(
                        ":{server_name} NOTICE {channel} :💰 Budget set: {max_amount:.2} {unit_str}/{period_str} by {nick}\r\n"
                    );
                    helpers::broadcast_to_channel(&state, &channel, &notice);
                    tracing::info!(channel = %channel, by = %nick, max = max_amount, unit = %unit_str, "BUDGET set");
                } else {
                    // Query budget
                    let budget_json = state.with_db(|db| Ok(db.get_budget(&channel, None))).flatten();
                    if let Some(ref bj) = budget_json {
                        if let Ok(budget) = serde_json::from_str::<crate::policy::types::BudgetPolicy>(bj) {
                            let period_start = budget_period_start(&budget.period);
                            let total_spent = state.with_db(|db| Ok(db.sum_spend(&channel, None, &budget.unit, period_start)))
                                .unwrap_or(0.0);
                            let remaining = budget.max_amount - total_spent;
                            let pct = (total_spent / budget.max_amount * 100.0).min(100.0);
                            let reply = Message::from_server(&server_name, "NOTICE",
                                vec![&nick, &format!("💰 {channel}: {total_spent:.2}/{:.2} {} ({pct:.0}% used, {remaining:.2} remaining)", budget.max_amount, budget.unit)]);
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    } else {
                        let reply = Message::from_server(&server_name, "NOTICE",
                            vec![&nick, &format!("No budget set for {channel}")]);
                        send(&state, &session_id, format!("{reply}\r\n"));
                    }
                }
            }

            // PROVENANCE command — submit a provenance declaration for this agent.
            // Usage: PROVENANCE :<base64url-encoded JSON>
            "PROVENANCE" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                if msg.params.is_empty() {
                    let reply = Message::from_server(
                        &server_name, irc::ERR_NEEDMOREPARAMS,
                        vec![&nick, "PROVENANCE", "Not enough parameters"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                let encoded = &msg.params[0];
                // Try base64url decode, or accept raw JSON
                let json_result = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(encoded)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .or_else(|| serde_json::from_str::<serde_json::Value>(encoded).ok());

                match json_result {
                    Some(provenance) => {
                        if let Some(ref did) = conn.authenticated_did {
                            state.provenance_declarations.lock().insert(did.clone(), provenance);
                            let reply = Message::from_server(
                                &server_name, "NOTICE",
                                vec![&nick, "Provenance declaration stored"],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                            tracing::info!(nick = %nick, did = %did, "Provenance declaration stored");
                        } else {
                            let reply = Message::from_server(
                                &server_name, "NOTICE",
                                vec![&nick, "Must be authenticated to submit provenance"],
                            );
                            send(&state, &session_id, format!("{reply}\r\n"));
                        }
                    }
                    None => {
                        let reply = Message::from_server(
                            &server_name, "NOTICE",
                            vec![&nick, "Invalid provenance format (expected base64url-encoded JSON or raw JSON)"],
                        );
                        send(&state, &session_id, format!("{reply}\r\n"));
                    }
                }
            }

            // PRESENCE command — update structured agent presence.
            // Usage: PRESENCE :state=executing;status=building project;task=01JQXYZ
            "PRESENCE" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                if msg.params.is_empty() {
                    let reply = Message::from_server(
                        &server_name, irc::ERR_NEEDMOREPARAMS,
                        vec![&nick, "PRESENCE", "Not enough parameters"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                // Parse key=value pairs separated by semicolons
                let raw = &msg.params[0];
                let mut presence_state: Option<PresenceState> = None;
                let mut status_text: Option<String> = None;
                let mut task_ref: Option<String> = None;

                for part in raw.split(';') {
                    if let Some((k, v)) = part.split_once('=') {
                        match k.trim() {
                            "state" => presence_state = v.trim().parse().ok(),
                            "status" => status_text = Some(v.trim().to_string()),
                            "task" => task_ref = Some(v.trim().to_string()),
                            _ => {}
                        }
                    }
                }

                let ps = presence_state.unwrap_or(PresenceState::Online);
                let presence = AgentPresence {
                    state: ps,
                    status: status_text.clone(),
                    task: task_ref,
                    updated_at: chrono::Utc::now().timestamp(),
                };

                state.agent_presence.lock().insert(session_id.clone(), presence.clone());

                // Broadcast via AWAY mechanism for backwards compat
                let away_json = serde_json::to_string(&presence).unwrap_or_default();
                let hostmask = conn.hostmask();

                // Set/clear AWAY state
                if ps == PresenceState::Online || ps == PresenceState::Active || ps == PresenceState::Idle {
                    state.session_away.lock().remove(&session_id);
                } else {
                    let away_text = status_text.as_deref().unwrap_or(&ps.to_string()).to_string();
                    state.session_away.lock().insert(session_id.clone(), away_text);
                }

                // Broadcast to away-notify subscribers in shared channels
                {
                    // Collect targets first, then send (avoid holding multiple locks)
                    let targets: Vec<String> = {
                        let channels = state.channels.lock();
                        let away_caps = state.cap_away_notify.lock();
                        let mut sids = Vec::new();
                        for ch in channels.values() {
                            if ch.members.contains(&session_id) {
                                for member_sid in &ch.members {
                                    if member_sid != &session_id && away_caps.contains(member_sid) {
                                        sids.push(member_sid.clone());
                                    }
                                }
                            }
                        }
                        sids
                    };
                    let conns = state.connections.lock();
                    // For active/online/idle: send AWAY with no parameter (= back from away)
                    // For other states: send AWAY with the JSON payload
                    let is_clear = ps == PresenceState::Online || ps == PresenceState::Active || ps == PresenceState::Idle;
                    let line = if is_clear {
                        format!(":{hostmask} AWAY\r\n")
                    } else {
                        format!(":{hostmask} AWAY :{away_json}\r\n")
                    };
                    for sid in &targets {
                        if let Some(tx) = conns.get(sid) {
                            let _ = tx.try_send(line.clone());
                        }
                    }
                }

                let reply = Message::from_server(
                    &server_name, "NOTICE",
                    vec![&nick, &format!("Presence updated: {ps}")],
                );
                send(&state, &session_id, format!("{reply}\r\n"));
                tracing::debug!(nick = %nick, state = %ps, "PRESENCE updated");
            }

            // HEARTBEAT command — agent liveness signal.
            // Usage: HEARTBEAT :state=active;ttl=60
            "HEARTBEAT" => {
                if !conn.registered { continue; }
                let raw = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                let mut hb_state = PresenceState::Active;
                let mut ttl: u64 = 60;

                for part in raw.split(';') {
                    if let Some((k, v)) = part.split_once('=') {
                        match k.trim() {
                            "state" => { if let Ok(s) = v.trim().parse() { hb_state = s; } }
                            "ttl" => { if let Ok(t) = v.trim().parse() { ttl = t; } }
                            _ => {}
                        }
                    }
                }

                let now = chrono::Utc::now().timestamp();
                state.agent_heartbeats.lock().insert(session_id.clone(), (now, ttl));

                // Update presence from heartbeat
                let presence = AgentPresence {
                    state: hb_state,
                    status: None,
                    task: None,
                    updated_at: now,
                };
                state.agent_presence.lock().insert(session_id.clone(), presence);
            }

            // Phase 4: Revoke a peer's S2S access (oper-only).
            // Usage: REVOKEPEER <endpoint_id>
            "REVOKEPEER" => {
                if !conn.registered { continue; }
                let nick = conn.nick_or_star().to_string();
                if !conn.is_oper {
                    let reply = Message::from_server(
                        &server_name, "481",
                        vec![&nick, "Permission Denied - You're not an IRC operator"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                if msg.params.is_empty() {
                    let reply = Message::from_server(
                        &server_name, irc::ERR_NEEDMOREPARAMS,
                        vec![&nick, "REVOKEPEER", "Not enough parameters"],
                    );
                    send(&state, &session_id, format!("{reply}\r\n"));
                    continue;
                }
                let target_peer = &msg.params[0];
                let manager = state.s2s_manager.lock().clone();
                if let Some(manager) = manager {
                    // Disconnect the peer
                    let removed = manager.peers.lock().await.remove(target_peer);
                    if removed.is_some() {
                        manager.peer_names.lock().await.remove(target_peer);
                        manager.authenticated_peers.lock().await.remove(target_peer);
                        manager.dedup.remove_peer(target_peer).await;
                        let notice = format!(":{} NOTICE {} :S2S peer {} revoked and disconnected\r\n",
                            server_name, nick, target_peer);
                        send(&state, &session_id, notice);
                        tracing::warn!(
                            oper = %nick,
                            peer = %target_peer,
                            "S2S peer revoked via REVOKEPEER"
                        );
                    } else {
                        let notice = format!(":{} NOTICE {} :S2S peer {} not found in active connections\r\n",
                            server_name, nick, target_peer);
                        send(&state, &session_id, notice);
                    }
                } else {
                    let notice = format!(":{} NOTICE {} :S2S not active\r\n",
                        server_name, nick);
                    send(&state, &session_id, notice);
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
            if sessions.is_empty() {
                ds.remove(d);
            }
            remaining == 0
        } else {
            true
        }
    } else {
        true // Guest sessions are always "last"
    };

    // Grace period for DID users: hold channel membership for 30s before broadcasting QUIT.
    // If they reconnect within that window, suppress the quit/join churn entirely.
    const QUIT_GRACE_SECS: u64 = 30;

    if let Some(ref nick) = conn.nick {
        if is_last_session_for_did {
            if let Some(ref did) = conn.authenticated_did {
                // DID user — enter ghost mode instead of immediate QUIT
                let hostmask = conn.hostmask();

                // Collect channel membership to preserve
                let ghost_channels: Vec<(String, bool, bool, bool)> = {
                    let channels = state.channels.lock();
                    channels
                        .iter()
                        .filter(|(_, ch)| ch.members.contains(&session_id))
                        .map(|(name, ch)| {
                            (
                                name.clone(),
                                ch.ops.contains(&session_id),
                                ch.voiced.contains(&session_id),
                                ch.halfops.contains(&session_id),
                            )
                        })
                        .collect()
                };

                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

                // Remove from channels/state now, but hold nick and don't broadcast
                cleanup_session_state(&state, &session_id);

                // Don't remove the nick — ghost holds it
                // state.nick_to_session.lock().remove_by_nick(nick); // <-- NOT here

                let ghost = crate::server::GhostSession {
                    nick: nick.clone(),
                    hostmask: hostmask.clone(),
                    channels: ghost_channels,
                    disconnect_time: std::time::Instant::now(),
                    cancel: cancel_tx,
                };
                state.ghost_sessions.lock().insert(did.clone(), ghost);

                tracing::info!(
                    %session_id, nick = %nick, did = %did,
                    "Entered ghost mode ({}s grace period)", QUIT_GRACE_SECS
                );

                // Spawn deferred QUIT broadcast
                let state_clone = state.clone();
                let did_clone = did.clone();
                let nick_clone = nick.clone();
                let hostmask_clone = hostmask.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(QUIT_GRACE_SECS)) => {
                            // Grace period expired — broadcast QUIT now
                            let ghost = state_clone.ghost_sessions.lock().remove(&did_clone);
                            if ghost.is_some() {
                                let quit_msg = format!(":{hostmask_clone} QUIT :Connection closed\r\n");
                                let channels = state_clone.channels.lock();
                                let conns = state_clone.connections.lock();
                                for ch in channels.values() {
                                    for member in &ch.members {
                                        if let Some(tx) = conns.get(member) {
                                            let _ = tx.try_send(quit_msg.clone());
                                        }
                                    }
                                }
                                drop(conns);
                                drop(channels);
                                state_clone.nick_to_session.lock().remove_by_nick(&nick_clone);
                                tracing::info!(
                                    nick = %nick_clone, did = %did_clone,
                                    "Ghost grace expired — broadcasting QUIT"
                                );
                                // S2S
                                let origin = state_clone.server_iroh_id.lock().clone().unwrap_or_default();
                                s2s_broadcast(&state_clone, crate::s2s::S2sMessage::Quit {
                                    event_id: s2s_next_event_id(&state_clone),
                                    nick: nick_clone,
                                    reason: "Connection closed".to_string(),
                                    origin,
                                });
                            }
                        }
                        _ = cancel_rx => {
                            // Reconnected — ghost was reclaimed, no QUIT needed
                        }
                    }
                });
            } else {
                // Guest user — immediate QUIT (no grace period)
                let hostmask = conn.hostmask();
                broadcast_quit(&state, &session_id, &hostmask);
                state.nick_to_session.lock().remove_by_nick(nick);
                broadcast_quit_s2s(&state, nick);
                cleanup_session_state(&state, &session_id);
                cleanup_channel_membership(&state, &session_id);
            }
        } else {
            tracing::info!(
                %session_id,
                nick = %nick,
                "Session closed but other sessions remain for DID"
            );
            cleanup_session_state(&state, &session_id);
            cleanup_channel_membership(&state, &session_id);
        }
    } else {
        cleanup_session_state(&state, &session_id);
        cleanup_channel_membership(&state, &session_id);
    }

    tracing::info!(
        %session_id,
        nick = conn.nick.as_deref().unwrap_or("-"),
        did = conn.authenticated_did.as_deref().unwrap_or("-"),
        last_session = is_last_session_for_did,
        "Connection closed"
    );

    write_handle.abort();
    Ok(())
}

/// Detect the client software from the USER realname field.
fn detect_client(realname: &str) -> String {
    let r = realname.to_lowercase();
    if r.contains("freeq web") {
        return "freeq-web".to_string();
    }
    if r.contains("freeq ios") {
        return "freeq-ios".to_string();
    }
    if r.contains("freeq android") {
        return "freeq-android".to_string();
    }
    if r == "freeq" {
        return "freeq-sdk".to_string();
    }
    // Common IRC clients
    if r.contains("irssi") {
        return "irssi".to_string();
    }
    if r.contains("weechat") {
        return "weechat".to_string();
    }
    if r.contains("hexchat") {
        return "hexchat".to_string();
    }
    if r.contains("thunderbird") {
        return "thunderbird".to_string();
    }
    if r.contains("textual") {
        return "textual".to_string();
    }
    if r.contains("mutter") {
        return "mutter".to_string();
    }
    if r.contains("irccloud") {
        return "irccloud".to_string();
    }
    if r.contains("znc") {
        return "znc".to_string();
    }
    if r.contains("kiwi") {
        return "kiwi-irc".to_string();
    }
    if r.contains("thelounge") {
        return "thelounge".to_string();
    }
    if r.contains("revolution") {
        return "revolution-irc".to_string();
    }
    if r.contains("goguma") {
        return "goguma".to_string();
    }
    // fallback: first word of realname, capped
    let first = realname.split_whitespace().next().unwrap_or("unknown");
    first.chars().take(20).collect()
}

/// Broadcast QUIT to all channels the session is in.
fn broadcast_quit(state: &Arc<SharedState>, session_id: &str, hostmask: &str) {
    let quit_msg = format!(":{hostmask} QUIT :Connection closed\r\n");
    let channels = state.channels.lock();
    let conns = state.connections.lock();
    for ch in channels.values() {
        if ch.members.contains(session_id) {
            for member in &ch.members {
                if member != session_id
                    && let Some(tx) = conns.get(member)
                {
                    let _ = tx.try_send(quit_msg.clone());
                }
            }
        }
    }
}

/// Broadcast QUIT to S2S peers.
fn broadcast_quit_s2s(state: &Arc<SharedState>, nick: &str) {
    let origin = state.server_iroh_id.lock().clone().unwrap_or_default();
    s2s_broadcast(
        state,
        crate::s2s::S2sMessage::Quit {
            event_id: s2s_next_event_id(state),
            nick: nick.to_string(),
            reason: "Connection closed".to_string(),
            origin,
        },
    );
}

/// Clean up per-session state (connections, caps, etc.) but NOT channel membership.
fn cleanup_session_state(state: &Arc<SharedState>, session_id: &str) {
    state.connections.lock().remove(session_id);
    state.session_dids.lock().remove(session_id);
    state.session_handles.lock().remove(session_id);
    state.session_iroh_ids.lock().remove(session_id);
    state.session_away.lock().remove(session_id);
    state.msg_timestamps.lock().remove(session_id);
    state.session_msg_keys.lock().remove(session_id);
    state.session_client_info.lock().remove(session_id);
    state.cap_message_tags.lock().remove(session_id);
    state.cap_multi_prefix.lock().remove(session_id);
    state.cap_echo_message.lock().remove(session_id);
    state.cap_server_time.lock().remove(session_id);
    state.cap_batch.lock().remove(session_id);
    state.cap_account_notify.lock().remove(session_id);
    state.cap_extended_join.lock().remove(session_id);
    state.cap_away_notify.lock().remove(session_id);
    state.server_opers.lock().remove(session_id);
    state.session_actor_class.lock().remove(session_id);
    state.agent_presence.lock().remove(session_id);
    state.agent_heartbeats.lock().remove(session_id);

    // Clean up any spawned (virtual) child agents owned by this session
    let mut spawned = state.spawned_agents.lock();
    let children: Vec<crate::server::SpawnedAgent> = spawned
        .values()
        .filter(|sa| sa.parent_session == session_id)
        .cloned()
        .collect();
    for child in &children {
        spawned.remove(&child.child_did);
    }
    drop(spawned);

    // Broadcast QUIT for each orphaned child
    for child in children {
        let quit_line = format!(
            ":{}!spawn@freeq/spawn QUIT :Parent disconnected\r\n",
            child.nick
        );
        helpers::broadcast_to_channel(state, &child.channel, &quit_line);
        state.with_db(|db| db.record_despawn(&child.child_did));
        tracing::info!(child = %child.nick, parent_session = %session_id, "Despawned orphaned child agent");
    }
}

/// Remove a session from all channels. Retains channels that still have content.
fn cleanup_channel_membership(state: &Arc<SharedState>, session_id: &str) {
    let mut channels = state.channels.lock();
    for ch in channels.values_mut() {
        ch.members.remove(session_id);
        ch.ops.remove(session_id);
        ch.voiced.remove(session_id);
        ch.halfops.remove(session_id);
    }
    channels.retain(|_, ch| {
        !ch.members.is_empty()
            || !ch.remote_members.is_empty()
            || ch.founder_did.is_some()
            || ch.topic.is_some()
            || !ch.bans.is_empty()
    });
}

/// Compute the start timestamp for a budget period.
pub fn budget_period_start(period: &crate::policy::types::BudgetPeriod) -> i64 {
    use crate::policy::types::BudgetPeriod;
    let now = chrono::Utc::now();
    match period {
        BudgetPeriod::PerHour => {
            now.date_naive()
                .and_hms_opt(now.time().hour(), 0, 0)
                .map(|dt| dt.and_utc().timestamp())
                .unwrap_or(0)
        }
        BudgetPeriod::PerDay => {
            now.date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc().timestamp())
                .unwrap_or(0)
        }
        BudgetPeriod::PerWeek => {
            use chrono::Datelike;
            let days_since_monday = now.weekday().num_days_from_monday();
            let monday = now.date_naive() - chrono::Duration::days(days_since_monday as i64);
            monday.and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc().timestamp())
                .unwrap_or(0)
        }
        BudgetPeriod::PerTask => 0, // per-task tracks from task creation, not calendar
    }
}

use chrono::Timelike;
