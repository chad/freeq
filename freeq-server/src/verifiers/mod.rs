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
pub mod moderation;

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
    pub pending: parking_lot::Mutex<std::collections::HashMap<String, PendingVerification>>,
    /// Moderator roster: channel → active appointments.
    pub mod_roster: parking_lot::Mutex<moderation::ModRoster>,
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

/// Load or generate a persistent signing key from the given path.
fn load_or_generate_signing_key(path: &std::path::Path) -> SigningKey {
    if path.exists() {
        if let Ok(data) = std::fs::read(path)
            && let Ok(bytes) = <[u8; 32]>::try_from(data.as_slice())
        {
            let key = SigningKey::from_bytes(&bytes);
            tracing::info!(
                "Loaded existing verifier signing key from {}",
                path.display()
            );
            return key;
        }
        tracing::warn!("Corrupt signing key at {}, regenerating", path.display());
    }
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    if let Err(e) = std::fs::write(path, key.to_bytes()) {
        tracing::error!("Failed to persist signing key to {}: {}", path.display(), e);
    } else {
        tracing::info!(
            "Generated and persisted new verifier signing key to {}",
            path.display()
        );
    }
    key
}

/// Build the verifier router. Returns None if no verifiers are configured.
pub fn router(
    issuer_did: String,
    github: Option<GitHubConfig>,
    data_dir: &std::path::Path,
) -> Option<(Router<()>, Arc<VerifierState>)> {
    let key_path = data_dir.join("verifier-signing-key.secret");
    let signing_key = load_or_generate_signing_key(&key_path);
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
        pending: parking_lot::Mutex::new(std::collections::HashMap::new()),
        mod_roster: parking_lot::Mutex::new(moderation::ModRoster {
            channels: std::collections::HashMap::new(),
        }),
    });

    let mut app = Router::new()
        // DID document — any client can resolve this to get our public key
        // Serve at both .well-known path and did:web spec path (/verify/did.json)
        .route(
            "/verify/.well-known/did.json",
            axum::routing::get(did_document),
        )
        .route("/verify/did.json", axum::routing::get(did_document));

    // Bluesky follower verifier — always available (uses public API, no config needed)
    app = app.merge(bluesky::routes());

    // Moderation verifier — always available
    app = app.merge(moderation::routes());

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
