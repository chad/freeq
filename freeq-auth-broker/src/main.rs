use std::sync::Arc;
use std::time::SystemTime;

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect},
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::CorsLayer;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use p256::ecdsa::SigningKey;
use base64::Engine;

#[derive(Clone)]
struct BrokerConfig {
    public_url: String,
    freeq_server_url: String,
    shared_secret: String,
    db_path: String,
}

struct BrokerState {
    config: BrokerConfig,
    pending: Mutex<std::collections::HashMap<String, PendingAuth>>,
    db: Mutex<rusqlite::Connection>,
}

#[derive(Clone)]
struct PendingAuth {
    handle: String,
    did: String,
    pds_url: String,
    code_verifier: String,
    redirect_uri: String,
    client_id: String,
    token_endpoint: String,
    dpop_key_b64: String,
    dpop_nonce: Option<String>,
    mobile: bool,
    return_to: Option<String>,
}

#[derive(Debug, Clone)]
struct DpopKey {
    signing_key: SigningKey,
}

impl DpopKey {
    fn generate() -> Self {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        Self { signing_key }
    }

    fn to_base64url(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.signing_key.to_bytes())
    }

    fn from_base64url(s: &str) -> Result<Self, anyhow::Error> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s)?;
        let signing_key = SigningKey::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("Invalid DPoP key: {e}"))?;
        Ok(Self { signing_key })
    }

    fn jwk(&self) -> serde_json::Value {
        let verifying_key = self.signing_key.verifying_key();
        let point = verifying_key.to_encoded_point(false);
        let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(point.x().unwrap());
        let y = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(point.y().unwrap());
        serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": x,
            "y": y,
        })
    }

    fn proof(&self, method: &str, url: &str, nonce: Option<&str>, access_token: Option<&str>) -> Result<String, anyhow::Error> {
        use p256::ecdsa::{Signature, signature::Signer};
        use sha2::{Digest, Sha256};
        use base64::Engine;

        let header = serde_json::json!({
            "typ": "dpop+jwt",
            "alg": "ES256",
            "jwk": self.jwk(),
        });

        let mut payload = serde_json::json!({
            "jti": generate_random_string(16),
            "htm": method,
            "htu": url,
            "iat": chrono::Utc::now().timestamp(),
        });
        if let Some(nonce) = nonce {
            payload["nonce"] = serde_json::Value::String(nonce.to_string());
        }
        if let Some(token) = access_token {
            let hash = Sha256::digest(token.as_bytes());
            payload["ath"] = serde_json::Value::String(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash));
        }

        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header)?);
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
        let signing_input = format!("{header_b64}.{payload_b64}");

        let sig: Signature = self.signing_key.sign(signing_input.as_bytes());
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());

        Ok(format!("{signing_input}.{sig_b64}"))
    }
}

#[derive(Debug, Deserialize)]
struct DidDocument {
    #[serde(default)]
    service: Vec<DidService>,
}

#[derive(Debug, Deserialize)]
struct DidService {
    #[serde(rename = "type")]
    service_type: String,
    #[serde(rename = "serviceEndpoint")]
    service_endpoint: String,
}

async fn resolve_handle(handle: &str) -> Result<String, anyhow::Error> {
    // Try HTTPS well-known first
    let url = format!("https://{handle}/.well-known/atproto-did");
    if let Ok(resp) = reqwest::get(&url).await {
        if resp.status().is_success() {
            let did = resp.text().await?.trim().to_string();
            if did.starts_with("did:") {
                return Ok(did);
            }
        }
    }

    // Fallback to public API (DNS TXT)
    let api_url = format!(
        "https://public.api.bsky.app/xrpc/com.atproto.identity.resolveHandle?handle={}",
        handle
    );
    let json: serde_json::Value = reqwest::get(&api_url).await?.json().await?;
    let did = json["did"].as_str().ok_or_else(|| anyhow::anyhow!("No DID in response"))?;
    Ok(did.to_string())
}

async fn resolve_did(did: &str) -> Result<DidDocument, anyhow::Error> {
    if did.starts_with("did:plc:") {
        let url = format!("https://plc.directory/{did}");
        let doc: DidDocument = reqwest::get(&url).await?.json().await?;
        return Ok(doc);
    }
    if did.starts_with("did:web:") {
        let domain = did.trim_start_matches("did:web:").replace(':', "/");
        let url = format!("https://{domain}/.well-known/did.json");
        let doc: DidDocument = reqwest::get(&url).await?.json().await?;
        return Ok(doc);
    }
    Err(anyhow::anyhow!("Unsupported DID method"))
}

fn pds_endpoint(doc: &DidDocument) -> Option<String> {
    doc.service.iter().find_map(|svc| {
        if svc.service_type == "AtprotoPersonalDataServer" {
            Some(svc.service_endpoint.clone())
        } else {
            None
        }
    })
}

#[derive(Deserialize)]
struct AuthLoginQuery {
    handle: String,
    mobile: Option<String>,
    return_to: Option<String>,
    popup: Option<String>,
}

#[derive(Deserialize)]
struct AuthCallbackQuery {
    state: String,
    code: String,
}

#[derive(Deserialize)]
struct BrokerSessionRequest {
    broker_token: String,
}

#[derive(Serialize)]
struct BrokerSessionResponse {
    token: String,
    nick: String,
    did: String,
    handle: String,
}

#[derive(Serialize)]
struct BrokerSessionRecord {
    broker_token: String,
    did: String,
    handle: String,
    pds_url: String,
    token_endpoint: String,
    refresh_token: String,
    dpop_key_b64: String,
    dpop_nonce: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let public_url = std::env::var("BROKER_PUBLIC_URL").unwrap_or_else(|_| "http://127.0.0.1:8081".to_string());
    let freeq_server_url = std::env::var("FREEQ_SERVER_URL").unwrap_or_else(|_| "https://irc.freeq.at".to_string());
    let shared_secret = std::env::var("BROKER_SHARED_SECRET").unwrap_or_else(|_| "".to_string());
    let db_path = std::env::var("BROKER_DB_PATH").unwrap_or_else(|_| "broker.db".to_string());

    if shared_secret.is_empty() {
        tracing::warn!("BROKER_SHARED_SECRET not set — broker cannot mint web-tokens");
    }

    let db = rusqlite::Connection::open(&db_path).expect("Failed to open broker db");
    init_db(&db).expect("Failed to init db");

    let state = Arc::new(BrokerState {
        config: BrokerConfig { public_url, freeq_server_url, shared_secret, db_path },
        pending: Mutex::new(std::collections::HashMap::new()),
        db: Mutex::new(db),
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/client-metadata.json", get(client_metadata))
        .route("/auth/login", get(auth_login))
        .route("/auth/callback", get(auth_callback))
        .route("/session", post(session))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = std::env::var("BROKER_ADDR").unwrap_or_else(|_| {
        if let Ok(port) = std::env::var("PORT") {
            format!("0.0.0.0:{port}")
        } else {
            "0.0.0.0:8081".to_string()
        }
    });
    tracing::info!(%addr, "freeq auth broker listening");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "ok"
}

async fn client_metadata(State(state): State<Arc<BrokerState>>) -> Json<serde_json::Value> {
    let redirect_uri = format!("{}/auth/callback", state.config.public_url.trim_end_matches('/'));
    let client_id = build_client_id(&state.config.public_url, &redirect_uri);
    Json(serde_json::json!({
        "client_id": client_id,
        "client_name": "freeq-auth-broker",
        "client_uri": state.config.public_url,
        "logo_uri": format!("{}/freeq.png", state.config.public_url),
        "tos_uri": state.config.public_url,
        "policy_uri": state.config.public_url,
        "redirect_uris": [redirect_uri],
        "scope": "atproto transition:generic",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "web",
        "dpop_bound_access_tokens": true
    }))
}

async fn auth_login(
    Query(q): Query<AuthLoginQuery>,
    State(state): State<Arc<BrokerState>>,
    headers: HeaderMap,
) -> Result<Redirect, (StatusCode, String)> {
    let handle = q.handle.trim().to_string();
    let did = resolve_handle(&handle).await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot resolve handle: {e}")))?;
    let did_doc = resolve_did(&did).await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot resolve DID: {e}")))?;
    let pds_url = pds_endpoint(&did_doc)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "No PDS in DID document".to_string()))?;

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

    let redirect_uri = format!("{}/auth/callback", state.config.public_url.trim_end_matches('/'));
    let scope = "atproto transition:generic";
    let client_id = build_client_id(&state.config.public_url, &redirect_uri);

    let dpop_key = DpopKey::generate();
    let (code_verifier, code_challenge) = generate_pkce();
    let oauth_state = generate_random_string(16);

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

    let dpop_proof = dpop_key.proof("POST", par_endpoint, None, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP proof failed: {e}")))?;
    let resp = client.post(par_endpoint).header("DPoP", &dpop_proof).form(&params).send().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("PAR failed: {e}")))?;

    let status = resp.status();
    let dpop_nonce = resp.headers().get("dpop-nonce")
        .and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let par_resp: serde_json::Value = if status.as_u16() == 400 && dpop_nonce.is_some() {
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut return_to = q.return_to.clone();
    let is_popup = matches!(q.popup.as_deref(), Some("1"));
    if !is_popup && return_to.is_none() {
        if let Some(referer) = headers.get("referer").and_then(|v| v.to_str().ok()) {
            if let Ok(url) = url::Url::parse(referer) {
                return_to = Some(url.origin().ascii_serialization());
            }
        }
    }
    if !is_popup && return_to.is_none() && !matches!(q.mobile.as_deref(), Some("1")) {
        return_to = Some("https://irc.freeq.at".to_string());
    }

    state.pending.lock().await.insert(oauth_state.clone(), PendingAuth {
        handle: handle.clone(),
        did: did.clone(),
        pds_url: pds_url.clone(),
        code_verifier,
        redirect_uri: redirect_uri.clone(),
        client_id: client_id.clone(),
        token_endpoint: token_endpoint.to_string(),
        dpop_key_b64: dpop_key.to_base64url(),
        dpop_nonce: dpop_nonce.clone(),
        mobile: q.mobile.as_deref() == Some("1"),
        return_to,
    });

    let auth_url = format!(
        "{}?client_id={}&request_uri={}",
        authorization_endpoint, urlencod(&client_id), urlencod(request_uri)
    );

    tracing::info!(handle = %handle, did = %did, "OAuth login started");
    Ok(Redirect::temporary(&auth_url))
}

async fn auth_callback(
    Query(q): Query<AuthCallbackQuery>,
    State(state): State<Arc<BrokerState>>,
) -> Result<Html<String>, (StatusCode, String)> {
    let pending = {
        let mut pending_map = state.pending.lock().await;
        pending_map.remove(&q.state)
    };
    let pending = pending.ok_or((StatusCode::BAD_REQUEST, "Invalid state".to_string()))?;

    let dpop_key = DpopKey::from_base64url(&pending.dpop_key_b64)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid DPoP key: {e}")))?;

    let params = [
        ("grant_type", "authorization_code"),
        ("code", q.code.as_str()),
        ("redirect_uri", pending.redirect_uri.as_str()),
        ("client_id", pending.client_id.as_str()),
        ("code_verifier", pending.code_verifier.as_str()),
    ];

    let client = reqwest::Client::new();
    let dpop_proof = dpop_key.proof("POST", &pending.token_endpoint, None, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DPoP proof failed: {e}")))?;
    let resp = client.post(&pending.token_endpoint).header("DPoP", &dpop_proof).form(&params).send().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token exchange failed: {e}")))?;

    let status = resp.status();
    let dpop_nonce = resp.headers().get("dpop-nonce")
        .and_then(|v| v.to_str().ok()).map(|s| s.to_string())
        .or(pending.dpop_nonce.clone());

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

    let refresh_token = token_resp["refresh_token"].as_str()
        .ok_or((StatusCode::BAD_GATEWAY, "No refresh_token".to_string()))?;

    let broker_token = generate_random_string(32);
    let now = chrono::Utc::now().timestamp();
    {
        let db = state.db.lock().await;
        db.execute(
            "INSERT INTO sessions (broker_token, did, handle, pds_url, token_endpoint, refresh_token, dpop_key_b64, dpop_nonce, created_at, updated_at)\
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)\
             ON CONFLICT(broker_token) DO UPDATE SET refresh_token=excluded.refresh_token, updated_at=excluded.updated_at",
            rusqlite::params![
                broker_token,
                pending.did,
                pending.handle,
                pending.pds_url,
                pending.token_endpoint,
                refresh_token,
                pending.dpop_key_b64,
                dpop_nonce,
                now,
                now
            ],
        ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    }

    // Mint a one-time web-token + web session on the freeq server
    let (web_token, nick) = mint_web_token(&state.config, &pending.did, &pending.handle).await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Broker token mint failed: {e}")))?;

    if let Err(e) = push_web_session(&state.config, &pending, &token_resp, dpop_nonce.clone()).await {
        tracing::warn!(error = %e, "Failed to push web session to server");
    }

    if pending.mobile {
        let redirect = format!(
            "freeq://auth?token={}&broker_token={}&nick={}&did={}&handle={}",
            urlencod(&web_token),
            urlencod(&broker_token),
            urlencod(&nick),
            urlencod(&pending.did),
            urlencod(&pending.handle),
        );
        return Ok(Html(format!(
            r#"<!DOCTYPE html><html><head><meta http-equiv=\"refresh\" content=\"0;url={redirect}\"></head><body><script>window.location.href = \"{redirect}\";</script><p>Redirecting to freeq app...</p></body></html>"#
        )));
    }

    let result = serde_json::json!({
        "token": web_token,
        "broker_token": broker_token,
        "nick": nick,
        "did": pending.did,
        "handle": pending.handle,
        "pds_url": pending.pds_url,
    });

    if let Some(ref return_to) = pending.return_to {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&result).unwrap_or_default());
        let redirect = format!("{return_to}#oauth={payload}");
        return Ok(Html(format!(
            r#"<!DOCTYPE html><html><head><meta http-equiv=\"refresh\" content=\"0;url={redirect}\"></head><body><script>window.location.href = \"{redirect}\";</script><p>Redirecting to freeq...</p></body></html>"#
        )));
    }

    Ok(Html(oauth_result_page("Authentication successful!", Some(&result))))
}

async fn session(
    State(state): State<Arc<BrokerState>>,
    Json(req): Json<BrokerSessionRequest>,
) -> Result<Json<BrokerSessionResponse>, (StatusCode, String)> {
    let record = get_session(&state, &req.broker_token).await
        .ok_or((StatusCode::UNAUTHORIZED, "Invalid broker token".to_string()))?;

    let (access_token, refresh_token, dpop_nonce) = refresh_access_token(&state.config, &record).await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Refresh failed: {e}")))?;

    // Update stored refresh token + nonce
    let now = chrono::Utc::now().timestamp();
    {
        let db = state.db.lock().await;
        db.execute(
            "UPDATE sessions SET refresh_token = ?1, dpop_nonce = ?2, updated_at = ?3 WHERE broker_token = ?4",
            rusqlite::params![refresh_token, dpop_nonce, now, record.broker_token],
        ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    }

    let (web_token, nick) = mint_web_token(&state.config, &record.did, &record.handle).await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Broker token mint failed: {e}")))?;

    let pending = PendingAuth {
        handle: record.handle.clone(),
        did: record.did.clone(),
        pds_url: record.pds_url.clone(),
        code_verifier: String::new(),
        redirect_uri: String::new(),
        client_id: String::new(),
        token_endpoint: record.token_endpoint.clone(),
        dpop_key_b64: record.dpop_key_b64.clone(),
        dpop_nonce: dpop_nonce.clone(),
        mobile: true,
        return_to: None,
    };
    if let Err(e) = push_web_session_with_token(&state.config, &pending, &access_token, dpop_nonce.clone()).await {
        tracing::warn!(error = %e, "Failed to refresh web session on server");
    }

    Ok(Json(BrokerSessionResponse {
        token: web_token,
        nick,
        did: record.did,
        handle: record.handle,
    }))
}

async fn get_session(state: &Arc<BrokerState>, broker_token: &str) -> Option<BrokerSessionRecord> {
    let db = state.db.lock().await;
    let mut stmt = db.prepare(
        "SELECT broker_token, did, handle, pds_url, token_endpoint, refresh_token, dpop_key_b64, dpop_nonce, created_at, updated_at FROM sessions WHERE broker_token = ?1"
    ).ok()?;
    let mut rows = stmt.query(rusqlite::params![broker_token]).ok()?;
    if let Some(row) = rows.next().ok().flatten() {
        Some(BrokerSessionRecord {
            broker_token: row.get(0).ok()?,
            did: row.get(1).ok()?,
            handle: row.get(2).ok()?,
            pds_url: row.get(3).ok()?,
            token_endpoint: row.get(4).ok()?,
            refresh_token: row.get(5).ok()?,
            dpop_key_b64: row.get(6).ok()?,
            dpop_nonce: row.get(7).ok()?,
            created_at: row.get(8).ok()?,
            updated_at: row.get(9).ok()?,
        })
    } else {
        None
    }
}

async fn refresh_access_token(config: &BrokerConfig, record: &BrokerSessionRecord) -> Result<(String, String, Option<String>), anyhow::Error> {
    let dpop_key = DpopKey::from_base64url(&record.dpop_key_b64)?;
    let redirect_uri = format!("{}/auth/callback", config.public_url.trim_end_matches('/'));
    let client_id = build_client_id(&config.public_url, &redirect_uri);
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", record.refresh_token.as_str()),
        ("client_id", client_id.as_str()),
    ];

    let client = reqwest::Client::new();
    let dpop_proof = dpop_key.proof("POST", &record.token_endpoint, None, None)?;
    let resp = client.post(&record.token_endpoint).header("DPoP", &dpop_proof).form(&params).send().await?;
    let status = resp.status();
    let mut dpop_nonce = resp.headers().get("dpop-nonce")
        .and_then(|v| v.to_str().ok()).map(|s| s.to_string())
        .or(record.dpop_nonce.clone());

    let token_resp: serde_json::Value = if (status.as_u16() == 400 || status.as_u16() == 401) && dpop_nonce.is_some() {
        let nonce = dpop_nonce.as_deref().unwrap();
        let dpop_proof2 = dpop_key.proof("POST", &record.token_endpoint, Some(nonce), None)?;
        let resp2 = client.post(&record.token_endpoint).header("DPoP", &dpop_proof2).form(&params).send().await?;
        if !resp2.status().is_success() {
            return Err(anyhow::anyhow!("Refresh failed: {}", resp2.text().await.unwrap_or_default()));
        }
        resp2.json().await?
    } else if status.is_success() {
        resp.json().await?
    } else {
        return Err(anyhow::anyhow!("Refresh failed ({status})"));
    };

    let access_token = token_resp["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("No access_token"))?.to_string();
    let refresh_token = token_resp["refresh_token"].as_str().unwrap_or(&record.refresh_token).to_string();
    dpop_nonce = token_resp.get("dpop_nonce").and_then(|v| v.as_str()).map(|s| s.to_string()).or(dpop_nonce);

    Ok((access_token, refresh_token, dpop_nonce))
}

async fn mint_web_token(config: &BrokerConfig, did: &str, handle: &str) -> Result<(String, String), anyhow::Error> {
    let body = serde_json::json!({"did": did, "handle": handle});
    let sig = sign_body(&config.shared_secret, &body)?;
    let url = format!("{}/auth/broker/web-token", config.freeq_server_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client.post(&url)
        .header("X-Broker-Signature", sig)
        .json(&body)
        .send().await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("web-token failed: {}", resp.text().await.unwrap_or_default()));
    }
    let json: serde_json::Value = resp.json().await?;
    let token = json["token"].as_str().unwrap_or_default().to_string();
    let nick = json["nick"].as_str().unwrap_or_default().to_string();
    Ok((token, nick))
}

async fn push_web_session(config: &BrokerConfig, pending: &PendingAuth, token_resp: &serde_json::Value, dpop_nonce: Option<String>) -> Result<(), anyhow::Error> {
    let access_token = token_resp["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("No access_token"))?;
    push_web_session_with_token(config, pending, access_token, dpop_nonce).await
}

async fn push_web_session_with_token(config: &BrokerConfig, pending: &PendingAuth, access_token: &str, dpop_nonce: Option<String>) -> Result<(), anyhow::Error> {
    let body = serde_json::json!({
        "did": pending.did,
        "handle": pending.handle,
        "pds_url": pending.pds_url,
        "access_token": access_token,
        "dpop_key_b64": pending.dpop_key_b64,
        "dpop_nonce": dpop_nonce,
    });
    let sig = sign_body(&config.shared_secret, &body)?;
    let url = format!("{}/auth/broker/session", config.freeq_server_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client.post(&url)
        .header("X-Broker-Signature", sig)
        .json(&body)
        .send().await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("session push failed: {}", resp.text().await.unwrap_or_default()));
    }
    Ok(())
}

fn sign_body(secret: &str, body: &serde_json::Value) -> Result<String, anyhow::Error> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use base64::Engine;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    let bytes = serde_json::to_vec(body)?;
    mac.update(&bytes);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn init_db(db: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            broker_token TEXT PRIMARY KEY,
            did TEXT NOT NULL,
            handle TEXT NOT NULL,
            pds_url TEXT NOT NULL,
            token_endpoint TEXT NOT NULL,
            refresh_token TEXT NOT NULL,
            dpop_key_b64 TEXT NOT NULL,
            dpop_nonce TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );"
    )?;
    Ok(())
}

fn oauth_result_page(message: &str, result: Option<&serde_json::Value>) -> String {
    let script = if let Some(r) = result {
        let json = serde_json::to_string(r).unwrap_or_default();
        format!(r#"<script>
            try {{
                var resultWithTs = {json};
                resultWithTs._ts = Date.now();
                window.opener && window.opener.postMessage({{ type: 'freeq-oauth', payload: resultWithTs }}, '*');
                localStorage.setItem('freeq-oauth-result', JSON.stringify(resultWithTs));
            }} catch (e) {{}}
        </script>"#)
    } else { String::new() };

    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>freeq auth</title>
        <style>
        body {{ font-family: system-ui; background: #1e1e2e; color: #cdd6f4; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }}
        .box {{ text-align: center; }}
        h1 {{ color: #89b4fa; font-size: 20px; }}
        p {{ color: #a6adc8; }}
        </style></head>
        <body><div class="box"><h1>freeq</h1><p>{message}</p></div>
        {script}
        </body></html>"#
    )
}

fn generate_pkce() -> (String, String) {
    use base64::Engine;
    use sha2::{Digest, Sha256};
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

fn build_client_id(web_origin: &str, redirect_uri: &str) -> String {
    if web_origin.starts_with("http://127.") || web_origin.starts_with("http://192.168.") || web_origin.starts_with("http://10.") {
        let scope = "atproto transition:generic";
        format!(
            "http://localhost?redirect_uri={}&scope={}",
            urlencod(redirect_uri), urlencod(scope),
        )
    } else {
        format!("{web_origin}/client-metadata.json")
    }
}
