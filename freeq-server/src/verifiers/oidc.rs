//! OIDC / Google Workspace verifier — proves a user controls an email at an
//! allowed domain (e.g. `@acme.com`), then issues a signed credential that the
//! policy framework uses to gate channel JOIN.
//!
//! This is the "company SSO" path: a firm running Google Workspace (or any
//! OIDC IdP — Okta, Entra ID, Auth0) points a channel policy at this verifier,
//! and only staff whose IdP login resolves to the company domain can join. The
//! resulting `oidc_domain` credential is *also* the signal a group-key steward
//! checks before sealing the channel key to a new member (see
//! `freeq-sdk::e2ee_group`), so SSO admission and E2E key access share one
//! source of truth — no shared passphrase.
//!
//! Routes:
//!   GET /verify/oidc/start?subject_did=...&callback=...
//!     → Redirect to the IdP authorization endpoint (scope: openid email).
//!   GET /verify/oidc/callback
//!     → Exchange code, read the ID token, check the domain, sign + POST the VC.
//!
//! Config (env, read in `verifiers::router`):
//!   OIDC_CLIENT_ID, OIDC_CLIENT_SECRET   — IdP OAuth client
//!   OIDC_ALLOWED_DOMAIN                   — e.g. "acme.com" (required)
//!   OIDC_REDIRECT_URL                     — this verifier's /verify/oidc/callback URL
//!   OIDC_AUTH_URL, OIDC_TOKEN_URL         — default to Google's endpoints
//!
//! SECURITY: the ID token is read directly from the IdP token endpoint over
//! TLS, so its origin is authenticated by the connection. A hardened deployment
//! should additionally verify the ID token's RS256 signature against the IdP's
//! JWKS and validate `aud`/`iss`/`exp`/`nonce`. That is marked TODO below and
//! is required before treating this as more than a reference verifier.

use super::{PendingVerification, VerifierState};
use crate::policy::credentials;
use crate::policy::types::VerifiableCredential;
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;
use std::sync::Arc;

/// IdP + domain configuration for the OIDC verifier.
#[derive(Clone)]
pub struct OidcConfig {
    pub client_id: String,
    pub client_secret: String,
    /// Only users whose verified email is at this domain are issued a credential.
    pub allowed_domain: String,
    /// This verifier's own callback URL, registered with the IdP.
    pub redirect_url: String,
    pub auth_url: String,
    pub token_url: String,
}

impl OidcConfig {
    /// Load from env. Returns None unless client id/secret and allowed domain
    /// are all set. Auth/token URLs default to Google.
    pub fn from_env() -> Option<Self> {
        let client_id = std::env::var("OIDC_CLIENT_ID").ok()?;
        let client_secret = std::env::var("OIDC_CLIENT_SECRET").ok()?;
        let allowed_domain = std::env::var("OIDC_ALLOWED_DOMAIN").ok()?;
        if client_id.is_empty() || client_secret.is_empty() || allowed_domain.is_empty() {
            return None;
        }
        Some(Self {
            client_id,
            client_secret,
            allowed_domain,
            redirect_url: std::env::var("OIDC_REDIRECT_URL").unwrap_or_default(),
            auth_url: std::env::var("OIDC_AUTH_URL")
                .unwrap_or_else(|_| "https://accounts.google.com/o/oauth2/v2/auth".into()),
            token_url: std::env::var("OIDC_TOKEN_URL")
                .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".into()),
        })
    }
}

pub fn routes() -> Router<Arc<VerifierState>> {
    Router::new()
        .route("/verify/oidc/start", get(start))
        .route("/verify/oidc/callback", get(callback))
}

#[derive(Deserialize)]
struct StartQuery {
    /// DID of the user (already proven via AT Protocol auth on the freeq server).
    subject_did: String,
    /// URL to POST the signed credential to after verification.
    #[serde(default)]
    callback: String,
}

async fn start(
    Query(q): Query<StartQuery>,
    State(state): State<Arc<VerifierState>>,
) -> Result<Redirect, (StatusCode, String)> {
    let oidc = state.oidc.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "OIDC verifier not configured".into(),
    ))?;

    let state_token = hex::encode(rand::random::<[u8; 16]>());
    state.pending.lock().insert(
        state_token.clone(),
        PendingVerification {
            subject_did: q.subject_did,
            callback_url: q.callback,
            provider_params: serde_json::Value::Null,
            created_at: std::time::Instant::now(),
        },
    );

    // `hd` hints Google to prefer the company domain; the callback still
    // enforces the domain regardless of this hint.
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope=openid%20email&state={}&hd={}",
        oidc.auth_url,
        urlencoding_encode(&oidc.client_id),
        urlencoding_encode(&oidc.redirect_url),
        state_token,
        urlencoding_encode(&oidc.allowed_domain),
    );
    Ok(Redirect::temporary(&url))
}

async fn callback(
    Query(q): Query<std::collections::HashMap<String, String>>,
    State(state): State<Arc<VerifierState>>,
) -> Response {
    let code = match q.get("code") {
        Some(c) => c.clone(),
        None => return error_page("No authorization code from the identity provider"),
    };
    let oauth_state = match q.get("state") {
        Some(s) => s.clone(),
        None => return error_page("Missing state parameter"),
    };

    let pending = match state.pending.lock().remove(&oauth_state) {
        Some(p) if p.created_at.elapsed() < std::time::Duration::from_secs(300) => p,
        Some(_) => return error_page("Verification expired. Please try again."),
        None => return error_page("Unknown or expired verification"),
    };

    let oidc = match &state.oidc {
        Some(c) => c,
        None => return error_page("OIDC verifier not configured"),
    };

    let http = reqwest::Client::new();
    let token_json: serde_json::Value = match http
        .post(&oidc.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", oidc.client_id.as_str()),
            ("client_secret", oidc.client_secret.as_str()),
            ("code", code.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", oidc.redirect_url.as_str()),
        ])
        .send()
        .await
    {
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => return error_page(&format!("Token exchange failed: {e}")),
    };

    let id_token = match token_json["id_token"].as_str() {
        Some(t) => t.to_string(),
        None => {
            let err = token_json["error_description"]
                .as_str()
                .or(token_json["error"].as_str())
                .unwrap_or("no id_token in response");
            return error_page(&format!("IdP login failed: {err}"));
        }
    };

    // TODO(hardening): verify the RS256 signature against the IdP JWKS and check
    // aud == client_id, iss, exp, and a per-request nonce. The token is received
    // directly from the IdP token endpoint over TLS, so decoding its claims here
    // is sound for a reference verifier, but signature validation is required
    // before this gates anything sensitive.
    let claims = match decode_jwt_claims(&id_token) {
        Some(c) => c,
        None => return error_page("Could not read the ID token claims"),
    };

    let email = claims["email"].as_str().unwrap_or_default().to_lowercase();
    let email_verified = claims["email_verified"].as_bool().unwrap_or(false)
        || claims["email_verified"].as_str() == Some("true");
    // Google sets `hd` (hosted domain) for Workspace accounts; fall back to the
    // email's domain part for generic OIDC IdPs.
    let domain = claims["hd"]
        .as_str()
        .map(str::to_lowercase)
        .unwrap_or_else(|| email.rsplit('@').next().unwrap_or_default().to_string());

    if email.is_empty() || !email_verified {
        return error_page("The identity provider did not return a verified email.");
    }
    if domain != oidc.allowed_domain.to_lowercase() {
        return error_page(&format!(
            "{email} is at '{domain}', not the required domain '{}'.",
            oidc.allowed_domain
        ));
    }

    issue_credential(&state, &http, &pending, &email, &domain).await
}

/// Sign an `oidc_domain` credential and POST it to the callback URL.
async fn issue_credential(
    state: &Arc<VerifierState>,
    http: &reqwest::Client,
    pending: &PendingVerification,
    email: &str,
    domain: &str,
) -> Response {
    let mut vc = VerifiableCredential {
        credential_type_tag: "FreeqCredential/v1".into(),
        issuer: state.issuer_did.clone(),
        subject: pending.subject_did.clone(),
        credential_type: "oidc_domain".into(),
        claims: serde_json::json!({ "email": email, "domain": domain }),
        issued_at: chrono::Utc::now().to_rfc3339(),
        // Short TTL: re-auth through SSO picks up offboarding quickly, so an
        // ex-employee's credential lapses and they miss the next key epoch.
        expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(12)).to_rfc3339()),
        signature: String::new(),
    };
    if let Err(e) = credentials::sign_credential(&mut vc, &state.signing_key) {
        return error_page(&format!("Failed to sign credential: {e}"));
    }

    tracing::info!(
        subject = %pending.subject_did,
        email = %email,
        domain = %domain,
        "OIDC verification complete, credential issued"
    );

    if !pending.callback_url.is_empty() {
        match http
            .post(&pending.callback_url)
            .json(&serde_json::json!({ "credential": vc }))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::warn!(status = %r.status(), "OIDC credential callback failed"),
            Err(e) => tracing::warn!(error = %e, "OIDC credential callback request failed"),
        }
    }

    let safe_email = html_escape(email);
    let safe_domain = html_escape(domain);
    Html(format!(
        r#"<!DOCTYPE html><html><head><title>freeq — Verified</title>
<style>body{{font-family:system-ui;max-width:560px;margin:60px auto;text-align:center;background:#0a0a1a;color:#e0e0e0}}h1{{color:#0f0}}</style>
<script>if(window.opener){{window.opener.postMessage({{type:'freeq-credential',status:'verified',credential_type:'oidc_domain'}},'*');setTimeout(function(){{window.close()}},1500);}}</script>
</head><body><h1>✓ Verified</h1><p>{safe_email} confirmed at <code>{safe_domain}</code>.</p>
<p>You can close this window and return to freeq.</p></body></html>"#
    ))
    .into_response()
}

/// Decode the (unverified) claims payload of a compact JWS/JWT.
fn decode_jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload_b64 = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Minimal percent-encoding for query-string values (avoids a new dependency).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn error_page(msg: &str) -> Response {
    let html = format!(
        r#"<!DOCTYPE html><html><head><title>freeq — Error</title>
<style>body{{font-family:system-ui;max-width:500px;margin:80px auto;text-align:center;background:#0a0a1a;color:#e0e0e0}}h1{{color:#f44}}p{{white-space:pre-wrap;text-align:left}}</style>
</head><body><h1>Verification Failed</h1><p>{}</p></body></html>"#,
        html_escape(msg)
    );
    Html(html).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn jwt_with_payload(payload: serde_json::Value) -> String {
        let b64 = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        let header = b64(br#"{"alg":"RS256","typ":"JWT"}"#);
        let body = b64(payload.to_string().as_bytes());
        format!("{header}.{body}.signature-not-checked-here")
    }

    #[test]
    fn decodes_google_workspace_claims() {
        let jwt = jwt_with_payload(serde_json::json!({
            "email": "jane@acme.com",
            "email_verified": true,
            "hd": "acme.com",
        }));
        let claims = decode_jwt_claims(&jwt).unwrap();
        assert_eq!(claims["email"], "jane@acme.com");
        assert_eq!(claims["hd"], "acme.com");
        assert_eq!(claims["email_verified"], true);
    }

    #[test]
    fn rejects_garbage_token() {
        assert!(decode_jwt_claims("not-a-jwt").is_none());
        assert!(decode_jwt_claims("only.two").is_none());
    }

    #[test]
    fn query_encoding_escapes_reserved() {
        assert_eq!(urlencoding_encode("a b/c?d"), "a%20b%2Fc%3Fd");
        assert_eq!(urlencoding_encode("acme.com"), "acme.com");
    }
}
