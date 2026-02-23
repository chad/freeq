//! Verifiable credential verification.
//!
//! Verifies externally-issued credentials by:
//! 1. Resolving the issuer's DID document
//! 2. Extracting the Ed25519 public key
//! 3. Verifying the signature over JCS-canonical payload
//!
//! This decouples credential issuance from the freeq server.
//! Any service can issue credentials — the server just verifies signatures.

use super::canonical;
use super::types::VerifiableCredential;
use ed25519_dalek::{Signature, VerifyingKey, Verifier};

/// Verify a credential's signature against the issuer's public key.
///
/// `issuer_public_key` is the raw 32-byte Ed25519 public key.
pub fn verify_credential_signature(
    credential: &VerifiableCredential,
    issuer_public_key: &[u8; 32],
) -> Result<bool, String> {
    let verifying_key = VerifyingKey::from_bytes(issuer_public_key)
        .map_err(|e| format!("Invalid public key: {e}"))?;

    // Canonicalize with empty signature (as it was signed)
    let mut unsigned = credential.clone();
    unsigned.signature = String::new();
    let canonical = canonical::canonicalize(&unsigned)
        .map_err(|e| format!("Canonicalization failed: {e}"))?;

    // Decode signature from base64url
    let sig_bytes = base64_url_decode(&credential.signature)
        .map_err(|e| format!("Invalid signature encoding: {e}"))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| format!("Invalid signature format: {e}"))?;

    Ok(verifying_key.verify(canonical.as_bytes(), &signature).is_ok())
}

/// Verify a credential end-to-end:
/// 1. Check type tag
/// 2. Check expiry
/// 3. Check subject matches claimed DID
/// 4. Verify signature
pub fn verify_credential(
    credential: &VerifiableCredential,
    subject_did: &str,
    issuer_public_key: &[u8; 32],
) -> Result<(), String> {
    // Check type tag
    if credential.credential_type_tag != "FreeqCredential/v1" {
        return Err(format!("Unknown credential type: {}", credential.credential_type_tag));
    }

    // Check expiry
    if credential.is_expired() {
        return Err("Credential has expired".into());
    }

    // Check subject
    if credential.subject != subject_did {
        return Err(format!(
            "Credential subject {} does not match claimed DID {}",
            credential.subject, subject_did
        ));
    }

    // Verify signature
    if !verify_credential_signature(credential, issuer_public_key)? {
        return Err("Signature verification failed".into());
    }

    Ok(())
}

/// Sign a credential with an Ed25519 signing key.
/// Sets the signature field to the base64url-encoded Ed25519 signature.
pub fn sign_credential(
    credential: &mut VerifiableCredential,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<(), String> {
    use ed25519_dalek::Signer;

    credential.signature = String::new();
    let canonical = canonical::canonicalize(credential)
        .map_err(|e| format!("Canonicalization failed: {e}"))?;
    let signature = signing_key.sign(canonical.as_bytes());
    credential.signature = base64_url_encode(&signature.to_bytes());
    Ok(())
}

fn base64_url_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| e.to_string())
}

fn base64_url_encode(input: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn make_test_credential(
        issuer: &str,
        subject: &str,
        cred_type: &str,
        signing_key: &SigningKey,
    ) -> VerifiableCredential {
        let mut vc = VerifiableCredential {
            credential_type_tag: "FreeqCredential/v1".into(),
            issuer: issuer.into(),
            subject: subject.into(),
            credential_type: cred_type.into(),
            claims: serde_json::json!({
                "github_username": "octocat",
                "org": "freeq",
            }),
            issued_at: chrono::Utc::now().to_rfc3339(),
            expires_at: Some((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()),
            signature: String::new(),
        };
        sign_credential(&mut vc, signing_key).unwrap();
        vc
    }

    #[test]
    fn test_sign_and_verify() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key = signing_key.verifying_key();

        let vc = make_test_credential(
            "did:web:verify.example.com",
            "did:plc:user1",
            "github_membership",
            &signing_key,
        );

        assert!(!vc.signature.is_empty());
        assert!(verify_credential_signature(&vc, public_key.as_bytes()).unwrap());
    }

    #[test]
    fn test_tampered_credential_fails() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key = signing_key.verifying_key();

        let mut vc = make_test_credential(
            "did:web:verify.example.com",
            "did:plc:user1",
            "github_membership",
            &signing_key,
        );

        // Tamper with claims
        vc.claims = serde_json::json!({"org": "evil"});
        assert!(!verify_credential_signature(&vc, public_key.as_bytes()).unwrap());
    }

    #[test]
    fn test_wrong_key_fails() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng);

        let vc = make_test_credential(
            "did:web:verify.example.com",
            "did:plc:user1",
            "github_membership",
            &signing_key,
        );

        assert!(!verify_credential_signature(&vc, wrong_key.verifying_key().as_bytes()).unwrap());
    }

    #[test]
    fn test_full_verification() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key = signing_key.verifying_key();

        let vc = make_test_credential(
            "did:web:verify.example.com",
            "did:plc:user1",
            "github_membership",
            &signing_key,
        );

        // Correct subject
        assert!(verify_credential(&vc, "did:plc:user1", public_key.as_bytes()).is_ok());

        // Wrong subject
        assert!(verify_credential(&vc, "did:plc:attacker", public_key.as_bytes()).is_err());
    }

    #[test]
    fn test_expired_credential() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key = signing_key.verifying_key();

        let mut vc = VerifiableCredential {
            credential_type_tag: "FreeqCredential/v1".into(),
            issuer: "did:web:verify.example.com".into(),
            subject: "did:plc:user1".into(),
            credential_type: "github_membership".into(),
            claims: serde_json::json!({}),
            issued_at: "2020-01-01T00:00:00Z".into(),
            expires_at: Some("2020-01-02T00:00:00Z".into()),
            signature: String::new(),
        };
        sign_credential(&mut vc, &signing_key).unwrap();

        let result = verify_credential(&vc, "did:plc:user1", public_key.as_bytes());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expired"));
    }
}
