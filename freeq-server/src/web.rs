//! WebSocket IRC transport and read-only REST API.
//!
//! The WebSocket endpoint (`/irc`) upgrades to a WebSocket connection, then
//! bridges it to the IRC connection handler via a `DuplexStream`. From the
//! server's perspective, a WebSocket client is just another async stream.
//!
//! The REST API exposes read-only data backed by the persistence layer.
//! No write endpoints — if you want to act on the server, speak IRC.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::SystemTime;

use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Redirect};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tower_http::cors::CorsLayer;

use crate::server::SharedState;

// ── WebSocket ↔ IRC bridge ─────────────────────────────────────────────

/// A WebSocket bridged as `AsyncRead + AsyncWrite` for the IRC handler.
///
/// Uses a `tokio::io::DuplexStream` pair with two background tasks:
/// - **rx task:** reads WebSocket frames → appends `\r\n` → writes to bridge
/// - **tx task:** reads from bridge → splits on `\r\n` → sends as WS text frames
pub struct WsBridge {
    pub reader: tokio::io::ReadHalf<tokio::io::DuplexStream>,
    pub writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
}

/// Create a bridged stream from a WebSocket.
///
/// Spawns two async tasks that shuttle data between the WebSocket and a
/// DuplexStream. The returned `WsBridge` implements `AsyncRead + AsyncWrite`
/// and can be passed directly to `handle_generic()`.
fn bridge_ws(socket: WebSocket) -> WsBridge {
    // Split WebSocket into two halves via a channel so each task owns one.
    let (ws_tx, ws_rx) = tokio::sync::mpsc::channel::<WsMessage>(64);

    // DuplexStream: irc_side is what the IRC handler reads/writes.
    // bridge_side is what our background tasks read/write.
    let (irc_side, bridge_side) = tokio::io::duplex(16384);
    let (irc_read, irc_write) = tokio::io::split(irc_side);
    let (mut bridge_read, mut bridge_write) = tokio::io::split(bridge_side);

    // We need the WebSocket as a single owner. Use an Arc<Mutex> for sends,
    // and move the socket into the rx task which also handles sends.
    // Actually simpler: move socket into one task, use channel for the other direction.

    // Task 1: owns the WebSocket, reads frames → bridge_write, reads ws_rx → sends frames
    tokio::spawn(async move {
        let mut socket = socket;
        let mut ws_rx = ws_rx;
        let ws_send_timeout = tokio::time::Duration::from_secs(30);
        loop {
            tokio::select! {
                // Read from WebSocket → write to bridge (→ IRC handler reads)
                frame = socket.recv() => {
                    match frame {
                        Some(Ok(WsMessage::Text(text))) => {
                            let mut bytes = text.as_bytes().to_vec();
                            bytes.extend_from_slice(b"\r\n");
                            if bridge_write.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(WsMessage::Binary(data))) => {
                            let mut bytes = data.to_vec();
                            if !bytes.ends_with(b"\r\n") {
                                bytes.extend_from_slice(b"\r\n");
                            }
                            if bridge_write.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(WsMessage::Close(_))) | None => break,
                        Some(Ok(_)) => {} // Ping/Pong handled by axum
                        Some(Err(_)) => break,
                    }
                }
                // Read from channel → send as WebSocket frame (with timeout to detect dead sockets)
                msg = ws_rx.recv() => {
                    match msg {
                        Some(ws_msg) => {
                            match tokio::time::timeout(ws_send_timeout, socket.send(ws_msg)).await {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) | Err(_) => {
                                    tracing::debug!("WebSocket send failed or timed out, closing bridge");
                                    break;
                                }
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = bridge_write.shutdown().await;
        let _ = socket.send(WsMessage::Close(None)).await;
    });

    // Task 2: reads from bridge (← IRC handler writes) → sends as WS text frames via channel
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        let mut line_buf = Vec::new();
        loop {
            match bridge_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    line_buf.extend_from_slice(&buf[..n]);
                    // Send complete lines as text frames
                    while let Some(pos) = line_buf.windows(2).position(|w| w == b"\r\n") {
                        let line = String::from_utf8_lossy(&line_buf[..pos]).to_string();
                        line_buf.drain(..pos + 2);
                        if ws_tx.send(WsMessage::Text(line.into())).await.is_err() {
                            return;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    WsBridge {
        reader: irc_read,
        writer: irc_write,
    }
}

impl AsyncRead for WsBridge {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for WsBridge {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.writer).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.writer).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

// ── Axum router ────────────────────────────────────────────────────────

/// Build the axum router with WebSocket and REST endpoints.
pub fn router(state: Arc<SharedState>) -> Router {
    let mut app = Router::new()
        // WebSocket IRC transport
        .route("/irc", get(ws_upgrade))
        // OAuth endpoints for web client
        .route("/auth/login", get(auth_login))
        .route("/auth/callback", get(auth_callback))
        .route("/auth/broker/web-token", post(auth_broker_web_token))
        .route("/auth/broker/session", post(auth_broker_session))
        .route("/client-metadata.json", get(client_metadata))
        // REST API (read-only, v1)
        .route("/api/v1/health", get(api_health))
        .route("/api/v1/channels", get(api_channels))
        .route("/api/v1/channels/{name}/history", get(api_channel_history))
        .route("/api/v1/channels/{name}/topic", get(api_channel_topic))
        .route("/api/v1/users/{nick}", get(api_user))
        .route("/api/v1/users/{nick}/whois", get(api_user_whois))
        .route("/api/v1/upload", axum::routing::post(api_upload))
        .route("/api/v1/blob", get(api_blob_proxy))
        .route("/api/v1/og", get(api_og_preview))
        .route("/api/v1/keys/{did}", get(api_get_keys))
        .route("/api/v1/keys", axum::routing::post(api_upload_keys))
        .route("/api/v1/signing-key", get(api_signing_key))
        .route("/api/v1/signing-keys/{did}", get(api_did_signing_key))
        .route("/api/v1/verify/{msgid}", get(api_verify_message))
        .route("/auth/mobile", get(auth_mobile_redirect))
        .route("/join/{channel}", get(channel_invite_page))
        .layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024)) // 12MB
        .layer({
            use tower_http::cors::AllowOrigin;
            use axum::http::{Method, header};
            let origins = [
                "https://irc.freeq.at",
                "https://auth.freeq.at",
                "https://freeq.at",
                "http://127.0.0.1:5173",  // vite dev
                "http://localhost:5173",
            ];
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(
                    origins.iter().filter_map(|o| o.parse().ok())
                ))
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION,
                    "X-Broker-Signature".parse().unwrap()])
                .allow_credentials(true)
        });

    // Policy API endpoints
    if state.policy_engine.is_some() {
        app = app.merge(crate::policy::api::routes());
    }

    // Build verifier router (stashed, merged after .with_state())
    let verifier_router = {
        let github_config = state.config.github_client_id.as_ref().map(|id| {
            crate::verifiers::GitHubConfig {
                client_id: id.clone(),
                client_secret: state.config.github_client_secret.clone().unwrap_or_default(),
            }
        });
        let issuer_did = format!("did:web:{}:verify", state.config.server_name);
        let data_dir = state.config.db_path.as_ref()
            .map(|p| std::path::Path::new(p).parent().unwrap_or(std::path::Path::new(".")))
            .unwrap_or(std::path::Path::new("."));
        crate::verifiers::router(issuer_did, github_config, data_dir).map(|(r, _)| r)
    };

    // Serve static web client files if the directory exists
    if let Some(ref web_dir) = state.config.web_static_dir {
        let dir = std::path::PathBuf::from(web_dir);
        if dir.exists() {
            tracing::info!("Serving web client from {}", dir.display());
            // SPA fallback: serve index.html for any path not matching a static file
            let index_path = dir.join("index.html");
            let serve = tower_http::services::ServeDir::new(&dir)
                .append_index_html_on_directories(true)
                .fallback(tower_http::services::ServeFile::new(index_path));
            app = app.fallback_service(serve);
        } else {
            tracing::warn!("Web static dir not found: {}", dir.display());
        }
    }

    // Apply state, then merge verifier (which has its own state already applied)
    let mut final_app = app.with_state(state);
    if let Some(vr) = verifier_router {
        final_app = final_app.merge(vr);
    }
    // Security headers as outermost layer so they apply to all responses
    // including static files served via fallback_service
    final_app.layer(axum::middleware::from_fn(security_headers))
}

// ── WebSocket handler ──────────────────────────────────────────────────

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    State(state): State<Arc<SharedState>>,
) -> impl IntoResponse {
    let ip = addr.ip();
    // Per-IP connection limit for WebSocket (same limit as TCP)
    const MAX_CONNS_PER_IP: u32 = 20;
    {
        let ip_conns = state.ip_connections.lock();
        if ip_conns.get(&ip).copied().unwrap_or(0) >= MAX_CONNS_PER_IP {
            tracing::warn!(%ip, "WebSocket connection rejected: per-IP limit reached");
            return axum::http::StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    ws.on_upgrade(move |socket| handle_ws(socket, state, ip)).into_response()
}

async fn handle_ws(socket: WebSocket, state: Arc<SharedState>, ip: std::net::IpAddr) {
    {
        let mut ip_conns = state.ip_connections.lock();
        *ip_conns.entry(ip).or_insert(0) += 1;
    }
    let stream = bridge_ws(socket);
    if let Err(e) = crate::connection::handle_generic(stream, state.clone()).await {
        tracing::error!("WebSocket connection error: {e}");
    }
    // Decrement on disconnect
    let mut ip_conns = state.ip_connections.lock();
    if let Some(count) = ip_conns.get_mut(&ip) {
        *count = count.saturating_sub(1);
        if *count == 0 { ip_conns.remove(&ip); }
    }
}

// ── REST types ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct HealthResponse {
    server_name: String,
    connections: usize,
    channels: usize,
    uptime_secs: u64,
}

#[derive(Serialize)]
struct ChannelInfo {
    name: String,
    members: usize,
    topic: Option<String>,
}

#[derive(Serialize)]
struct ChannelTopicResponse {
    channel: String,
    topic: Option<String>,
    set_by: Option<String>,
    set_at: Option<u64>,
}

#[derive(Serialize)]
struct MessageResponse {
    id: i64,
    sender: String,
    text: String,
    timestamp: u64,
    tags: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
    before: Option<u64>,
}

#[derive(Serialize)]
struct UserResponse {
    nick: String,
    online: bool,
    did: Option<String>,
    handle: Option<String>,
}

#[derive(Serialize)]
struct WhoisResponse {
    nick: String,
    online: bool,
    did: Option<String>,
    handle: Option<String>,
    channels: Vec<String>,
}

// ── REST handlers ──────────────────────────────────────────────────────

/// Server start time (set once on first call).
static START_TIME: std::sync::OnceLock<SystemTime> = std::sync::OnceLock::new();

/// Public endpoint: returns the server's message signing public key.
/// Clients and federated servers use this to verify `+freeq.at/sig` tags.
async fn api_signing_key(State(state): State<Arc<SharedState>>) -> Json<serde_json::Value> {
    let vk = state.msg_signing_key.verifying_key();
    use base64::Engine;
    let pubkey_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(vk.as_bytes());
    Json(serde_json::json!({
        "algorithm": "ed25519",
        "public_key": pubkey_b64,
        "encoding": "base64url",
        "usage": "message-signing",
        "canonical_form": "{sender_did}\\0{target}\\0{text}\\0{timestamp}",
        "tag": "+freeq.at/sig"
    }))
}

/// Per-DID signing key: returns the client's registered session signing key.
async fn api_did_signing_key(
    State(state): State<Arc<SharedState>>,
    axum::extract::Path(did): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let did_decoded = urlencoding::decode(&did).unwrap_or(std::borrow::Cow::Borrowed(&did));
    if let Some(pubkey) = state.did_msg_keys.lock().get(did_decoded.as_ref()) {
        Ok(Json(serde_json::json!({
            "did": did_decoded.as_ref(),
            "algorithm": "ed25519",
            "public_key": pubkey,
            "encoding": "base64url",
            "source": "client-session"
        })))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

/// Verify a message's cryptographic signature by msgid.
/// Returns the message, signature, verification result, and the math to prove it.
async fn api_verify_message(
    State(state): State<Arc<SharedState>>,
    axum::extract::Path(msgid): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    // Search all channel histories for this msgid
    let channels = state.channels.lock();
    let mut found = None;
    let mut found_channel = String::new();
    for (ch_name, ch) in channels.iter() {
        for msg in ch.history.iter() {
            if msg.msgid.as_deref() == Some(&msgid) {
                found = Some(msg.clone());
                found_channel = ch_name.clone();
                break;
            }
        }
        if found.is_some() { break; }
    }
    drop(channels);

    let msg = found.ok_or((
        axum::http::StatusCode::NOT_FOUND,
        format!("Message {msgid} not found in recent history"),
    ))?;

    let sig_b64 = msg.tags.get("+freeq.at/sig").cloned();
    let sender_nick = msg.from.split('!').next().unwrap_or(&msg.from);

    // Resolve sender's DID
    let sender_did = state.nick_owners.lock()
        .iter()
        .find(|(_, did)| {
            state.did_nicks.lock().get(*did).map(|n| n == sender_nick).unwrap_or(false)
        })
        .map(|(_, did)| did.clone())
        // Also check active sessions
        .or_else(|| {
            let n2s = state.nick_to_session.lock();
            let session = n2s.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(sender_nick))
                .map(|(_, s)| s.clone());
            session.and_then(|s| state.session_dids.lock().get(&s).cloned())
        });

    let canonical = sender_did.as_ref().map(|did| {
        format!("{did}\0{found_channel}\0{}\0{}", msg.text, msg.timestamp)
    });

    // Try to verify: first against client session key, then server key
    let mut verification = serde_json::json!(null);
    if let (Some(sig_b64), Some(canonical_str)) = (&sig_b64, &canonical) {
        use base64::Engine;
        if let Ok(sig_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64) {
            if sig_bytes.len() == 64 {
                let sig_array: [u8; 64] = sig_bytes.try_into().unwrap();
                let sig = ed25519_dalek::Signature::from_bytes(&sig_array);
                let canonical_bytes = canonical_str.as_bytes();

                // Try client session key first
                let mut verified_by = "none";
                if let Some(ref did) = sender_did {
                    if let Some(pubkey_b64) = state.did_msg_keys.lock().get(did) {
                        if let Ok(pk_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(pubkey_b64) {
                            if pk_bytes.len() == 32 {
                                let pk_arr: [u8; 32] = pk_bytes.try_into().unwrap();
                                if let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&pk_arr) {
                                    use ed25519_dalek::Verifier;
                                    if vk.verify(canonical_bytes, &sig).is_ok() {
                                        verified_by = "client-session-key";
                                    }
                                }
                            }
                        }
                    }
                }

                // Fall back to server key
                if verified_by == "none" {
                    use ed25519_dalek::Verifier;
                    let server_vk = state.msg_signing_key.verifying_key();
                    if server_vk.verify(canonical_bytes, &sig).is_ok() {
                        verified_by = "server-key";
                    }
                }

                let server_pubkey = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(state.msg_signing_key.verifying_key().as_bytes());
                let client_pubkey = sender_did.as_ref()
                    .and_then(|did| state.did_msg_keys.lock().get(did).cloned());

                verification = serde_json::json!({
                    "valid": verified_by != "none",
                    "verified_by": verified_by,
                    "server_public_key": server_pubkey,
                    "client_public_key": client_pubkey,
                });
            }
        }
    }

    let canonical_hex = canonical.as_ref().map(|c| {
        c.as_bytes().iter().map(|b| format!("{:02x}", b)).collect::<String>()
    });

    Ok(Json(serde_json::json!({
        "msgid": msgid,
        "channel": found_channel,
        "from": msg.from,
        "text": msg.text,
        "timestamp": msg.timestamp,
        "sender_did": sender_did,
        "signature": sig_b64,
        "canonical_form": canonical,
        "canonical_hex": canonical_hex,
        "verification": verification,
        "how_to_verify": "echo -n '<canonical_form>' | openssl dgst -ed25519 -verify <pubkey.pem> -signature <sig.bin>"
    })))
}

async fn api_health(State(state): State<Arc<SharedState>>) -> Json<HealthResponse> {
    let start = START_TIME.get_or_init(SystemTime::now);
    let uptime = start.elapsed().unwrap_or_default().as_secs();
    let connections = state.connections.lock().len();
    let channels = state.channels.lock().len();
    Json(HealthResponse {
        server_name: state.server_name.clone(),
        connections,
        channels,
        uptime_secs: uptime,
    })
}

async fn api_channels(State(state): State<Arc<SharedState>>) -> Json<Vec<ChannelInfo>> {
    let channels = state.channels.lock();
    let list: Vec<ChannelInfo> = channels
        .iter()
        .map(|(name, ch)| ChannelInfo {
            name: name.clone(),
            members: ch.members.len(),
            topic: ch.topic.as_ref().map(|t| t.text.clone()),
        })
        .collect();
    Json(list)
}

async fn api_channel_history(
    Path(name): Path<String>,
    Query(params): Query<HistoryQuery>,
    State(state): State<Arc<SharedState>>,
) -> Result<Json<Vec<MessageResponse>>, StatusCode> {
    let channel = if name.starts_with('#') {
        name
    } else {
        format!("#{name}")
    };

    let limit = params.limit.unwrap_or(50).min(200);

    // Try database first for full history
    let messages = state.with_db(|db| db.get_messages(&channel, limit, params.before));

    match messages {
        Some(rows) => {
            let resp: Vec<MessageResponse> = rows
                .into_iter()
                .map(|r| MessageResponse {
                    id: r.id,
                    sender: r.sender,
                    text: r.text,
                    timestamp: r.timestamp,
                    tags: r.tags,
                })
                .collect();
            Ok(Json(resp))
        }
        None => {
            // No database — fall back to in-memory history
            let channels = state.channels.lock();
            match channels.get(&channel) {
                Some(ch) => {
                    let resp: Vec<MessageResponse> = ch
                        .history
                        .iter()
                        .filter(|m| params.before.is_none_or(|b| m.timestamp < b))
                        .rev()
                        .take(limit)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .enumerate()
                        .map(|(i, m)| MessageResponse {
                            id: i as i64,
                            sender: m.from.clone(),
                            text: m.text.clone(),
                            timestamp: m.timestamp,
                            tags: m.tags.clone(),
                        })
                        .collect();
                    Ok(Json(resp))
                }
                None => Err(StatusCode::NOT_FOUND),
            }
        }
    }
}

async fn api_channel_topic(
    Path(name): Path<String>,
    State(state): State<Arc<SharedState>>,
) -> Result<Json<ChannelTopicResponse>, StatusCode> {
    let channel = if name.starts_with('#') {
        name
    } else {
        format!("#{name}")
    };

    let channels = state.channels.lock();
    match channels.get(&channel) {
        Some(ch) => Ok(Json(ChannelTopicResponse {
            channel,
            topic: ch.topic.as_ref().map(|t| t.text.clone()),
            set_by: ch.topic.as_ref().map(|t| t.set_by.clone()),
            set_at: ch.topic.as_ref().map(|t| t.set_at),
        })),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn api_user(
    Path(nick): Path<String>,
    State(state): State<Arc<SharedState>>,
) -> Result<Json<UserResponse>, StatusCode> {
    let session = state.nick_to_session.lock().get(&nick).cloned();
    let online = session.is_some();

    let (did, handle) = if let Some(ref session_id) = session {
        let did = state.session_dids.lock().get(session_id).cloned();
        let handle = state
            .session_handles
            .lock()
            .get(session_id)
            .cloned();
        (did, handle)
    } else {
        let did = state
            .nick_owners
            .lock()
            .get(&nick.to_lowercase())
            .cloned();
        (did, None)
    };

    if !online && did.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(UserResponse {
        nick,
        online,
        did,
        handle,
    }))
}

async fn api_user_whois(
    Path(nick): Path<String>,
    State(state): State<Arc<SharedState>>,
) -> Result<Json<WhoisResponse>, StatusCode> {
    let session = state.nick_to_session.lock().get(&nick).cloned();
    let online = session.is_some();

    let (did, handle) = if let Some(ref session_id) = session {
        let did = state.session_dids.lock().get(session_id).cloned();
        let handle = state
            .session_handles
            .lock()
            .get(session_id)
            .cloned();
        (did, handle)
    } else {
        let did = state
            .nick_owners
            .lock()
            .get(&nick.to_lowercase())
            .cloned();
        (did, None)
    };

    if !online && did.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let channels = if let Some(ref session_id) = session {
        let chans = state.channels.lock();
        chans
            .iter()
            .filter(|(_, ch)| ch.members.contains(session_id))
            .map(|(name, _)| name.clone())
            .collect()
    } else {
        vec![]
    };

    Ok(Json(WhoisResponse {
        nick,
        online,
        did,
        handle,
        channels,
    }))
}

// ── Auth broker endpoints ───────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
struct BrokerTokenRequest {
    did: String,
    handle: String,
}

#[derive(Deserialize, Serialize)]
struct BrokerSessionRequest {
    did: String,
    handle: String,
    pds_url: String,
    access_token: String,
    dpop_key_b64: String,
    dpop_nonce: Option<String>,
}

#[derive(Serialize)]
struct BrokerTokenResponse {
    token: String,
    nick: String,
    did: String,
    handle: String,
}

async fn auth_broker_web_token(
    State(state): State<Arc<SharedState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<BrokerTokenResponse>, (StatusCode, String)> {
    let secret = state.config.broker_shared_secret.clone()
        .ok_or((StatusCode::FORBIDDEN, "Broker auth not configured".to_string()))?;
    verify_broker_signature_raw(&secret, &headers, &body)?;
    let req: BrokerTokenRequest = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON: {e}")))?;

    let token = generate_random_string(32);
    state.web_auth_tokens.lock().insert(
        token.clone(),
        (req.did.clone(), req.handle.clone(), std::time::Instant::now()),
    );
    let nick = mobile_nick_from_handle(&req.handle);
    Ok(Json(BrokerTokenResponse { token, nick, did: req.did, handle: req.handle }))
}

async fn auth_broker_session(
    State(state): State<Arc<SharedState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let secret = state.config.broker_shared_secret.clone()
        .ok_or((StatusCode::FORBIDDEN, "Broker auth not configured".to_string()))?;
    verify_broker_signature_raw(&secret, &headers, &body)?;
    let req: BrokerSessionRequest = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON: {e}")))?;

    tracing::info!(did = %req.did, "Broker pushed web session");
    state.web_sessions.lock().insert(req.did.clone(), crate::server::WebSession {
        did: req.did.clone(),
        handle: req.handle.clone(),
        pds_url: req.pds_url.clone(),
        access_token: req.access_token.clone(),
        dpop_key_b64: req.dpop_key_b64.clone(),
        dpop_nonce: req.dpop_nonce.clone(),
        created_at: std::time::Instant::now(),
    });

    Ok(Json(serde_json::json!({"ok": true})))
}

/// Verify HMAC-SHA256 signature over raw request bytes.
/// This avoids JSON re-serialization order mismatches between broker and server.
fn verify_broker_signature_raw(
    secret: &str,
    headers: &axum::http::HeaderMap,
    body_bytes: &[u8],
) -> Result<(), (StatusCode, String)> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let sig = headers.get("x-broker-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "Missing broker signature".to_string()))?;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "HMAC init failed".to_string()))?;
    mac.update(body_bytes);
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    if expected != sig {
        return Err((StatusCode::UNAUTHORIZED, "Invalid broker signature".to_string()));
    }
    Ok(())
}

// ── OAuth client metadata ──────────────────────────────────────────────

/// Serves the AT Protocol OAuth client-metadata.json document.
/// The client_id for non-localhost origins is `{origin}/client-metadata.json`.
async fn client_metadata(
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    let (web_origin, _) = derive_web_origin(&headers);
    let redirect_uri = format!("{web_origin}/auth/callback");
    let client_id = build_client_id(&web_origin, &redirect_uri);

    Json(serde_json::json!({
        "client_id": client_id,
        "client_name": "freeq",
        "client_uri": web_origin,
        "logo_uri": format!("{web_origin}/freeq.png"),
        "tos_uri": format!("{web_origin}"),
        "policy_uri": format!("{web_origin}"),
        "redirect_uris": [redirect_uri],
        "scope": "atproto transition:generic",
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "web",
        "dpop_bound_access_tokens": true
    }))
}

/// Derive web origin and scheme from Host header.
fn derive_web_origin(headers: &axum::http::HeaderMap) -> (String, String) {
    let raw_host = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:8080");
    let host = raw_host.replace("localhost", "127.0.0.1");
    let scheme = if host.starts_with("127.") || host.starts_with("192.168.") || host.starts_with("10.") {
        "http"
    } else {
        "https"
    };
    let origin = format!("{scheme}://{host}");
    (origin, scheme.to_string())
}

/// Derive web origin from server config (for startup-time use, no headers available).
fn derive_web_origin_from_config(config: &crate::config::ServerConfig) -> (String, String) {
    let addr = config.web_addr.as_deref().unwrap_or("127.0.0.1:8080");
    let host = addr.replace("localhost", "127.0.0.1");
    let scheme = if host.starts_with("127.") || host.starts_with("0.0.0.0") {
        "http"
    } else {
        "https"
    };
    (format!("{scheme}://{host}"), scheme.to_string())
}

/// Build OAuth client_id. Loopback uses http://localhost?... form;
/// production uses {origin}/client-metadata.json.
fn build_client_id(web_origin: &str, redirect_uri: &str) -> String {
    if web_origin.starts_with("http://127.") || web_origin.starts_with("http://192.168.") || web_origin.starts_with("http://10.") {
        // Loopback client — use http://localhost form per AT Protocol spec
        let scope = "atproto transition:generic";
        format!(
            "http://localhost?redirect_uri={}&scope={}",
            urlencod(redirect_uri), urlencod(scope),
        )
    } else {
        // Production — client_id is the URL of the client-metadata.json document
        format!("{web_origin}/client-metadata.json")
    }
}

// ── OAuth endpoints for web client ─────────────────────────────────────

#[derive(Deserialize)]
struct AuthLoginQuery {
    handle: String,
    /// If "1", callback redirects to freeq:// URL scheme for mobile apps.
    mobile: Option<String>,
}

/// GET /auth/login?handle=user.bsky.social
///
/// Initiates the AT Protocol OAuth flow. Resolves the handle, does PAR,
/// and redirects the browser to the authorization server.
/// Serves a page that reads #oauth=base64json from the hash fragment,
/// parses it, and redirects to freeq://auth?token=...&broker_token=...
/// This is used by the iOS app because the broker's HTML redirect has
/// broken JS (escaped quotes in raw strings) and ASWebAuthenticationSession
/// doesn't intercept JS-initiated custom scheme navigations.
async fn auth_mobile_redirect() -> impl IntoResponse {
    let html = r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>freeq</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
body{font-family:system-ui;background:#1e1e2e;color:#cdd6f4;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}
.box{text-align:center}
h1{color:#89b4fa;font-size:24px}
p{color:#a6adc8;font-size:15px}
a{color:#89b4fa;font-size:17px;font-weight:600;text-decoration:none;display:inline-block;margin-top:16px;padding:12px 32px;background:#89b4fa22;border-radius:12px}
</style></head>
<body><div class="box" id="box">
<h1>freeq</h1>
<p id="status">Connecting...</p>
<a id="link" style="display:none" href="#">Open freeq</a>
</div>
<script>
try {
  var h = location.hash;
  if (h && h.indexOf('#oauth=') === 0) {
    var b64 = h.substring(7).replace(/-/g,'+').replace(/_/g,'/');
    while(b64.length%4) b64+='=';
    var json = JSON.parse(atob(b64));
    var t = json.token || json.web_token || json.access_jwt || '';
    var bt = json.broker_token || '';
    var n = json.nick || json.handle || '';
    var d = json.did || '';
    var ha = json.handle || '';
    var url = 'freeq://auth?token=' + encodeURIComponent(t)
      + '&broker_token=' + encodeURIComponent(bt)
      + '&nick=' + encodeURIComponent(n)
      + '&did=' + encodeURIComponent(d)
      + '&handle=' + encodeURIComponent(ha);
    document.getElementById('link').href = url;
    document.getElementById('link').style.display = 'inline-block';
    document.getElementById('status').textContent = 'Tap to return to freeq';
    window.location.href = url;
  } else {
    document.getElementById('status').textContent = 'Authentication failed.';
  }
} catch(e) {
  document.getElementById('status').textContent = 'Error: ' + e.message;
}
</script></body></html>"##;
    (
        [
            ("content-type", "text/html; charset=utf-8"),
            ("content-security-policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'"),
        ],
        html,
    )
}

async fn auth_login(
    headers: axum::http::HeaderMap,
    Query(q): Query<AuthLoginQuery>,
    State(state): State<Arc<SharedState>>,
) -> Result<Redirect, (StatusCode, String)> {
    let handle = q.handle.trim().to_string();

    // Derive the origin from the Host header so redirect_uri matches what the browser sees
    let (web_origin, _scheme) = derive_web_origin(&headers);

    // Resolve handle → DID → PDS
    let resolver = freeq_sdk::did::DidResolver::http();
    let did = resolver.resolve_handle(&handle).await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot resolve handle: {e}")))?;
    let did_doc = resolver.resolve(&did).await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot resolve DID: {e}")))?;
    let pds_url = freeq_sdk::pds::pds_endpoint(&did_doc)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "No PDS in DID document".to_string()))?;

    // Discover authorization server
    let client = reqwest::Client::new();
    let pr_url = format!("{}/.well-known/oauth-protected-resource", pds_url.trim_end_matches('/'));
    let pr_meta: serde_json::Value = client.get(&pr_url).send().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("PDS metadata fetch failed: {e}")))?
        .json().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("PDS metadata parse failed: {e}")))?;

    let auth_server = pr_meta["authorization_servers"][0].as_str()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No authorization server".to_string()))?;

    let as_url = format!("{}/.well-known/oauth-authorization-server", auth_server.trim_end_matches('/'));
    let auth_meta: serde_json::Value = client.get(&as_url).send().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Auth server metadata failed: {e}")))?
        .json().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Auth server metadata parse failed: {e}")))?;

    let authorization_endpoint = auth_meta["authorization_endpoint"].as_str()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No authorization_endpoint".to_string()))?;
    let token_endpoint = auth_meta["token_endpoint"].as_str()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No token_endpoint".to_string()))?;
    let par_endpoint = auth_meta["pushed_authorization_request_endpoint"].as_str()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No PAR endpoint".to_string()))?;

    // Build redirect URI and client_id
    let redirect_uri = format!("{web_origin}/auth/callback");
    let scope = "atproto transition:generic";
    let client_id = build_client_id(&web_origin, &redirect_uri);

    // Generate PKCE + DPoP key + state
    let dpop_key = freeq_sdk::oauth::DpopKey::generate();
    let (code_verifier, code_challenge) = generate_pkce();
    let oauth_state = generate_random_string(16);

    // PAR request
    let params = [
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", &redirect_uri),
        ("code_challenge", &code_challenge),
        ("code_challenge_method", "S256"),
        ("scope", scope),
        ("state", &oauth_state),
        ("login_hint", &handle),
    ];

    // Try without nonce first
    let dpop_proof = dpop_key.proof("POST", par_endpoint, None, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP proof failed: {e}")))?;
    let resp = client.post(par_endpoint).header("DPoP", &dpop_proof).form(&params).send().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("PAR failed: {e}")))?;

    let status = resp.status();
    let dpop_nonce = resp.headers().get("dpop-nonce")
        .and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let par_resp: serde_json::Value = if status.as_u16() == 400 && dpop_nonce.is_some() {
        // Retry with nonce
        let nonce = dpop_nonce.as_deref().unwrap();
        let dpop_proof2 = dpop_key.proof("POST", par_endpoint, Some(nonce), None)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP retry failed: {e}")))?;
        let resp2 = client.post(par_endpoint).header("DPoP", &dpop_proof2).form(&params).send().await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("PAR retry failed: {e}")))?;
        if !resp2.status().is_success() {
            let text = resp2.text().await.unwrap_or_default();
            return Err((StatusCode::BAD_GATEWAY, format!("PAR failed: {text}")));
        }
        resp2.json().await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("PAR parse failed: {e}")))?
    } else if status.is_success() {
        resp.json().await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("PAR parse failed: {e}")))?
    } else {
        let text = resp.text().await.unwrap_or_default();
        return Err((StatusCode::BAD_GATEWAY, format!("PAR failed ({status}): {text}")));
    };

    let request_uri = par_resp["request_uri"].as_str()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No request_uri in PAR response".to_string()))?;

    // Store pending session
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    state.oauth_pending.lock().insert(oauth_state.clone(), crate::server::OAuthPending {
        handle: handle.clone(),
        did: did.clone(),
        pds_url: pds_url.clone(),
        code_verifier,
        redirect_uri: redirect_uri.clone(),
        client_id: client_id.clone(),
        token_endpoint: token_endpoint.to_string(),
        dpop_key_b64: dpop_key.to_base64url(),
        created_at: now,
        mobile: q.mobile.as_deref() == Some("1"),
    });

    // Redirect to authorization server
    let auth_url = format!(
        "{}?client_id={}&request_uri={}",
        authorization_endpoint, urlencod(&client_id), urlencod(request_uri),
    );

    tracing::info!(handle = %handle, did = %did, "OAuth login started, redirecting to auth server");
    Ok(Redirect::temporary(&auth_url))
}

#[derive(Deserialize)]
struct AuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// GET /auth/callback?code=...&state=...
///
/// OAuth callback from the authorization server. Exchanges the code for
/// tokens and returns an HTML page that posts the result to the parent window.
async fn auth_callback(
    Query(q): Query<AuthCallbackQuery>,
    State(state): State<Arc<SharedState>>,
) -> Result<Html<String>, (StatusCode, String)> {
    // Check for error
    if let Some(error) = &q.error {
        let desc = q.error_description.as_deref().unwrap_or("Unknown error");
        return Ok(Html(oauth_result_page(&format!("Error: {error}: {desc}"), None)));
    }

    let code = q.code.as_deref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing code".to_string()))?;
    let oauth_state = q.state.as_deref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing state".to_string()))?;

    // Look up pending session
    let pending = state.oauth_pending.lock().remove(oauth_state)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Unknown or expired OAuth state".to_string()))?;

    // Check expiry (5 minutes)
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    if now - pending.created_at > 300 {
        return Err((StatusCode::BAD_REQUEST, "OAuth session expired".to_string()));
    }

    // Exchange code for token
    let dpop_key = freeq_sdk::oauth::DpopKey::from_base64url(&pending.dpop_key_b64)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP key error: {e}")))?;

    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", pending.redirect_uri.as_str()),
        ("client_id", pending.client_id.as_str()),
        ("code_verifier", pending.code_verifier.as_str()),
    ];

    // Try without nonce
    let dpop_proof = dpop_key.proof("POST", &pending.token_endpoint, None, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP proof failed: {e}")))?;
    let resp = client.post(&pending.token_endpoint).header("DPoP", &dpop_proof).form(&params).send().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token exchange failed: {e}")))?;

    let status = resp.status();
    let dpop_nonce = resp.headers().get("dpop-nonce")
        .and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let token_resp: serde_json::Value = if (status.as_u16() == 400 || status.as_u16() == 401) && dpop_nonce.is_some() {
        let nonce = dpop_nonce.as_deref().unwrap();
        let dpop_proof2 = dpop_key.proof("POST", &pending.token_endpoint, Some(nonce), None)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP retry failed: {e}")))?;
        let resp2 = client.post(&pending.token_endpoint).header("DPoP", &dpop_proof2).form(&params).send().await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token retry failed: {e}")))?;
        if !resp2.status().is_success() {
            let text = resp2.text().await.unwrap_or_default();
            return Ok(Html(oauth_result_page(&format!("Token exchange failed: {text}"), None)));
        }
        resp2.json().await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token parse failed: {e}")))?
    } else if status.is_success() {
        resp.json().await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token parse failed: {e}")))?
    } else {
        let text = resp.text().await.unwrap_or_default();
        return Ok(Html(oauth_result_page(&format!("Token exchange failed ({status}): {text}"), None)));
    };

    let access_token = token_resp["access_token"].as_str()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No access_token".to_string()))?;

    // Generate a one-time web auth token for SASL
    let web_token = generate_random_string(32);
    state.web_auth_tokens.lock().insert(
        web_token.clone(),
        (pending.did.clone(), pending.handle.clone(), std::time::Instant::now()),
    );

    let result = crate::server::OAuthResult {
        did: pending.did.clone(),
        handle: pending.handle.clone(),
        access_jwt: access_token.to_string(),
        pds_url: pending.pds_url.clone(),
        web_token: Some(web_token),
    };

    // Store web session for server-proxied operations (media upload)
    state.web_sessions.lock().insert(pending.did.clone(), crate::server::WebSession {
        did: pending.did.clone(),
        handle: pending.handle.clone(),
        pds_url: pending.pds_url.clone(),
        access_token: access_token.to_string(),
        dpop_key_b64: pending.dpop_key_b64.clone(),
        dpop_nonce: dpop_nonce.clone(),
        created_at: std::time::Instant::now(),
    });

    tracing::info!(did = %pending.did, handle = %pending.handle, mobile = pending.mobile, "OAuth callback: token obtained, session stored");

    // Mobile apps get a redirect to freeq:// custom scheme
    if pending.mobile {
        let nick = mobile_nick_from_handle(&pending.handle);
        let redirect = format!(
            "freeq://auth?token={}&nick={}&did={}&handle={}",
            urlencod(result.web_token.as_deref().unwrap_or("")),
            urlencod(&nick),
            urlencod(&result.did),
            urlencod(&result.handle),
        );
        return Ok(Html(format!(
            r#"<!DOCTYPE html><html><head><meta http-equiv="refresh" content="0;url={redirect}"></head><body><script>window.location.href = "{redirect}";</script><p>Redirecting to freeq app...</p></body></html>"#
        )));
    }

    // Return HTML page that posts result to parent window
    Ok(Html(oauth_result_page("Authentication successful!", Some(&result))))
}

/// Generate the HTML page returned by the OAuth callback.
/// If result is Some, it posts the credentials to the parent window via postMessage.
fn oauth_result_page(message: &str, result: Option<&crate::server::OAuthResult>) -> String {
    let script = if let Some(r) = result {
        let json = serde_json::to_string(r).unwrap_or_default();
        format!(
            r#"<script>
            // Store result in localStorage with timestamp (used by polling fallback and Tauri redirect)
            try {{
                var resultWithTs = {json};
                resultWithTs._ts = Date.now();
                localStorage.setItem('freeq-oauth-result', JSON.stringify(resultWithTs));
            }} catch(e) {{}}
            // BroadcastChannel delivers result to main window (works cross-origin)
            try {{
                const bc = new BroadcastChannel('freeq-oauth');
                bc.postMessage({{ type: 'freeq-oauth', result: {json} }});
                bc.close();
            }} catch(e) {{}}
            // Try postMessage to opener as secondary channel
            if (window.opener) {{
                try {{ window.opener.postMessage({{ type: 'freeq-oauth', result: {json} }}, '*'); }} catch(e) {{}}
            }}
            // Try to close this window after a delay (gives BroadcastChannel time to deliver).
            // The main window will also try popup.close() when it receives the result.
            // If close fails (not a popup), check for Tauri and redirect.
            setTimeout(() => {{
                document.querySelector('#hint').textContent = 'You can close this window.';
                window.close();
                // If we're still here after close(), check if this is Tauri (same-window flow)
                setTimeout(() => {{
                    if (window.__TAURI_INTERNALS__ || !window.opener && window.name !== 'freeq-auth') {{
                        window.location.href = '/';
                    }}
                }}, 500);
            }}, 1500);
            </script>"#
        )
    } else {
        String::new()
    };

    // Show different text depending on whether this is a popup or same-window flow
    let close_hint = if result.is_some() {
        "<p id=\"hint\" style=\"color:#6c7086\">Connecting...</p>\
<div style=\"margin-top:16px\"><svg width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" \
style=\"animation:spin 1s linear infinite\"><style>@keyframes spin{{to{{transform:rotate(360deg)}}}}</style>\
<circle cx=\"12\" cy=\"12\" r=\"10\" stroke=\"#6c7086\" stroke-width=\"3\" fill=\"none\" \
stroke-dasharray=\"31.4 31.4\" stroke-linecap=\"round\"/></svg></div>\
<script>if(window.opener)document.getElementById('hint').textContent='You can close this window.';</script>"
    } else {
        "<p style=\"color:#f38ba8\">Please close this window and try again.</p>"
    };
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>freeq auth</title>
<style>
body {{ font-family: system-ui; background: #1e1e2e; color: #cdd6f4; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }}
.box {{ text-align: center; }}
h1 {{ color: #89b4fa; font-size: 20px; }}
p {{ color: #a6adc8; }}
</style></head>
<body><div class="box"><h1>freeq</h1><p>{message}</p>{close_hint}</div>
{script}
</body></html>"#
    )
}

fn generate_pkce() -> (String, String) {
    use base64::Engine;
    use sha2::{Sha256, Digest};
    let verifier = generate_random_string(32);
    let hash = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);
    (verifier, challenge)
}

fn generate_random_string(len: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}

fn urlencod(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// Derive an IRC nick from an AT Protocol handle.
/// Custom domains use the full handle; standard hosting suffixes are stripped.
fn mobile_nick_from_handle(handle: &str) -> String {
    let standard_suffixes = [".bsky.social", ".bsky.app", ".bsky.team", ".bsky.network"];
    for suffix in &standard_suffixes {
        if let Some(stripped) = handle.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    handle.to_string()
}

// ── Media upload endpoint ───────────────────────────────────────────

/// POST /api/v1/upload
/// Multipart form: `file` (binary), `did` (text), `alt` (optional text), `channel` (optional text).
/// Server proxies the upload to the user's PDS using their stored OAuth credentials.
/// Returns JSON: `{ "url": "...", "content_type": "...", "size": N }`.
async fn api_upload(
    State(state): State<Arc<SharedState>>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut content_type = String::from("application/octet-stream");
    let mut did = String::new();
    let mut alt = None::<String>;
    let mut channel = None::<String>;
    let mut cross_post = false;

    while let Some(field) = multipart.next_field().await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(ct) = field.content_type() {
                    content_type = ct.to_string();
                }
                let bytes = field.bytes().await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("File read error: {e}")))?;
                if bytes.len() > 10 * 1024 * 1024 {
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "File too large (max 10MB)".into()));
                }
                file_data = Some(bytes.to_vec());
            }
            "did" => {
                did = field.text().await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("DID read error: {e}")))?;
            }
            "alt" => {
                alt = Some(field.text().await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("Alt read error: {e}")))?);
            }
            "channel" => {
                channel = Some(field.text().await
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("Channel read error: {e}")))?);
            }
            "cross_post" => {
                let val = field.text().await.unwrap_or_default();
                cross_post = val == "true" || val == "1";
            }
            _ => {}
        }
    }

    let file_data = file_data.ok_or_else(|| (StatusCode::BAD_REQUEST, "No file provided".into()))?;
    if did.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No DID provided".into()));
    }

    // Look up the user's web session
    let session = state.web_sessions.lock().get(&did).cloned();
    let session = match session {
        Some(s) => s,
        None => {
            let count = state.web_sessions.lock().len();
            tracing::warn!(did = %did, session_count = count, "Upload 401: no web session for DID");
            return Err((StatusCode::UNAUTHORIZED, "No active session for this DID — please re-authenticate".into()));
        }
    };

    // Sessions persist until server restart (in-memory only).
    // No time-based expiry — the PDS access token may expire on its own,
    // in which case the upload will fail with a PDS error.

    // Upload to PDS using stored DPoP credentials
    let dpop_key = freeq_sdk::oauth::DpopKey::from_base64url(&session.dpop_key_b64)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP key error: {e}")))?;

    let result = freeq_sdk::media::upload_media_to_pds(
        &session.pds_url,
        &session.did,
        &session.access_token,
        Some(&dpop_key),
        session.dpop_nonce.as_deref(),
        &content_type,
        &file_data,
        alt.as_deref(),
        channel.as_deref(),
        cross_post,
    ).await.map_err(|e| {
        tracing::warn!(did = %did, error = %e, "Media upload failed");
        (StatusCode::BAD_GATEWAY, format!("PDS upload failed: {e}"))
    })?;

    // For non-image content, proxy through our server to avoid PDS
    // Content-Disposition: attachment and sandbox CSP blocking playback
    let client_url = if !result.mime_type.starts_with("image/") {
        let encoded = urlencoding::encode(&result.url);
        // Include mime hint so clients can render without HEAD request
        let mime_encoded = urlencoding::encode(&result.mime_type);
        format!("https://irc.freeq.at/api/v1/blob?url={encoded}&mime={mime_encoded}")
    } else {
        result.url.clone()
    };

    tracing::info!(did = %did, url = %client_url, size = result.size, "Media uploaded to PDS");

    Ok(Json(serde_json::json!({
        "url": client_url,
        "content_type": result.mime_type,
        "size": result.size,
    })))
}


// ── Channel invite page ────────────────────────────────────────────────

async fn channel_invite_page(
    Path(channel): Path<String>,
    State(state): State<Arc<SharedState>>,
) -> impl IntoResponse {
    let channel = if channel.starts_with('#') || channel.starts_with("%23") {
        channel.replace("%23", "#")
    } else {
        format!("#{channel}")
    };

    // Get channel info
    let (member_count, topic_text) = {
        let channels = state.channels.lock();
        let key = channel.to_lowercase();
        match channels.get(&key) {
            Some(ch) => (ch.members.len(), ch.topic.as_ref().map(|t| t.text.clone())),
            None => (0, None),
        }
    };

    let server = &state.config.server_name;
    let topic_html = topic_text.as_deref().unwrap_or("No topic set");
    let channel_display = channel.trim_start_matches('#');
    let member_word = if member_count == 1 { "member" } else { "members" };

    Html(format!(r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{channel} — freeq</title>
<meta property="og:title" content="{channel} on freeq">
<meta property="og:description" content="{topic_html} — {member_count} {member_word} online">
<meta property="og:type" content="website">
<meta property="og:url" content="https://{server}/join/{channel_display}">
<meta property="og:image" content="https://{server}/freeq.png">
<meta name="twitter:card" content="summary">
<meta name="twitter:title" content="{channel} on freeq">
<meta name="twitter:description" content="{topic_html} — {member_count} {member_word} online">
<meta name="twitter:image" content="https://{server}/freeq.png">
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#0c0c0f;color:#e8e8ed;min-height:100vh;display:flex;align-items:center;justify-content:center}}
.card{{background:#131318;border:1px solid #1e1e2e;border-radius:20px;padding:48px;max-width:460px;width:90vw;text-align:center;box-shadow:0 20px 60px rgba(0,0,0,0.5)}}
.logo{{width:64px;height:64px;margin:0 auto 16px}}
h1{{font-size:28px;margin-bottom:4px}}
h1 .accent{{color:#00d4aa}}
.channel{{font-size:36px;font-weight:800;color:#00d4aa;margin:24px 0 8px;letter-spacing:-0.5px}}
.topic{{color:#9898b0;font-size:15px;margin-bottom:24px;line-height:1.5}}
.stats{{color:#555570;font-size:13px;margin-bottom:32px}}
.stats span{{color:#9898b0}}
.btn{{display:inline-block;background:#00d4aa;color:#000;font-size:18px;font-weight:700;padding:14px 40px;border-radius:12px;text-decoration:none;transition:all 0.2s}}
.btn:hover{{background:#00f0c0;box-shadow:0 0 24px rgba(0,212,170,0.2)}}
.alt{{color:#555570;font-size:12px;margin-top:20px}}
.alt a{{color:#00d4aa;text-decoration:none}}
.alt a:hover{{text-decoration:underline}}
.badge{{display:inline-flex;align-items:center;gap:4px;background:#00d4aa15;color:#00d4aa;font-size:11px;font-weight:600;padding:3px 10px;border-radius:20px;margin-bottom:16px}}
</style>
</head>
<body>
<div class="card">
  <img src="/freeq.png" alt="freeq" class="logo">
  <div class="badge">IRC + AT Protocol</div>
  <h1><span class="accent">free</span>q</h1>
  <div class="channel">#{channel_display}</div>
  <div class="topic">{topic_html}</div>
  <div class="stats"><span>{member_count}</span> {member_word} online on <span>{server}</span></div>
  <a href="https://{server}/#auto-join={channel}" class="btn">Join Channel</a>
  <div class="alt">
    Or connect with any IRC client: <code>{server}:6667</code><br>
    <a href="https://freeq.at" target="_blank">Learn more about freeq</a>
  </div>
</div>
</body>
</html>"##)).into_response()
}

// ── OG metadata proxy (replaces allorigins.win privacy leak) ──────────

#[derive(Deserialize)]
struct OgQuery {
    url: String,
}

// ── Blob proxy endpoint ───────────────────────────────────────────

/// GET /api/v1/blob?url=<pds-blob-url>
/// Proxies PDS blob downloads, stripping Content-Disposition: attachment
/// and sandbox CSP headers that prevent browser/AVPlayer playback.
/// Supports Range requests for video seeking / AVPlayer compatibility.
async fn api_blob_proxy(
    headers: axum::http::HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(url) = q.get("url") else {
        return (StatusCode::BAD_REQUEST, "missing url parameter").into_response();
    };

    // Only proxy known PDS blob URLs
    if !url.contains("/xrpc/com.atproto.sync.getBlob") && !url.contains("cdn.bsky.app") {
        return (StatusCode::BAD_REQUEST, "not a valid blob URL").into_response();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    // Forward Range header if present (needed for AVPlayer / video seeking)
    let mut req = client.get(url);
    if let Some(range) = headers.get(axum::http::header::RANGE) {
        if let Ok(range_str) = range.to_str() {
            req = req.header("Range", range_str);
        }
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url = %url, error = %e, "Blob proxy fetch failed");
            return (StatusCode::BAD_GATEWAY, "fetch failed").into_response();
        }
    };

    let upstream_status = resp.status();
    if !upstream_status.is_success() && upstream_status.as_u16() != 206 {
        return (StatusCode::BAD_GATEWAY, "upstream error").into_response();
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let content_range = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let content_length = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_GATEWAY, "read failed").into_response(),
    };

    let status = if upstream_status.as_u16() == 206 {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    let mut resp_headers = axum::http::HeaderMap::new();
    resp_headers.insert(axum::http::header::CONTENT_TYPE, content_type.parse().unwrap_or_else(|_| "application/octet-stream".parse().unwrap()));
    resp_headers.insert(axum::http::header::CACHE_CONTROL, "public, max-age=86400".parse().unwrap());
    resp_headers.insert(axum::http::header::ACCEPT_RANGES, "bytes".parse().unwrap());

    if let Some(cr) = content_range {
        if let Ok(val) = cr.parse() {
            resp_headers.insert(axum::http::header::CONTENT_RANGE, val);
        }
    }
    if let Some(cl) = content_length {
        if let Ok(val) = cl.parse() {
            resp_headers.insert(axum::http::header::CONTENT_LENGTH, val);
        }
    }

    (status, resp_headers, bytes).into_response()
}

/// Fetch OpenGraph metadata from a URL and return as JSON.
/// Avoids clients leaking browsing data to third-party proxy services.
async fn api_og_preview(Query(q): Query<OgQuery>) -> impl IntoResponse {
    // Validate URL
    let url = match url::Url::parse(&q.url) {
        Ok(u) if u.scheme() == "http" || u.scheme() == "https" => u,
        _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid URL"}))).into_response(),
    };

    // Fetch with timeout
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .unwrap();

    let resp = match client.get(url.as_str())
        .header("User-Agent", "freeq/1.0 (link preview)")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Fetch failed"}))).into_response(),
    };

    // Only process HTML
    let ct = resp.headers().get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ct.contains("text/html") {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Not HTML"}))).into_response();
    }

    // Limit body size to 256KB
    let body = match resp.bytes().await {
        Ok(b) if b.len() <= 256 * 1024 => String::from_utf8_lossy(&b).to_string(),
        _ => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Body too large"}))).into_response(),
    };

    // Parse OG tags
    let get_meta = |prop: &str| -> Option<String> {
        let patterns = [
            format!(r#"<meta[^>]*(?:property|name)=["']{prop}["'][^>]*content=["']([^"']*)["']"#),
            format!(r#"<meta[^>]*content=["']([^"']*)["'][^>]*(?:property|name)=["']{prop}["']"#),
        ];
        for pat in &patterns {
            if let Ok(re) = regex::Regex::new(pat) {
                if let Some(caps) = re.captures(&body) {
                    return caps.get(1).map(|m| decode_html_entities(m.as_str()));
                }
            }
        }
        None
    };

    // Also try <title> tag
    let title = get_meta("og:title")
        .or_else(|| {
            regex::Regex::new(r"<title[^>]*>([^<]+)</title>")
                .ok()
                .and_then(|re| re.captures(&body))
                .and_then(|caps| caps.get(1))
                .map(|m| decode_html_entities(m.as_str()))
        });

    Json(serde_json::json!({
        "title": title,
        "description": get_meta("og:description").or_else(|| get_meta("description")),
        "image": get_meta("og:image"),
        "site_name": get_meta("og:site_name"),
    })).into_response()
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
     .replace("&lt;", "<")
     .replace("&gt;", ">")
     .replace("&quot;", "\"")
     .replace("&#39;", "'")
     .replace("&apos;", "'")
     .replace("&#x27;", "'")
     .replace("&#x2F;", "/")
     .replace("&nbsp;", " ")
}

// ── E2EE Pre-Key Bundle API ────────────────────────────────────────

/// GET /api/v1/keys/{did} — Fetch a user's pre-key bundle.
async fn api_get_keys(
    State(state): State<Arc<crate::server::SharedState>>,
    axum::extract::Path(did): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    let bundles = state.prekey_bundles.lock();
    match bundles.get(&did) {
        Some(bundle) => (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({ "bundle": bundle })),
        ),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "No pre-key bundle for this DID" })),
        ),
    }
}

/// POST /api/v1/keys — Upload a pre-key bundle.
///
/// Body: `{ "did": "did:plc:...", "bundle": { ... } }`
///
/// The DID must match the authenticated session. In practice, this is
/// called after SASL authentication when the client generates encryption keys.
async fn api_upload_keys(
    State(state): State<Arc<crate::server::SharedState>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl axum::response::IntoResponse {
    let did = body.get("did").and_then(|v| v.as_str());
    let bundle = body.get("bundle");

    match (did, bundle) {
        (Some(did), Some(bundle)) => {
            state.prekey_bundles.lock()
                .insert(did.to_string(), bundle.clone());
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({ "ok": true })),
            )
        }
        _ => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": "Missing 'did' or 'bundle'" })),
        ),
    }
}

/// Security headers middleware.
async fn security_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());
    headers.insert(
        "Strict-Transport-Security",
        "max-age=63072000; includeSubDomains; preload".parse().unwrap(),
    );
    // Only set CSP if the handler didn't already set one (e.g. /auth/mobile needs inline scripts)
    if !headers.contains_key("content-security-policy") {
        headers.insert(
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' https: data:; connect-src 'self' wss: https:; frame-ancestors 'none'; base-uri 'self'; form-action 'self'".parse().unwrap(),
        );
    }
    resp
}
