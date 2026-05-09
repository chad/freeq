//! Provenance declaration verification.
//!
//! Today this only handles `FreeqBotDelegation/v1` certs. Other provenance
//! shapes (free-form JSON metadata) flow through unverified.
//!
//! The verifier matches the canonical form used by `freeq-bot-id` (see S3):
//! the cert is JCS-canonicalized with the `signature` field removed, then
//! the bytes are checked against an ed25519 signature using the creator's
//! registered MSGSIG public key (looked up via `db.get_signing_key`).
//!
//! Verification is fully synchronous — no DID resolution, no network I/O,
//! no async — so it runs inside the IRC command handler without blocking.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;

use crate::db::Db;

/// Outcome of a verification attempt.
#[derive(Debug, Clone)]
pub(super) struct VerificationOutcome {
    /// True only if the signature checked against a registered creator key.
    pub verified: bool,
    /// One-line reason. Always populated; useful for logs and the IRC NOTICE.
    pub reason: String,
    /// DID whose registered key signed off. Only set on successful verify.
    pub verifier_key_did: Option<String>,
}

impl VerificationOutcome {
    fn ok(creator_did: String) -> Self {
        Self {
            verified: true,
            reason: format!("Verified against creator key for {creator_did}"),
            verifier_key_did: Some(creator_did),
        }
    }
    fn unverified(reason: impl Into<String>) -> Self {
        Self {
            verified: false,
            reason: reason.into(),
            verifier_key_did: None,
        }
    }
}

/// If `json` is a `FreeqBotDelegation/v1` cert, attempt to verify it.
/// For any other shape, return an `unverified` outcome (caller can still store).
///
/// Returns `Err` only when `submitter_did` and `cert.bot_did` disagree —
/// that's a hard reject (someone is trying to register a cert that doesn't
/// belong to their own session).
pub(super) fn verify_provenance(
    json: &Value,
    submitter_did: &str,
    db: Option<&Db>,
) -> Result<VerificationOutcome, String> {
    // Free-form provenance (anything that's not FreeqBotDelegation/v1) is
    // accepted as unverified — preserves the v0 behavior for other shapes.
    let type_tag = json.get("type").and_then(|v| v.as_str());
    if type_tag != Some("FreeqBotDelegation/v1") {
        return Ok(VerificationOutcome::unverified(
            "Not a FreeqBotDelegation/v1 cert; stored as-is",
        ));
    }

    // Sanity: cert.bot_did MUST match the submitter (the SASL-authenticated
    // session). Mismatch means the wrong agent is presenting this cert.
    let bot_did = json.get("bot_did").and_then(|v| v.as_str()).unwrap_or("");
    if bot_did.is_empty() {
        return Ok(VerificationOutcome::unverified("Cert is missing bot_did"));
    }
    if bot_did != submitter_did {
        return Err(format!(
            "Cert bot_did ({bot_did}) does not match the authenticated session DID ({submitter_did})"
        ));
    }

    // Required fields for verification
    let creator_did = match json.get("creator_did").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(VerificationOutcome::unverified("Cert is missing creator_did")),
    };
    let sig_b64 = match json.get("signature").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(VerificationOutcome::unverified(
            "Cert has no signature; declarative only",
        )),
    };

    // Look up the creator's registered ed25519 signing key
    let Some(db) = db else {
        return Ok(VerificationOutcome::unverified(
            "Server has no DB; cannot look up creator key",
        ));
    };
    let pubkey_bytes = match db.get_signing_key(&creator_did) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Ok(VerificationOutcome::unverified(format!(
                "No registered MSGSIG key for {creator_did}; creator must register one before signing"
            )));
        }
        Err(e) => {
            return Ok(VerificationOutcome::unverified(format!(
                "DB error looking up signing key: {e}"
            )));
        }
    };

    let vk = match VerifyingKey::from_bytes(&pubkey_bytes) {
        Ok(k) => k,
        Err(e) => {
            return Ok(VerificationOutcome::unverified(format!(
                "Stored creator key is malformed: {e}"
            )));
        }
    };

    // Build the canonical form: cert with the `signature` field removed.
    // This mirrors freeq-bot-id main.rs at sign-time, where `signature` is
    // `skip_serializing_if = Option::is_none` and is None when canonicalizing.
    let mut canonical_json = json.clone();
    if let Some(obj) = canonical_json.as_object_mut() {
        obj.remove("signature");
    }
    let canonical_bytes = match freeq_sdk::canonical::canonicalize(&canonical_json) {
        Ok(s) => s,
        Err(e) => {
            return Ok(VerificationOutcome::unverified(format!(
                "Failed to canonicalize cert for verification: {e}"
            )));
        }
    };

    let sig_bytes = match URL_SAFE_NO_PAD.decode(sig_b64.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return Ok(VerificationOutcome::unverified(format!(
                "Signature is not valid base64url: {e}"
            )));
        }
    };
    if sig_bytes.len() != 64 {
        return Ok(VerificationOutcome::unverified(format!(
            "Signature has wrong length: expected 64, got {}",
            sig_bytes.len()
        )));
    }
    let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().unwrap();
    let sig = Signature::from_bytes(&sig_arr);

    if vk.verify(canonical_bytes.as_bytes(), &sig).is_ok() {
        Ok(VerificationOutcome::ok(creator_did))
    } else {
        Ok(VerificationOutcome::unverified(
            "Signature did not verify against creator's registered key",
        ))
    }
}

/// Annotate the provenance JSON with server-side verification metadata.
/// Mutates in place; reads stay backwards-compatible (extra fields only).
pub(super) fn annotate(json: &mut Value, outcome: &VerificationOutcome) {
    let Some(obj) = json.as_object_mut() else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    obj.insert("_verified".to_string(), Value::Bool(outcome.verified));
    obj.insert(
        "_verification_reason".to_string(),
        Value::String(outcome.reason.clone()),
    );
    if outcome.verified {
        obj.insert("_verified_at".to_string(), Value::Number(now.into()));
    }
    if let Some(ref vk_did) = outcome.verifier_key_did {
        obj.insert(
            "_verifier_key_did".to_string(),
            Value::String(vk_did.clone()),
        );
    }
}
