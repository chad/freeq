//! JCS (RFC 8785) canonicalization and SHA-256 hashing.
//!
//! All policy objects are canonicalized before hashing or signing.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Canonicalize a value using JCS (RFC 8785).
///
/// JCS specifies:
/// - Object keys sorted lexicographically
/// - No whitespace
/// - Numbers serialized without trailing zeros
/// - Unicode escaping rules
///
/// We use serde_json's compact serialization with sorted keys via BTreeMap
/// in the types (serde_json serializes in insertion order, BTreeMap gives sorted).
/// For top-level objects, we round-trip through serde_json::Value to ensure
/// key sorting at all levels.
pub fn canonicalize<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    // Serialize to Value first to normalize
    let v = serde_json::to_value(value)?;
    // Then serialize the Value — serde_json serializes object keys in order,
    // and Value uses BTreeMap internally, giving us sorted keys.
    canonicalize_value(&v)
}

/// Canonicalize a serde_json::Value.
fn canonicalize_value(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    match value {
        serde_json::Value::Object(map) => {
            // JCS: keys sorted lexicographically
            let mut pairs: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            pairs.sort_by_key(|(k, _)| *k);

            let mut result = String::from("{");
            for (i, (k, v)) in pairs.iter().enumerate() {
                if i > 0 {
                    result.push(',');
                }
                // Key is JSON-escaped string
                result.push_str(&serde_json::to_string(k)?);
                result.push(':');
                result.push_str(&canonicalize_value(v)?);
            }
            result.push('}');
            Ok(result)
        }
        serde_json::Value::Array(arr) => {
            let mut result = String::from("[");
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    result.push(',');
                }
                result.push_str(&canonicalize_value(v)?);
            }
            result.push(']');
            Ok(result)
        }
        // Primitives: use serde_json's serialization (handles numbers, strings, bools, null)
        _ => serde_json::to_string(value),
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_canonicalize_sorts_keys() {
        let v = json!({"b": 1, "a": 2});
        let c = canonicalize_value(&v).unwrap();
        assert_eq!(c, r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn test_canonicalize_nested() {
        let v = json!({"z": {"b": 1, "a": 2}, "a": []});
        let c = canonicalize_value(&v).unwrap();
        assert_eq!(c, r#"{"a":[],"z":{"a":2,"b":1}}"#);
    }

    #[test]
    fn test_hash_deterministic() {
        let v = json!({"channel": "#test", "version": 1});
        let h1 = hash_canonical(&v).unwrap();
        let h2 = hash_canonical(&v).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // 32 bytes hex
    }

    #[test]
    fn test_canonicalize_strings() {
        let v = json!({"msg": "hello \"world\""});
        let c = canonicalize_value(&v).unwrap();
        assert_eq!(c, r#"{"msg":"hello \"world\""}"#);
    }

    #[test]
    fn test_canonicalize_array() {
        let v = json!([3, 1, 2]);
        let c = canonicalize_value(&v).unwrap();
        assert_eq!(c, "[3,1,2]");
    }
}
