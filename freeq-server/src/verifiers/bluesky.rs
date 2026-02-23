//! Bluesky follower gate verifier.
//!
//! Checks the public AT Protocol social graph — no OAuth needed.
//! If the user follows the target handle, issues a signed credential.
//!
//! Routes:
//!   GET /verify/bluesky/start?subject_did=...&target=handle&callback=...
//!     → Check follow via public API, issue credential or show follow prompt
//!   GET /verify/bluesky/check?subject_did=...&target=handle&callback=...
//!     → Re-check (after user has followed)

use super::VerifierState;
use crate::policy::credentials;
use crate::policy::types::VerifiableCredential;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<VerifierState>> {
    Router::new()
        .route("/verify/bluesky/start", get(start))
        .route("/verify/bluesky/check", get(check))
}

#[derive(Deserialize)]
struct StartQuery {
    subject_did: String,
    target: String, // handle to follow (e.g. "chadfowler.com")
    #[serde(default)]
    callback: String,
}

/// Resolve a handle to a DID via the public Bluesky API.
async fn resolve_handle(http: &reqwest::Client, handle: &str) -> Option<String> {
    let url = format!(
        "https://public.api.bsky.app/xrpc/com.atproto.identity.resolveHandle?handle={}",
        handle
    );
    let resp = http.get(&url)
        .header("User-Agent", "freeq-verifier")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json["did"].as_str().map(String::from)
}

/// Check if `actor_did` follows `target_did` using the public API.
/// Walks the follows list (paginated) looking for the target.
async fn check_follows(
    http: &reqwest::Client,
    actor_did: &str,
    target_did: &str,
) -> bool {
    let mut cursor: Option<String> = None;
    // Check up to 10 pages (1000 follows)
    for _ in 0..10 {
        let mut url = format!(
            "https://public.api.bsky.app/xrpc/app.bsky.graph.getFollows?actor={}&limit=100",
            actor_did
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let resp = match http
            .get(&url)
            .header("User-Agent", "freeq-verifier")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => return false,
        };

        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => return false,
        };

        if let Some(follows) = json["follows"].as_array() {
            for follow in follows {
                if follow["did"].as_str() == Some(target_did) {
                    return true;
                }
            }
        }

        cursor = json["cursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    false
}

/// Resolve a DID to a handle via public API.
async fn resolve_did_to_handle(http: &reqwest::Client, did: &str) -> Option<String> {
    let url = format!(
        "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor={}",
        did
    );
    let resp = http.get(&url)
        .header("User-Agent", "freeq-verifier")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json["handle"].as_str().map(String::from)
}

async fn start(
    Query(q): Query<StartQuery>,
    State(state): State<Arc<VerifierState>>,
) -> impl IntoResponse {
    do_check(&q.subject_did, &q.target, &q.callback, &state, true).await
}

async fn check(
    Query(q): Query<StartQuery>,
    State(state): State<Arc<VerifierState>>,
) -> impl IntoResponse {
    do_check(&q.subject_did, &q.target, &q.callback, &state, false).await
}

async fn do_check(
    subject_did: &str,
    target: &str,
    callback: &str,
    state: &Arc<VerifierState>,
    is_initial: bool,
) -> axum::response::Response {
    let http = reqwest::Client::new();

    // Resolve target handle → DID
    let target_handle = target.trim_start_matches('@');
    let target_did = match resolve_handle(&http, target_handle).await {
        Some(d) => d,
        None => return error_page(&format!("Could not resolve @{target_handle} on Bluesky")),
    };

    // Resolve subject DID → handle (for display)
    let subject_handle = resolve_did_to_handle(&http, subject_did)
        .await
        .unwrap_or_else(|| subject_did.to_string());

    tracing::info!(
        subject = %subject_did,
        subject_handle = %subject_handle,
        target = %target_handle,
        "Checking Bluesky follow relationship"
    );

    // Check if subject follows target
    let follows = check_follows(&http, subject_did, &target_did).await;

    if follows {
        // Issue credential
        let mut vc = VerifiableCredential {
            credential_type_tag: "FreeqCredential/v1".into(),
            issuer: state.issuer_did.clone(),
            subject: subject_did.to_string(),
            credential_type: "bluesky_follower".into(),
            claims: serde_json::json!({
                "handle": subject_handle,
                "follows": target_handle,
                "follows_did": target_did,
            }),
            issued_at: chrono::Utc::now().to_rfc3339(),
            expires_at: Some(
                (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339(),
            ),
            signature: String::new(),
        };
        credentials::sign_credential(&mut vc, &state.signing_key).unwrap();

        tracing::info!(
            subject = %subject_did,
            handle = %subject_handle,
            target = %target_handle,
            "Bluesky follow verified, credential issued"
        );

        // POST credential to callback
        let callback_ok = if !callback.is_empty() {
            match http
                .post(callback)
                .json(&serde_json::json!({ "credential": vc }))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => true,
                Ok(r) => {
                    tracing::warn!(status = %r.status(), "Bluesky credential callback failed");
                    false
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Bluesky credential callback request failed");
                    false
                }
            }
        } else {
            false
        };

        let callback_msg = if callback_ok {
            "<p class='success'>✓ Verified! Credential delivered. You can close this window.</p>"
        } else {
            "<p>Credential issued but not auto-delivered.</p>"
        };

        let html = format!(
            r#"<!DOCTYPE html><html><head><title>freeq — Verified</title>
<style>
body {{ font-family: system-ui; max-width: 500px; margin: 40px auto; padding: 0 20px; background: #0a0a1a; color: #e0e0e0; }}
.card {{ background: #1a1a2e; border-radius: 16px; padding: 32px; text-align: center; }}
h1 {{ color: #00d4aa; margin-bottom: 8px; }}
.badge {{ display: inline-flex; align-items: center; gap: 8px; background: #00d4aa22; border: 1px solid #00d4aa44;
          padding: 8px 16px; border-radius: 20px; margin: 16px 0; }}
.badge img {{ width: 24px; height: 24px; border-radius: 12px; }}
.success {{ color: #00d4aa; font-weight: 600; }}
</style>
<script>
if (window.opener) {{
    window.opener.postMessage({{ type: 'freeq-credential', status: 'verified', credential_type: 'bluesky_follower' }}, '*');
}}
</script>
</head><body>
<div class="card">
<h1>✓ Verified</h1>
<p style="color:#999">@{subject_handle} follows @{target_handle}</p>
<div class="badge">🦋 Bluesky Follower</div>
{callback_msg}
</div>
</body></html>"#,
        );

        axum::response::Html(html).into_response()
    } else {
        // Not following — show prompt
        let check_url = format!(
            "/verify/bluesky/check?subject_did={}&target={}&callback={}",
            urlencoding::encode(subject_did),
            urlencoding::encode(target_handle),
            urlencoding::encode(callback),
        );

        let html = format!(
            r#"<!DOCTYPE html><html><head><title>freeq — Follow Required</title>
<style>
body {{ font-family: system-ui; max-width: 500px; margin: 40px auto; padding: 0 20px; background: #0a0a1a; color: #e0e0e0; }}
.card {{ background: #1a1a2e; border-radius: 16px; padding: 32px; text-align: center; }}
h1 {{ color: #fff; margin-bottom: 8px; font-size: 22px; }}
.sub {{ color: #999; margin-bottom: 24px; }}
.target {{ display: inline-flex; align-items: center; gap: 8px; background: #1185fe22; border: 1px solid #1185fe44;
           padding: 12px 20px; border-radius: 12px; margin: 16px 0; font-size: 18px; color: #1185fe; font-weight: 600;
           text-decoration: none; }}
.target:hover {{ background: #1185fe33; }}
.recheck {{ display: inline-block; margin-top: 20px; background: #00d4aa; color: #000; font-weight: 700;
            padding: 12px 32px; border-radius: 10px; text-decoration: none; font-size: 16px; }}
.recheck:hover {{ background: #00e4ba; }}
.hint {{ color: #666; font-size: 13px; margin-top: 16px; }}
</style></head><body>
<div class="card">
<h1>Follow Required</h1>
<p class="sub">This channel requires you to follow a Bluesky account</p>
<a href="https://bsky.app/profile/{target_handle}" target="_blank" class="target">
🦋 @{target_handle}
</a>
<br>
<a href="{check_url}" class="recheck">I followed — check again</a>
<p class="hint">Follow @{target_handle} on Bluesky, then click the button above.</p>
</div>
</body></html>"#,
        );

        axum::response::Html(html).into_response()
    }
}

fn error_page(msg: &str) -> axum::response::Response {
    let html = format!(
        r#"<!DOCTYPE html><html><head><title>freeq — Error</title>
<style>
body {{ font-family: system-ui; max-width: 500px; margin: 80px auto; text-align: center; background: #0a0a1a; color: #e0e0e0; }}
h1 {{ color: #f44; }}
</style></head><body>
<h1>Error</h1>
<p>{msg}</p>
</body></html>"#,
    );
    axum::response::Html(html).into_response()
}
