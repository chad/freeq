//! JCS (RFC 8785) canonicalization and SHA-256 hashing.
//!
//! All policy objects are canonicalized before hashing or signing.
//!
//! Canonicalization itself lives in `freeq_sdk::canonical` so freeq-bot-id and
//! external consumers share one implementation. Sigs minted on one side MUST
//! verify against the same canonical form on the other.

pub use freeq_sdk::canonical::canonicalize;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// SHA-256 hash of the JCS-canonicalized representation (hex-encoded).
pub fn hash_canonical<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let canonical = canonicalize(value)?;
    Ok(sha256_hex(canonical.as_bytes()))
}

/// Raw SHA-256 hash (hex-encoded).
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// HMAC-SHA256 sign the JCS-canonicalized representation of a value.
/// Returns hex-encoded signature.
pub fn hmac_sign<T: Serialize>(value: &T, key: &[u8]) -> Result<String, serde_json::Error> {
    use hmac::{Hmac, Mac};
    let canonical = canonicalize(value)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key length is always valid");
    mac.update(canonical.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verify an HMAC-SHA256 signature over the JCS-canonicalized representation.
pub fn hmac_verify<T: Serialize>(
    value: &T,
    key: &[u8],
    signature: &str,
) -> Result<bool, serde_json::Error> {
    let expected = hmac_sign(value, key)?;
    // Constant-time comparison
    Ok(expected == signature)
}

#[cfg(test)]
mod tests {
    // Canonicalization-only tests live in freeq_sdk::canonical. These cover the
    // server-specific wrappers (hash_canonical, hmac_sign/verify).
    use super::*;
    use serde_json::json;

    #[test]
    fn test_hash_deterministic() {
        let v = json!({"channel": "#test", "version": 1});
        let h1 = hash_canonical(&v).unwrap();
        let h2 = hash_canonical(&v).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // 32 bytes hex
    }

    #[test]
    fn test_hmac_sign_verify() {
        let key = b"test-signing-key-32bytes!!!!!!!!";
        let v = json!({"channel": "#test", "user": "did:plc:abc"});
        let sig = hmac_sign(&v, key).unwrap();
        assert!(!sig.is_empty());
        assert!(hmac_verify(&v, key, &sig).unwrap());
        // Wrong key fails
        assert!(!hmac_verify(&v, b"wrong-key-32bytes!!!!!!!!!!!!!!!!", &sig).unwrap());
        // Tampered data fails
        let v2 = json!({"channel": "#test", "user": "did:plc:xyz"});
        assert!(!hmac_verify(&v2, key, &sig).unwrap());
    }
}
