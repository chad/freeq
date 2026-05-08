//! JCS (RFC 8785) canonicalization.
//!
//! Use this for any value that needs a deterministic byte representation —
//! typically before hashing or signing. Object keys are sorted lexicographically,
//! whitespace is stripped, primitives go through `serde_json`'s standard
//! serialization (which handles JSON-string escaping rules).
//!
//! Lives in `freeq-sdk` so `freeq-bot-id`, `freeq-server` (policy + provenance
//! verification), and external consumers all share one implementation. Sigs minted
//! by one component MUST verify against the same canonical form on the other.

use serde::Serialize;

/// JCS-canonicalize an arbitrary serializable value.
///
/// Returns the canonical form as a `String` (UTF-8). The canonical bytes are
/// `output.as_bytes()` — feed those into your hash or signature function.
///
/// Implementation: serialize through `serde_json::Value` to normalize the type,
/// then walk the value emitting sorted-key objects and arrays. Primitives use
/// `serde_json`'s default serialization, which handles numbers, strings, booleans
/// and null per the JSON spec.
pub fn canonicalize<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let v = serde_json::to_value(value)?;
    canonicalize_value(&v)
}

fn canonicalize_value(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    match value {
        serde_json::Value::Object(map) => {
            // JCS: keys sorted lexicographically (codepoint order).
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys() {
        let v = json!({"b": 1, "a": 2});
        assert_eq!(canonicalize(&v).unwrap(), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn nested_objects_sorted() {
        let v = json!({"z": {"b": 1, "a": 2}, "a": []});
        assert_eq!(canonicalize(&v).unwrap(), r#"{"a":[],"z":{"a":2,"b":1}}"#);
    }

    #[test]
    fn array_order_preserved() {
        let v = json!([3, 1, 2]);
        assert_eq!(canonicalize(&v).unwrap(), "[3,1,2]");
    }

    #[test]
    fn strings_escape_correctly() {
        let v = json!({"msg": "hello \"world\""});
        assert_eq!(canonicalize(&v).unwrap(), r#"{"msg":"hello \"world\""}"#);
    }

    #[test]
    fn primitive_passthrough() {
        assert_eq!(canonicalize(&json!(42)).unwrap(), "42");
        assert_eq!(canonicalize(&json!("x")).unwrap(), r#""x""#);
        assert_eq!(canonicalize(&json!(null)).unwrap(), "null");
        assert_eq!(canonicalize(&json!(true)).unwrap(), "true");
    }
}
