//! Reference credential issuer service.
//!
//! A standalone HTTP service that:
//! 1. Verifies identities via external providers (GitHub OAuth, etc.)
//! 2. Issues signed VerifiableCredentials
//! 3. Publishes its Ed25519 public key via a DID document
//!
//! Any freeq server can verify credentials from this issuer by resolving
//! its DID and checking the signature — zero coupling.
//!
//! Usage:
//!   GITHUB_CLIENT_ID=... GITHUB_CLIENT_SECRET=... \
//!     cargo run --bin credential-issuer -- --listen 0.0.0.0:3000 --did did:web:verify.freeq.at
//!
//! The issuer serves:
//!   GET  /.well-known/did.json              — DID document with public key
//!   GET  /verify/github?subject_did=...&org=... — Start GitHub OAuth
//!   GET  /verify/github/callback            — GitHub OAuth callback → issue credential
//!   GET  /health                            — Health check

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::get,
    Json, Router,
};
use clap::Parser;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Parser)]
struct Args {
    /// Listen address.
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: String,

    /// DID for this issuer (e.g. did:web:verify.freeq.at).
    #[arg(long, default_value = "did:web:localhost")]
    did: String,

    /// GitHub OAuth Client ID.
    #[arg(long, env = "GITHUB_CLIENT_ID")]
    github_client_id: String,

    /// GitHub OAuth Client Secret.
    #[arg(long, env = "GITHUB_CLIENT_SECRET")]
    github_client_secret: String,

    /// External URL of this service (for OAuth redirect).
    #[arg(long, default_value = "http://localhost:3000")]
    external_url: String,
}

struct IssuerState {
    signing_key: SigningKey,
    did: String,
    github_client_id: String,
    github_client_secret: String,
    external_url: String,
    /// Pending verifications: state → PendingVerify.
    pending: Mutex<std::collections::HashMap<String, PendingVerify>>,
}

#[derive(Debug, Clone)]
struct PendingVerify {
    subject_did: String,
    org: String,
    created_at: std::time::Instant,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Generate or load signing key
    let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
    let public_key = signing_key.verifying_key();
    let public_key_multibase = format!("z{}", bs58::encode(
        [&[0xed, 0x01], public_key.as_bytes().as_slice()].concat()
    ).into_string());

    tracing::info!("Credential issuer starting");
    tracing::info!("  DID: {}", args.did);
    tracing::info!("  Public key (multibase): {public_key_multibase}");
    tracing::info!("  Listen: {}", args.listen);

    let state = Arc::new(IssuerState {
        signing_key,
        did: args.did,
        github_client_id: args.github_client_id,
        github_client_secret: args.github_client_secret,
        external_url: args.external_url,
        pending: Mutex::new(std::collections::HashMap::new()),
    });

    let app = Router::new()
        .route("/.well-known/did.json", get(did_document))
        .route("/verify/github", get(github_start))
        .route("/verify/github/callback", get(github_callback))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!("Listening on {}", args.listen);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Serve the DID document with our Ed25519 public key.
async fn did_document(State(state): State<Arc<IssuerState>>) -> impl IntoResponse {
    let public_key = state.signing_key.verifying_key();
    let public_key_multibase = format!("z{}", bs58::encode(
        [&[0xed, 0x01], public_key.as_bytes().as_slice()].concat()
    ).into_string());

    let key_id = format!("{}#key-1", state.did);

    Json(serde_json::json!({
        "@context": [
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/multikey/v1"
        ],
        "id": state.did,
        "verificationMethod": [{
            "id": key_id,
            "type": "Multikey",
            "controller": state.did,
            "publicKeyMultibase": public_key_multibase,
        }],
        "assertionMethod": [key_id],
        "authentication": [key_id],
    }))
}

#[derive(Deserialize)]
struct GitHubStartQuery {
    subject_did: String,
    org: String,
}

/// Start GitHub OAuth flow.
async fn github_start(
    Query(q): Query<GitHubStartQuery>,
    State(state): State<Arc<IssuerState>>,
) -> Result<Redirect, (StatusCode, String)> {
    let state_token: String = hex::encode(rand::random::<[u8; 16]>());

    state.pending.lock().unwrap().insert(
        state_token.clone(),
        PendingVerify {
            subject_did: q.subject_did,
            org: q.org,
            created_at: std::time::Instant::now(),
        },
    );

    let redirect_uri = format!("{}/verify/github/callback", state.external_url);
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read:org&state={}",
        state.github_client_id,
        urlencoding::encode(&redirect_uri),
        state_token,
    );

    Ok(Redirect::temporary(&url))
}

/// GitHub OAuth callback → verify org membership → issue credential.
async fn github_callback(
    Query(q): Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<IssuerState>>,
) -> impl IntoResponse {
    let code = match q.get("code") {
        Some(c) => c.clone(),
        None => return axum::response::Html(String::from("<h1>Error</h1><p>No code</p>")).into_response(),
    };
    let oauth_state = match q.get("state") {
        Some(s) => s.clone(),
        None => return axum::response::Html(String::from("<h1>Error</h1><p>No state</p>")).into_response(),
    };

    let pending = state.pending.lock().unwrap().remove(&oauth_state);
    let pending = match pending {
        Some(p) if p.created_at.elapsed() < std::time::Duration::from_secs(300) => p,
        _ => return axum::response::Html(String::from("<h1>Error</h1><p>Expired or unknown</p>")).into_response(),
    };

    let http = reqwest::Client::new();

    // Exchange code for token
    let token_resp = http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", state.github_client_id.as_str()),
            ("client_secret", state.github_client_secret.as_str()),
            ("code", &code),
        ])
        .send()
        .await;

    let token_json: serde_json::Value = match token_resp {
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => return axum::response::Html(format!("<h1>Error</h1><p>{e}</p>")).into_response(),
    };

    let access_token = match token_json["access_token"].as_str() {
        Some(t) => t.to_string(),
        None => return axum::response::Html(String::from("<h1>Error</h1><p>No access token</p>")).into_response(),
    };

    // Get authenticated username
    let user_resp = http
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "freeq-credential-issuer")
        .send()
        .await;
    let user_json: serde_json::Value = match user_resp {
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(_) => serde_json::Value::Null,
    };

    let username = match user_json["login"].as_str() {
        Some(u) => u.to_string(),
        None => return axum::response::Html(String::from("<h1>Error</h1><p>Cannot get username</p>")).into_response(),
    };

    // Check org membership
    let org_ok = http
        .get(&format!("https://api.github.com/user/memberships/orgs/{}", pending.org))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "freeq-credential-issuer")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if !org_ok {
        return axum::response::Html(format!(
            "<h1>Not a member</h1><p>{username} is not a member of {}</p>",
            pending.org
        )).into_response();
    }

    // Issue signed credential
    let mut vc = freeq_server::policy::VerifiableCredential {
        credential_type_tag: "FreeqCredential/v1".into(),
        issuer: state.did.clone(),
        subject: pending.subject_did.clone(),
        credential_type: "github_membership".into(),
        claims: serde_json::json!({
            "github_username": username,
            "org": pending.org,
        }),
        issued_at: chrono::Utc::now().to_rfc3339(),
        expires_at: Some((chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339()),
        signature: String::new(),
    };

    freeq_server::policy::credentials::sign_credential(&mut vc, &state.signing_key).unwrap();

    let vc_json = serde_json::to_string_pretty(&vc).unwrap_or_default();

    // Return the credential (user can present it to any freeq server)
    let html = format!(
        r#"<!DOCTYPE html><html><head><title>freeq — Credential Issued</title>
        <style>
            body {{ font-family: system-ui; max-width: 600px; margin: 40px auto; padding: 0 20px; }}
            h1 {{ color: #0a0; }}
            .badge {{ background: #0a0; color: white; padding: 4px 12px; border-radius: 12px; font-size: 14px; }}
            pre {{ background: #1a1a2e; color: #0f0; padding: 16px; border-radius: 8px; overflow-x: auto; font-size: 12px; }}
            .copy {{ background: #333; color: #fff; border: none; padding: 8px 16px; border-radius: 4px; cursor: pointer; }}
            .instructions {{ background: #f0f8ff; padding: 16px; border-radius: 8px; margin-top: 16px; }}
        </style></head>
        <body>
            <h1>✓ Credential Issued</h1>
            <p><span class="badge">{username}</span> is a member of <span class="badge">{org}</span></p>
            <p>Bound to DID: <code>{did}</code></p>
            <p>Expires: {expires}</p>

            <h3>Your Verifiable Credential</h3>
            <pre id="vc">{vc_json}</pre>
            <button class="copy" onclick="navigator.clipboard.writeText(document.getElementById('vc').textContent)">
                📋 Copy to clipboard
            </button>

            <div class="instructions">
                <h3>How to use this credential</h3>
                <p>Present it to any freeq server:</p>
                <pre>curl -X POST https://irc.freeq.at/api/v1/credentials/present \
  -H 'Content-Type: application/json' \
  -d '{{"credential": {vc_oneline} }}'</pre>
                <p>Or in IRC: <code>/POLICY #channel ACCEPT</code> (if credentials are auto-collected)</p>
            </div>
        </body></html>"#,
        org = pending.org,
        did = pending.subject_did,
        expires = vc.expires_at.as_deref().unwrap_or("never"),
        vc_oneline = serde_json::to_string(&vc).unwrap_or_default().replace('"', "&quot;"),
    );

    axum::response::Html(html).into_response()
}
