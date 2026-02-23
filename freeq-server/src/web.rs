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
use axum::routing::get;
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
        .route("/client-metadata.json", get(client_metadata))
        // REST API (read-only, v1)
        .route("/api/v1/health", get(api_health))
        .route("/api/v1/channels", get(api_channels))
        .route("/api/v1/channels/{name}/history", get(api_channel_history))
        .route("/api/v1/channels/{name}/topic", get(api_channel_topic))
        .route("/api/v1/users/{nick}", get(api_user))
        .route("/api/v1/users/{nick}/whois", get(api_user_whois))
        .route("/api/v1/upload", axum::routing::post(api_upload))
        .route("/api/v1/og", get(api_og_preview))
        .layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024)) // 12MB
        .layer(CorsLayer::permissive());

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
        crate::verifiers::router(issuer_did, github_config).map(|(r, _)| r)
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
    final_app
}

// ── WebSocket handler ──────────────────────────────────────────────────

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<SharedState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: Arc<SharedState>) {
    let stream = bridge_ws(socket);
    if let Err(e) = crate::connection::handle_generic(stream, state).await {
        tracing::error!("WebSocket connection error: {e}");
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

async fn api_health(State(state): State<Arc<SharedState>>) -> Json<HealthResponse> {
    let start = START_TIME.get_or_init(SystemTime::now);
    let uptime = start.elapsed().unwrap_or_default().as_secs();
    let connections = state.connections.lock().unwrap().len();
    let channels = state.channels.lock().unwrap().len();
    Json(HealthResponse {
        server_name: state.server_name.clone(),
        connections,
        channels,
        uptime_secs: uptime,
    })
}

async fn api_channels(State(state): State<Arc<SharedState>>) -> Json<Vec<ChannelInfo>> {
    let channels = state.channels.lock().unwrap();
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
            let channels = state.channels.lock().unwrap();
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

    let channels = state.channels.lock().unwrap();
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
    let session = state.nick_to_session.lock().unwrap().get(&nick).cloned();
    let online = session.is_some();

    let (did, handle) = if let Some(ref session_id) = session {
        let did = state.session_dids.lock().unwrap().get(session_id).cloned();
        let handle = state
            .session_handles
            .lock()
            .unwrap()
            .get(session_id)
            .cloned();
        (did, handle)
    } else {
        let did = state
            .nick_owners
            .lock()
            .unwrap()
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
    let session = state.nick_to_session.lock().unwrap().get(&nick).cloned();
    let online = session.is_some();

    let (did, handle) = if let Some(ref session_id) = session {
        let did = state.session_dids.lock().unwrap().get(session_id).cloned();
        let handle = state
            .session_handles
            .lock()
            .unwrap()
            .get(session_id)
            .cloned();
        (did, handle)
    } else {
        let did = state
            .nick_owners
            .lock()
            .unwrap()
            .get(&nick.to_lowercase())
            .cloned();
        (did, None)
    };

    if !online && did.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let channels = if let Some(ref session_id) = session {
        let chans = state.channels.lock().unwrap();
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
    state.oauth_pending.lock().unwrap().insert(oauth_state.clone(), crate::server::OAuthPending {
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
    let pending = state.oauth_pending.lock().unwrap().remove(oauth_state)
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
    state.web_auth_tokens.lock().unwrap().insert(
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
    state.web_sessions.lock().unwrap().insert(pending.did.clone(), crate::server::WebSession {
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
    let session = state.web_sessions.lock().unwrap().get(&did).cloned()
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "No active session for this DID — please re-authenticate".into()))?;

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

    tracing::info!(did = %did, url = %result.url, size = result.size, "Media uploaded to PDS");

    Ok(Json(serde_json::json!({
        "url": result.url,
        "content_type": result.mime_type,
        "size": result.size,
    })))
}


// ── OG metadata proxy (replaces allorigins.win privacy leak) ──────────

#[derive(Deserialize)]
struct OgQuery {
    url: String,
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
