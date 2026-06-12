//! Adversarial tests for the auth broker ↔ server interface.
//!
//! Tests the HMAC signature verification, timestamp replay protection,
//! web-token minting/consumption lifecycle, and session push security.
//! These target the critical path through which every web user authenticates.

use std::collections::HashMap;

use freeq_sdk::did::DidResolver;

const BROKER_SECRET: &str = "test-broker-secret-key-for-adversarial-testing";

/// Start a test server with broker secret configured.
async fn start() -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let resolver = DidResolver::static_map(HashMap::new());
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-broker".to_string(),
        challenge_timeout_secs: 60,
        broker_shared_secret: Some(BROKER_SECRET.to_string()),
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    server.start_with_web().await.unwrap()
}

/// Compute valid HMAC for a request body with current timestamp.
fn sign_request(body: &[u8]) -> (String, String) {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(BROKER_SECRET.as_bytes()).unwrap();
    mac.update(format!("ts={ts}\n").as_bytes());
    mac.update(body);
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    (sig, ts)
}

/// Compute HMAC with a specific timestamp (for testing expired/future).
fn sign_request_at(body: &[u8], ts: u64) -> (String, String) {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let ts_str = ts.to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(BROKER_SECRET.as_bytes()).unwrap();
    mac.update(format!("ts={ts_str}\n").as_bytes());
    mac.update(body);
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    (sig, ts_str)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ═══════════════════════════════════════════════════════════════
// HMAC VERIFICATION
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn valid_signature_accepted() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:test", "handle": "test.bsky"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "Valid signature should be accepted: {}",
        resp.status()
    );
}

#[tokio::test]
async fn missing_signature_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn missing_timestamp_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    // Sign but don't send timestamp header
    let (sig, _ts) = sign_request(&body_bytes);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "Missing timestamp must be rejected");
}

#[tokio::test]
async fn expired_timestamp_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    // Sign with timestamp 120 seconds ago (>60s skew)
    let (sig, ts) = sign_request_at(&body_bytes, now_secs() - 120);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "Expired timestamp must be rejected");
}

#[tokio::test]
async fn future_timestamp_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    // Sign with timestamp 120 seconds in the future
    let (sig, ts) = sign_request_at(&body_bytes, now_secs() + 120);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "Future timestamp must be rejected");
}

#[tokio::test]
async fn wrong_secret_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    // Sign with wrong key
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let ts = now_secs().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"wrong-secret").unwrap();
    mac.update(format!("ts={ts}\n").as_bytes());
    mac.update(&body_bytes);
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "Wrong secret must be rejected");
}

#[tokio::test]
async fn tampered_body_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:real", "handle": "real"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);

    // Tamper with body after signing
    let tampered = serde_json::json!({"did": "did:plc:attacker", "handle": "attacker"});
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&tampered).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "Tampered body must be rejected");
}

#[tokio::test]
async fn timestamp_not_in_mac_is_different_from_header() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    // Sign with one timestamp but send a different one in the header
    let (sig, _) = sign_request_at(&body_bytes, now_secs());
    let different_ts = (now_secs() + 1).to_string();
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &different_ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "Timestamp mismatch between MAC and header must be rejected"
    );
}

#[tokio::test]
async fn invalid_timestamp_format_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, _) = sign_request(&body_bytes);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", "not-a-number")
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "Non-numeric timestamp must be rejected");
}

#[tokio::test]
async fn empty_body_rejected() {
    let (_irc, http, _h) = start().await;
    let (sig, ts) = sign_request(b"");
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(b"".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "Empty body should be rejected as bad JSON"
    );
}

// ═══════════════════════════════════════════════════════════════
// WEB-TOKEN LIFECYCLE
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn web_token_minted_and_returned() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:mint", "handle": "mint.bsky"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(
        json["token"].as_str().is_some_and(|t| !t.is_empty()),
        "Token should be non-empty"
    );
    assert_eq!(json["did"].as_str(), Some("did:plc:mint"));
    assert_eq!(json["handle"].as_str(), Some("mint.bsky"));
}

#[tokio::test]
async fn different_mints_produce_different_tokens() {
    let (_irc, http, _h) = start().await;
    let mut tokens = Vec::new();
    for i in 0..3 {
        let body = serde_json::json!({"did": format!("did:plc:t{i}"), "handle": format!("t{i}")});
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let (sig, ts) = sign_request(&body_bytes);
        let resp = reqwest::Client::new()
            .post(format!("http://{http}/auth/broker/web-token"))
            .header("X-Broker-Signature", &sig)
            .header("X-Broker-Timestamp", &ts)
            .header("Content-Type", "application/json")
            .body(body_bytes)
            .send()
            .await
            .unwrap();
        let json: serde_json::Value = resp.json().await.unwrap();
        tokens.push(json["token"].as_str().unwrap().to_string());
    }
    // All tokens should be unique
    let unique: std::collections::HashSet<_> = tokens.iter().collect();
    assert_eq!(unique.len(), 3, "Each mint should produce a unique token");
}

// ═══════════════════════════════════════════════════════════════
// SESSION PUSH
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn session_push_accepted() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({
        "did": "did:plc:sess",
        "handle": "sess.bsky",
        "pds_url": "https://pds.example.com",
        "access_token": "test-token",
        "dpop_key_b64": "dGVzdA",
        "dpop_nonce": null,
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/session"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "Valid session push should succeed: {}",
        resp.status()
    );
}

#[tokio::test]
async fn session_push_without_signature_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({
        "did": "did:plc:x", "handle": "x",
        "pds_url": "https://x", "access_token": "x",
        "dpop_key_b64": "x", "dpop_nonce": null,
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/session"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// ═══════════════════════════════════════════════════════════════
// NO BROKER SECRET CONFIGURED
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn no_secret_configured_rejects_all() {
    // Start server WITHOUT broker secret
    let resolver = DidResolver::static_map(HashMap::new());
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-nosecret".to_string(),
        challenge_timeout_secs: 60,
        broker_shared_secret: None, // No secret!
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    let (_irc, http, _h) = server.start_with_web().await.unwrap();

    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        403,
        "No secret configured should return 403 Forbidden"
    );
}

// ═══════════════════════════════════════════════════════════════
// EDGE CASES
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn replay_same_request_within_window() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:replay", "handle": "replay"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);

    // First request — should succeed
    let resp1 = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes.clone())
        .send()
        .await
        .unwrap();
    assert!(resp1.status().is_success());

    // Replay — same sig, same ts, same body
    // This should succeed too (no nonce protection yet — just timestamp window)
    let resp2 = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    // Document: currently there's no nonce-based replay protection,
    // only the 60-second timestamp window
    let _ = resp2.status();
}

#[tokio::test]
async fn timestamp_at_exactly_60_seconds() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:edge", "handle": "edge"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    // Exactly 60 seconds ago
    let (sig, ts) = sign_request_at(&body_bytes, now_secs() - 60);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    // abs_diff(now, now-60) = 60, check is > 60, so 60 should PASS (boundary)
    // This documents the boundary behavior
    let status = resp.status();
    eprintln!("Timestamp at exactly 60s: {status}");
}

#[tokio::test]
async fn timestamp_at_61_seconds_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"did": "did:plc:x", "handle": "x"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request_at(&body_bytes, now_secs() - 61);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "61-second-old timestamp must be rejected"
    );
}

#[tokio::test]
async fn malformed_json_body_rejected() {
    let (_irc, http, _h) = start().await;
    let body = b"not json at all";
    let (sig, ts) = sign_request(body);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Malformed JSON should return 400");
}

#[tokio::test]
async fn missing_did_field_rejected() {
    let (_irc, http, _h) = start().await;
    let body = serde_json::json!({"handle": "x"}); // Missing "did"
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let (sig, ts) = sign_request(&body_bytes);
    let resp = reqwest::Client::new()
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("X-Broker-Signature", &sig)
        .header("X-Broker-Timestamp", &ts)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Missing 'did' field should return 400");
}
