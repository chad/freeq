//! Channel moderation verifier.
//!
//! Allows channel operators to appoint moderators (halfops) via signed
//! credentials. The server maps `channel_moderator` credentials to +h mode.
//!
//! This is the default moderation service shipped with freeq, but any
//! third-party service can issue `channel_moderator` credentials with
//! the same format — the server only checks Ed25519 signatures.
//!
//! Routes:
//!   GET  /verify/mod/start     — Moderator appointment page (for channel ops)
//!   POST /verify/mod/appoint   — Issue a moderator credential
//!   POST /verify/mod/revoke    — Revoke a moderator credential
//!   GET  /verify/mod/roster    — List active moderators for a channel

use super::VerifierState;
use crate::policy::credentials;
use crate::policy::types::VerifiableCredential;
use axum::{
    Json, Router,
    extract::{Query, State},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// In-memory roster of active moderator appointments.
/// In production, this would be backed by a database.
pub struct ModRoster {
    /// channel (lowercase) → vec of active appointments
    pub channels: HashMap<String, Vec<ModAppointment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModAppointment {
    pub subject_did: String,
    pub channel: String,
    pub appointed_by: String, // DID of the channel op who appointed
    pub appointed_at: String, // ISO 8601
    pub expires_at: String,   // ISO 8601
    pub credential_id: String,
    pub revoked: bool,
}

pub fn routes() -> Router<Arc<VerifierState>> {
    Router::new()
        .route("/verify/mod/start", get(start_page))
        .route("/verify/mod/appoint", post(appoint))
        .route("/verify/mod/revoke", post(revoke))
        .route("/verify/mod/roster", get(roster))
}

/// Landing page — shows appointment form for channel ops.
async fn start_page(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let channel = params.get("channel").cloned().unwrap_or_default();
    let callback = params.get("callback").cloned().unwrap_or_default();
    let subject_did = params.get("subject_did").cloned().unwrap_or_default();
    let appointer_did = params.get("appointer_did").cloned().unwrap_or_default();

    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>freeq — Appoint Moderator</title>
<style>
  body {{ background: #0d1117; color: #e6edf3; font-family: -apple-system, BlinkMacSystemFont, sans-serif; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; }}
  .card {{ background: #161b22; border: 1px solid #30363d; border-radius: 16px; padding: 32px; max-width: 480px; width: 90%; }}
  h1 {{ color: #00ffc8; font-size: 20px; margin: 0 0 8px 0; }}
  .subtitle {{ color: #8b949e; font-size: 14px; margin-bottom: 24px; }}
  label {{ display: block; color: #8b949e; font-size: 13px; margin-bottom: 4px; margin-top: 16px; }}
  input {{ width: 100%; padding: 10px 12px; background: #0d1117; border: 1px solid #30363d; border-radius: 8px; color: #e6edf3; font-size: 14px; box-sizing: border-box; }}
  select {{ width: 100%; padding: 10px 12px; background: #0d1117; border: 1px solid #30363d; border-radius: 8px; color: #e6edf3; font-size: 14px; box-sizing: border-box; }}
  .btn {{ display: block; width: 100%; padding: 12px; background: linear-gradient(135deg, #00ffc8, #00b4d8); color: #0d1117; border: none; border-radius: 10px; font-size: 15px; font-weight: 600; cursor: pointer; margin-top: 24px; }}
  .btn:hover {{ opacity: 0.9; }}
  .info {{ background: #1c2128; border-radius: 8px; padding: 12px; margin-top: 16px; font-size: 13px; color: #8b949e; }}
</style>
</head>
<body>
<div class="card">
  <h1>🛡️ Appoint Moderator</h1>
  <p class="subtitle">Issue a moderator credential for {channel}</p>
  <form method="POST" action="/verify/mod/appoint">
    <input type="hidden" name="channel" value="{channel}">
    <input type="hidden" name="callback" value="{callback}">
    <input type="hidden" name="appointer_did" value="{appointer_did}">
    <label>Moderator DID or Handle</label>
    <input name="subject" value="{subject_did}" placeholder="did:plc:... or handle.bsky.social" required>
    <label>Duration</label>
    <select name="duration">
      <option value="7">7 days</option>
      <option value="30" selected>30 days</option>
      <option value="90">90 days</option>
      <option value="365">1 year</option>
    </select>
    <div class="info">
      The moderator will receive <strong>+h (halfop)</strong> status:<br>
      • Can kick and ban regular users<br>
      • Can voice/unvoice users (+v)<br>
      • Cannot kick other moderators or operators<br>
      • Cannot change channel modes (+m, +t, etc.)
    </div>
    <button type="submit" class="btn">Appoint Moderator</button>
  </form>
</div>
</body>
</html>"#
    ))
}

#[derive(Deserialize)]
struct AppointRequest {
    subject: String, // DID or handle
    channel: String,
    appointer_did: String,
    callback: String,
    duration: Option<u64>, // days
}

/// Appoint a moderator — issue a signed credential.
async fn appoint(
    State(state): State<Arc<VerifierState>>,
    axum::extract::Form(req): axum::extract::Form<AppointRequest>,
) -> impl IntoResponse {
    let duration_days = req.duration.unwrap_or(30);
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::days(duration_days as i64);

    // Resolve handle → DID if needed
    let subject_did = if req.subject.starts_with("did:") {
        req.subject.clone()
    } else {
        let http = reqwest::Client::new();
        match resolve_handle_to_did(&http, &req.subject).await {
            Some(did) => did,
            None => {
                return Html(format!(
                    r#"<html><body style="background:#0d1117;color:#e6edf3;display:flex;justify-content:center;align-items:center;height:100vh;font-family:sans-serif">
                    <div style="text-align:center"><h2>❌ Could not resolve handle</h2><p>{} not found</p></div></body></html>"#,
                    req.subject
                ));
            }
        }
    };

    let channel_lower = req.channel.to_lowercase();

    // Build and sign the credential
    let mut credential = VerifiableCredential {
        credential_type_tag: "FreeqCredential/v1".to_string(),
        issuer: state.issuer_did.clone(),
        subject: subject_did.clone(),
        credential_type: "channel_moderator".to_string(),
        claims: serde_json::json!({
            "channel": channel_lower,
            "appointed_by": req.appointer_did,
            "powers": ["kick", "ban", "voice", "mute"],
        }),
        issued_at: now.to_rfc3339(),
        expires_at: Some(expires.to_rfc3339()),
        signature: String::new(),
    };
    let _ = credentials::sign_credential(&mut credential, &state.signing_key);

    let credential_id = format!(
        "mod-{}-{}",
        channel_lower,
        &subject_did[..20.min(subject_did.len())]
    );

    // Store in roster
    {
        let mut roster = state.mod_roster.lock();
        let entries = roster.channels.entry(channel_lower.clone()).or_default();
        // Remove any existing appointment for this DID in this channel
        entries.retain(|a| a.subject_did != subject_did || a.revoked);
        entries.push(ModAppointment {
            subject_did: subject_did.clone(),
            channel: channel_lower.clone(),
            appointed_by: req.appointer_did.clone(),
            appointed_at: now.to_rfc3339(),
            expires_at: expires.to_rfc3339(),
            credential_id: credential_id.clone(),
            revoked: false,
        });
    }

    // If callback is provided, POST the credential there
    if !req.callback.is_empty() {
        let http = reqwest::Client::new();
        let payload = serde_json::json!({ "credential": credential });
        let _ = http
            .post(&req.callback)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;
    }

    // Return success page that posts back to opener
    Html(format!(
        r#"<!DOCTYPE html>
<html><head>
<meta charset="utf-8">
<title>Moderator Appointed</title>
<style>
  body {{ background: #0d1117; color: #e6edf3; font-family: sans-serif; display: flex; justify-content: center; align-items: center; height: 100vh; margin: 0; }}
  .card {{ background: #161b22; border: 1px solid #30363d; border-radius: 16px; padding: 32px; text-align: center; max-width: 400px; }}
  h2 {{ color: #00ffc8; }}
</style>
<script>
  if (window.opener) {{
    window.opener.postMessage({{ type: 'freeq-credential', credential: {credential_json} }}, '*');
    setTimeout(() => window.close(), 1500);
  }}
</script>
</head>
<body>
<div class="card">
  <h2>🛡️ Moderator Appointed</h2>
  <p>{subject_did_short} is now a moderator of {channel}</p>
  <p style="color:#8b949e;font-size:13px">Expires: {expires_str}</p>
  <p style="color:#8b949e;font-size:13px">This window will close automatically.</p>
</div>
</body></html>"#,
        credential_json = serde_json::to_string(&credential).unwrap_or_default(),
        subject_did_short = &subject_did[..30.min(subject_did.len())],
        channel = channel_lower,
        expires_str = expires.format("%Y-%m-%d"),
    ))
}

#[derive(Deserialize)]
struct RevokeRequest {
    subject_did: String,
    channel: String,
}

/// Revoke a moderator credential.
async fn revoke(
    State(state): State<Arc<VerifierState>>,
    Json(req): Json<RevokeRequest>,
) -> impl IntoResponse {
    let channel_lower = req.channel.to_lowercase();
    let mut roster = state.mod_roster.lock();
    if let Some(entries) = roster.channels.get_mut(&channel_lower) {
        for entry in entries.iter_mut() {
            if entry.subject_did == req.subject_did && !entry.revoked {
                entry.revoked = true;
            }
        }
    }
    Json(serde_json::json!({ "ok": true }))
}

#[derive(Deserialize)]
struct RosterQuery {
    channel: String,
}

/// List active moderators for a channel.
async fn roster(
    State(state): State<Arc<VerifierState>>,
    Query(query): Query<RosterQuery>,
) -> impl IntoResponse {
    let channel_lower = query.channel.to_lowercase();
    let roster = state.mod_roster.lock();
    let now = chrono::Utc::now();
    let active: Vec<&ModAppointment> = roster
        .channels
        .get(&channel_lower)
        .map(|entries| {
            entries
                .iter()
                .filter(|a| {
                    !a.revoked
                        && chrono::DateTime::parse_from_rfc3339(&a.expires_at)
                            .map(|exp| exp > now)
                            .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();

    Json(serde_json::json!({
        "channel": channel_lower,
        "moderators": active,
    }))
}

/// Resolve a handle to a DID via the public AT Protocol API.
async fn resolve_handle_to_did(http: &reqwest::Client, handle: &str) -> Option<String> {
    let url = format!(
        "https://public.api.bsky.app/xrpc/com.atproto.identity.resolveHandle?handle={}",
        handle
    );
    let resp = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("did")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
