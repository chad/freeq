//! Credential verifiers — architecturally separate from the core protocol.
//!
//! Each verifier is a self-contained module that:
//! 1. Has its own OAuth/API credentials (from env vars)
//! 2. Serves routes under /verify/{provider}/
//! 3. Issues signed VerifiableCredentials
//! 4. POSTs credentials back to a callback URL
//!
//! The freeq protocol knows nothing about these providers.
//! Policies reference verifiers by issuer DID and endpoint URL.
//! Verifiers could run on a completely separate server — they're
//! colocated here for convenience, not coupling.

pub mod bluesky;
pub mod github;

use axum::Router;
use ed25519_dalek::SigningKey;
use std::sync::Arc;

/// Shared state for all verifiers.
pub struct VerifierState {
    /// Ed25519 signing key for issuing credentials.
    pub signing_key: SigningKey,
    /// DID for this verifier instance.
    pub issuer_did: String,
    /// GitHub OAuth credentials (if configured).
    pub github: Option<GitHubConfig>,
    /// Pending verification flows: state_token → PendingVerification.
    pub pending: std::sync::Mutex<std::collections::HashMap<String, PendingVerification>>,
}

#[derive(Clone)]
pub struct GitHubConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone)]
pub struct PendingVerification {
    pub subject_did: String,
    pub callback_url: String,
    pub provider_params: serde_json::Value,
    pub created_at: std::time::Instant,
}

/// Build the verifier router. Returns None if no verifiers are configured.
pub fn router(
    issuer_did: String,
    github: Option<GitHubConfig>,
) -> Option<(Router<()>, Arc<VerifierState>)> {
    let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
    let public_key = signing_key.verifying_key();
    let public_key_multibase = format!(
        "z{}",
        bs58::encode([&[0xed, 0x01], public_key.as_bytes().as_slice()].concat()).into_string()
    );

    tracing::info!(
        "Credential verifier initialized: did={}, pubkey={}",
        issuer_did,
        public_key_multibase
    );

    let state = Arc::new(VerifierState {
        signing_key,
        issuer_did: issuer_did.clone(),
        github,
        pending: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let mut app = Router::new()
        // DID document — any client can resolve this to get our public key
        .route(
            "/verify/.well-known/did.json",
            axum::routing::get(did_document),
        );

    // Bluesky follower verifier — always available (uses public API, no config needed)
    app = app.merge(bluesky::routes());

    // GitHub verifier — only if OAuth credentials are configured
    if state.github.is_some() {
        app = app.merge(github::routes());
    }

    let app = app.with_state(Arc::clone(&state));

    Some((app, state))
}

/// Serve the verifier's DID document with Ed25519 public key.
async fn did_document(
    axum::extract::State(state): axum::extract::State<Arc<VerifierState>>,
) -> impl axum::response::IntoResponse {
    let public_key = state.signing_key.verifying_key();
    let public_key_multibase = format!(
        "z{}",
        bs58::encode([&[0xed, 0x01], public_key.as_bytes().as_slice()].concat()).into_string()
    );
    let key_id = format!("{}#key-1", state.issuer_did);

    axum::Json(serde_json::json!({
        "@context": [
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/multikey/v1"
        ],
        "id": state.issuer_did,
        "verificationMethod": [{
            "id": key_id,
            "type": "Multikey",
            "controller": state.issuer_did,
            "publicKeyMultibase": public_key_multibase,
        }],
        "assertionMethod": [key_id],
        "authentication": [key_id],
    }))
}
