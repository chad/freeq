//! GitHub org membership verifier.
//!
//! Routes:
//!   GET /verify/github/start?subject_did=...&org=...&callback=...
//!     → Redirect to GitHub OAuth
//!   GET /verify/github/callback
//!     → Exchange code, verify org membership, sign credential, POST to callback

use super::{PendingVerification, VerifierState};
use crate::policy::credentials;
use crate::policy::types::VerifiableCredential;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<VerifierState>> {
    Router::new()
        .route("/verify/github/start", get(start))
        .route("/verify/github/callback", get(callback))
}

#[derive(Deserialize)]
struct StartQuery {
    /// DID of the user (proven via AT Protocol auth on the freeq server).
    subject_did: String,
    /// GitHub org to verify membership for.
    org: String,
    /// URL to POST the signed credential to after verification.
    callback: String,
}

async fn start(
    Query(q): Query<StartQuery>,
    State(state): State<Arc<VerifierState>>,
) -> Result<Redirect, (StatusCode, String)> {
    let github = state
        .github
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "GitHub not configured".into()))?;

    let state_token = hex::encode(rand::random::<[u8; 16]>());

    state.pending.lock().unwrap().insert(
        state_token.clone(),
        PendingVerification {
            subject_did: q.subject_did,
            callback_url: q.callback,
            provider_params: serde_json::json!({ "org": q.org }),
            created_at: std::time::Instant::now(),
        },
    );

    // Build redirect URI — points back to OUR callback, not the freeq server's
    // The callback_url is where we POST the credential AFTER verification
    let redirect_uri = format!(
        "{}/verify/github/callback",
        // Derive from the state — we need the external URL
        // For now, use a relative path (works when colocated)
        ""
    );

    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&scope=read:org&state={}",
        github.client_id, state_token,
    );

    Ok(Redirect::temporary(&url))
}

async fn callback(
    Query(q): Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<VerifierState>>,
) -> impl IntoResponse {
    let code = match q.get("code") {
        Some(c) => c.clone(),
        None => return error_page("No authorization code from GitHub"),
    };
    let oauth_state = match q.get("state") {
        Some(s) => s.clone(),
        None => return error_page("Missing state parameter"),
    };

    // Look up pending
    let pending = state.pending.lock().unwrap().remove(&oauth_state);
    let pending = match pending {
        Some(p) if p.created_at.elapsed() < std::time::Duration::from_secs(300) => p,
        Some(_) => return error_page("Verification expired. Please try again."),
        None => return error_page("Unknown or expired verification"),
    };

    let github = match &state.github {
        Some(g) => g,
        None => return error_page("GitHub not configured"),
    };

    let org = pending
        .provider_params
        .get("org")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let http = reqwest::Client::new();

    // Exchange code for token
    let token_json: serde_json::Value = match http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", github.client_id.as_str()),
            ("client_secret", github.client_secret.as_str()),
            ("code", &code),
        ])
        .send()
        .await
    {
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => return error_page(&format!("Token exchange failed: {e}")),
    };

    let access_token = match token_json["access_token"].as_str() {
        Some(t) => t.to_string(),
        None => {
            let err = token_json["error_description"]
                .as_str()
                .or(token_json["error"].as_str())
                .unwrap_or("unknown error");
            return error_page(&format!("GitHub OAuth failed: {err}"));
        }
    };

    // Get authenticated username (this proves identity — not self-attested)
    let user_json: serde_json::Value = match http
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "freeq-verifier")
        .send()
        .await
    {
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => return error_page(&format!("GitHub API error: {e}")),
    };

    let username = match user_json["login"].as_str() {
        Some(u) => u.to_string(),
        None => return error_page("Could not determine GitHub username"),
    };

    // Check org membership with authenticated token
    let is_member = http
        .get(&format!(
            "https://api.github.com/user/memberships/orgs/{}",
            org
        ))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "freeq-verifier")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if !is_member {
        // Fall back to public membership check
        let is_public = http
            .get(&format!(
                "https://api.github.com/orgs/{}/public_members/{}",
                org, username
            ))
            .header("User-Agent", "freeq-verifier")
            .send()
            .await
            .map(|r| r.status().as_u16() == 204)
            .unwrap_or(false);

        if !is_public {
            return error_page(&format!(
                "{username} is not a member of {org}. \
                 If you are a member, ensure your membership is public at \
                 https://github.com/orgs/{org}/people"
            ));
        }
    }

    // Issue signed credential
    let mut vc = VerifiableCredential {
        credential_type_tag: "FreeqCredential/v1".into(),
        issuer: state.issuer_did.clone(),
        subject: pending.subject_did.clone(),
        credential_type: "github_membership".into(),
        claims: serde_json::json!({
            "github_username": username,
            "org": org,
        }),
        issued_at: chrono::Utc::now().to_rfc3339(),
        expires_at: Some(
            (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
        ),
        signature: String::new(),
    };
    credentials::sign_credential(&mut vc, &state.signing_key).unwrap();

    tracing::info!(
        subject = %pending.subject_did,
        github = %username,
        org = %org,
        "GitHub org membership verified, credential issued"
    );

    // POST credential to callback URL (the freeq server's /api/v1/credentials/present)
    let callback_result = if !pending.callback_url.is_empty() {
        http.post(&pending.callback_url)
            .json(&serde_json::json!({ "credential": vc }))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    } else {
        false
    };

    // Success page
    let vc_json = serde_json::to_string_pretty(&vc).unwrap_or_default();
    let callback_status = if callback_result {
        "<p style='color:#0a0'>✓ Credential automatically delivered to the server. You can close this window.</p>"
    } else {
        "<p>Credential was not auto-delivered. Copy it and present manually.</p>"
    };

    let html = format!(
        r#"<!DOCTYPE html><html><head><title>freeq — Verified</title>
<style>
body {{ font-family: system-ui; max-width: 600px; margin: 40px auto; padding: 0 20px; background: #0a0a1a; color: #e0e0e0; }}
h1 {{ color: #0f0; }}
.badge {{ background: #0a0; color: white; padding: 3px 10px; border-radius: 10px; font-size: 14px; }}
pre {{ background: #1a1a2e; color: #0f0; padding: 16px; border-radius: 8px; overflow-x: auto; font-size: 11px; max-height: 200px; }}
button {{ background: #333; color: #fff; border: 1px solid #555; padding: 8px 16px; border-radius: 4px; cursor: pointer; }}
button:hover {{ background: #444; }}
</style>
<script>
// Notify opener (web client) that verification is complete
if (window.opener) {{
    window.opener.postMessage({{ type: 'freeq-credential', status: 'verified', credential_type: 'github_membership' }}, '*');
}}
</script>
</head><body>
<h1>✓ Verified</h1>
<p><span class="badge">{username}</span> is a member of <span class="badge">{org}</span></p>
<p>Credential issued for: <code>{did}</code></p>
{callback_status}
<details><summary>Credential JSON</summary>
<pre id="vc">{vc_json}</pre>
<button onclick="navigator.clipboard.writeText(document.getElementById('vc').textContent)">📋 Copy</button>
</details>
</body></html>"#,
        did = pending.subject_did,
    );

    axum::response::Html(html).into_response()
}

fn error_page(msg: &str) -> axum::response::Response {
    let html = format!(
        r#"<!DOCTYPE html><html><head><title>freeq — Error</title>
<style>
body {{ font-family: system-ui; max-width: 500px; margin: 80px auto; text-align: center; background: #0a0a1a; color: #e0e0e0; }}
h1 {{ color: #f44; }}
</style></head><body>
<h1>Verification Failed</h1>
<p>{msg}</p>
</body></html>"#,
    );
    axum::response::Html(html).into_response()
}
