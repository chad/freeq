//! Acceptance tests for `GET /api/v1/actors/{did}`.
//!
//! Specifically: the `nick` field must resolve from the persistent
//! `identities` table when no live session exists for the DID, so the
//! freeq-app's provenance card can render e.g. "Creator: lobot" even
//! after the moderator process has exited. Without the fallback, the
//! card renders a raw "did:key:z6Mk…" string for any offline creator.

use std::collections::HashMap;

use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};

const TEST_DID: &str = "did:key:z6MkActorIdentityFallbackTest";

async fn start_server() -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    String,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    // The server's resolver needs *some* mapping but the actor-identity
    // endpoint we exercise doesn't care about challenge verification;
    // a static empty resolver is fine for the offline-fallback path.
    let key = PrivateKey::generate_ed25519();
    let doc = did::make_test_did_document(TEST_DID, &key.public_key_multibase());
    let mut docs = HashMap::new();
    docs.insert(TEST_DID.to_string(), doc);
    let resolver = DidResolver::static_map(docs);

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap().to_string();
    // Leak the tempfile guard so the path lives for the test's duration.
    std::mem::forget(tmp);

    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-actor-identity".to_string(),
        challenge_timeout_secs: 60,
        db_path: Some(db_path.clone()),
        ..Default::default()
    };
    let (irc_addr, http_addr, handle) =
        freeq_server::server::Server::with_resolver(config, resolver)
            .start_with_web()
            .await
            .unwrap();
    (irc_addr, http_addr, db_path, handle)
}

/// Insert a DID↔nick binding directly into the identities table,
/// simulating a prior SASL/LOGIN success with no current session.
fn save_identity_row(db_path: &str, did: &str, nick: &str) {
    let conn = rusqlite::Connection::open(db_path).expect("open db");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    conn.execute(
        "INSERT OR REPLACE INTO identities (did, nick, last_auth_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![did, nick, now],
    )
    .expect("save identity");
}

#[tokio::test]
async fn actor_identity_returns_persisted_nick_for_offline_did() {
    let (_irc, http, db_path, _handle) = start_server().await;

    // Seed: a DID with a persistent nick but no active session. This is
    // the "offline creator" case — a moderator process that exited
    // hours ago but spawned a panelist that's still in someone's
    // history/provenance card.
    save_identity_row(&db_path, TEST_DID, "lobot");

    let url = format!(
        "http://{http}/api/v1/actors/{}",
        urlencoding::encode(TEST_DID)
    );
    let resp: serde_json::Value = reqwest::get(&url)
        .await
        .expect("GET /api/v1/actors")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(
        resp.get("nick").and_then(|v| v.as_str()),
        Some("lobot"),
        "offline DID should fall back to persistent identities table; got: {resp}",
    );
    assert_eq!(
        resp.get("online").and_then(|v| v.as_bool()),
        Some(false),
        "no session is active, so online must be false; got: {resp}",
    );
}

#[tokio::test]
async fn actor_identity_omits_nick_for_unknown_did() {
    let (_irc, http, _db_path, _handle) = start_server().await;

    // No identity row, no session — nick must be absent (not blank).
    let unknown = "did:key:z6MkNeverSeenBefore";
    let url = format!(
        "http://{http}/api/v1/actors/{}",
        urlencoding::encode(unknown)
    );
    let resp: serde_json::Value = reqwest::get(&url).await.unwrap().json().await.unwrap();

    assert!(
        resp.get("nick").is_none(),
        "DID with no session and no identity row should have no nick field; got: {resp}",
    );
}
