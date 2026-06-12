//! End-to-end test of the agent_assist surface USING AUTH.
//!
//! Proves the full flow:
//!
//!   1. Server emits `NOTICE * :API-BEARER <session_id>` on SASL success.
//!   2. A bot can capture that bearer.
//!   3. Calling /agent/tools/* with `Authorization: Bearer <session_id>`
//!      resolves to the bot's DID via state.session_dids and returns
//!      *useful* responses (not SELF_ONLY denials).
//!
//! Without this bridge bots can only get 2/5 tools (discovery +
//! validate_client_config). With it, all 5 SELF_ONLY-gated tools work.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use freeq_sdk::auth::{self, ChallengeSigner, KeySigner};
use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};

const DID_BOT: &str = "did:plc:authbot";

/// Returns (irc, web, state, handle, bot_key). The bot_key is the
/// private side of the DID document the static resolver was loaded
/// with, so the test client can sign the SASL challenge.
async fn start() -> (
    SocketAddr,
    SocketAddr,
    Arc<freeq_server::server::SharedState>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    PrivateKey,
) {
    let key = PrivateKey::generate_ed25519();
    let mut docs = HashMap::new();
    docs.insert(
        DID_BOT.to_string(),
        did::make_test_did_document(DID_BOT, &key.public_key_multibase()),
    );
    // We need to keep one private side for the test client. Re-derive
    // the same key by creating a second one here? No — generate_ed25519
    // returns a fresh key each time. Instead, generate once and re-use.
    let key2 = PrivateKey::generate_ed25519();
    docs.insert(
        format!("{DID_BOT}-2"),
        did::make_test_did_document(&format!("{DID_BOT}-2"), &key2.public_key_multibase()),
    );
    let resolver = DidResolver::static_map(docs);

    let tmp = tempfile::Builder::new()
        .prefix("freeq-auth-bot-")
        .suffix(".db")
        .tempfile()
        .unwrap();
    let db_path = tmp.path().to_str().unwrap().to_string();
    std::mem::forget(tmp);

    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "auth-test".to_string(),
        challenge_timeout_secs: 60,
        db_path: Some(db_path),
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    let (irc, web, handle, state) = server.start_with_web_state().await.unwrap();
    let _ = key2; // silence unused — keeping the second key for symmetry
    (irc, web, state, handle, key)
}

/// Connect via SASL with did:key, return the captured API-BEARER value
/// AND the open TcpStream — keep both halves alive so the server
/// doesn't clean up the session_id (and the bearer with it) the moment
/// the SASL handshake finishes.
fn auth_and_capture_bearer(
    addr: SocketAddr,
    key: PrivateKey,
) -> (String, TcpStream, BufReader<TcpStream>) {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let writer = s.try_clone().unwrap();
    let mut reader = BufReader::new(s);
    let mut writer = writer;

    fn tx(w: &mut TcpStream, line: &str) {
        writeln!(w, "{line}\r").unwrap();
        w.flush().ok();
    }
    fn rx_until(r: &mut BufReader<TcpStream>, p: impl Fn(&str) -> bool) -> String {
        loop {
            let mut b = String::new();
            if r.read_line(&mut b).unwrap() == 0 {
                panic!("EOF");
            }
            let l = b.trim_end().to_string();
            if p(&l) {
                return l;
            }
        }
    }

    tx(&mut writer, "CAP LS 302");
    tx(&mut writer, "NICK authbot");
    tx(&mut writer, "USER authbot 0 * :test");
    tx(&mut writer, "CAP REQ :sasl message-tags server-time");
    rx_until(&mut reader, |l| l.contains("ACK"));
    tx(&mut writer, "AUTHENTICATE ATPROTO-CHALLENGE");
    let challenge_line = rx_until(&mut reader, |l| l.starts_with("AUTHENTICATE "));
    let challenge = challenge_line.strip_prefix("AUTHENTICATE ").unwrap();
    let bytes = auth::decode_challenge_bytes(challenge).unwrap();
    let signer = KeySigner::new(DID_BOT.to_string(), key);
    let resp = signer.respond(&bytes).unwrap();
    tx(
        &mut writer,
        &format!("AUTHENTICATE {}", auth::encode_response(&resp)),
    );

    // After 903 RPL_SASLSUCCESS the server now also sends:
    //   :auth-test NOTICE * :API-BEARER <session_id>
    // Capture and return the session_id.
    let notice = rx_until(&mut reader, |l| l.contains("API-BEARER"));
    let bearer = notice.split_whitespace().last().unwrap().to_string();

    tx(&mut writer, "CAP END");
    rx_until(&mut reader, |l| l.contains(" 001 "));
    (bearer, writer, reader)
}

async fn call_tool(
    web: SocketAddr,
    bearer: &str,
    name: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let mut req = reqwest::Client::new()
        .post(format!("http://{web}/agent/tools/{name}"))
        .header("Content-Type", "application/json");
    if !bearer.is_empty() {
        req = req.header("Authorization", format!("Bearer {bearer}"));
    }
    req.json(&body).send().await.unwrap().json().await.unwrap()
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn server_emits_api_bearer_notice_on_sasl_success() {
    let (irc, _web, _state, _h, key) = start().await;
    let (bearer, _w, _r) = tokio::task::spawn_blocking(move || auth_and_capture_bearer(irc, key))
        .await
        .unwrap();
    assert!(
        !bearer.is_empty(),
        "expected the server to emit API-BEARER NOTICE after SASL success"
    );
    // session_ids look like "127.0.0.1:NNNNN" or "stream-..."; just
    // confirm it's a non-trivial token.
    assert!(
        bearer.len() > 4 && bearer.contains(':') || bearer.starts_with("stream-"),
        "session_id has an unexpected shape: {bearer}"
    );
}

#[tokio::test]
async fn anonymous_caller_gets_self_only_denials() {
    // Baseline: confirm the diagnostic tools deny anonymous callers
    // for the SELF_ONLY-gated ones. Demonstrates the pre-auth state
    // of the world before our bridge.
    let (_irc, web, _state, _h, _key) = start().await;
    for (tool, body, expected_code) in [
        (
            "predict_message_outcome",
            serde_json::json!({"account": DID_BOT, "target": "#anything"}),
            "PREDICT_MESSAGE_OUTCOME_SELF_ONLY",
        ),
        (
            "diagnose_join_failure",
            serde_json::json!({"account": DID_BOT, "channel": "#x"}),
            "DIAGNOSE_JOIN_FAILURE_SELF_ONLY",
        ),
        (
            "inspect_my_session",
            serde_json::json!({"account": DID_BOT}),
            "INSPECT_MY_SESSION_SELF_ONLY",
        ),
    ] {
        let resp = call_tool(web, "", tool, body).await;
        assert_eq!(
            resp["diagnosis"]["code"], expected_code,
            "anonymous caller should be denied by {tool}; got {resp:#}"
        );
    }
}

#[tokio::test]
async fn authenticated_bot_unlocks_predict_diagnose_inspect() {
    // The full flow: SASL with did:key → capture API-BEARER → call
    // each SELF_ONLY-gated tool with bearer → expect actual content.
    let (irc, web, _state, _h, key) = start().await;
    let (bearer, _w, _r) = tokio::task::spawn_blocking(move || auth_and_capture_bearer(irc, key))
        .await
        .unwrap();

    // 1. predict_message_outcome — bot is connected, no rate-limit
    //    state, target is a fresh channel name. Should report
    //    PREDICTED_ACCEPTED or PREDICT_CHANNEL_DOES_NOT_EXIST (the
    //    channel hasn't been created yet). Either is real, useful
    //    content — not SELF_ONLY.
    let predict = call_tool(
        web,
        &bearer,
        "predict_message_outcome",
        serde_json::json!({"account": DID_BOT, "target": "#new-channel"}),
    )
    .await;
    let predict_code = predict["diagnosis"]["code"].as_str().unwrap_or("");
    assert!(
        !predict_code.contains("SELF_ONLY"),
        "with bearer auth, predict_message_outcome must not deny as SELF_ONLY; got {predict:#}"
    );
    assert!(
        predict_code.starts_with("PREDICT"),
        "expected a PREDICT_* code; got {predict_code}"
    );

    // 2. inspect_my_session — should report the bot's actual session
    //    state: nick, joined channels, capabilities. NOT SELF_ONLY.
    let inspect = call_tool(
        web,
        &bearer,
        "inspect_my_session",
        serde_json::json!({"account": DID_BOT}),
    )
    .await;
    let inspect_code = inspect["diagnosis"]["code"].as_str().unwrap_or("");
    assert!(
        !inspect_code.contains("SELF_ONLY"),
        "with bearer auth, inspect_my_session must not deny as SELF_ONLY; got {inspect:#}"
    );
    assert_eq!(
        inspect_code, "SESSION_REPORTED",
        "expected SESSION_REPORTED; got {inspect_code}"
    );
    let facts = inspect["safe_facts"].as_array().unwrap();
    assert!(
        facts
            .iter()
            .any(|f| f.as_str().unwrap_or("").contains("authbot")),
        "session_reported should include the bot's nick (authbot); got facts {facts:?}"
    );

    // 3. diagnose_join_failure on a channel that doesn't exist —
    //    should report CHANNEL_DOES_NOT_EXIST (real, useful), not
    //    SELF_ONLY.
    let diagnose = call_tool(
        web,
        &bearer,
        "diagnose_join_failure",
        serde_json::json!({"account": DID_BOT, "channel": "#nope"}),
    )
    .await;
    let diag_code = diagnose["diagnosis"]["code"].as_str().unwrap_or("");
    assert!(
        !diag_code.contains("SELF_ONLY"),
        "with bearer auth, diagnose_join_failure must not deny as SELF_ONLY; got {diagnose:#}"
    );
    assert!(
        diag_code == "CHANNEL_DOES_NOT_EXIST" || diag_code == "JOIN_SHOULD_SUCCEED",
        "expected real diagnosis code; got {diag_code}"
    );
}

#[tokio::test]
async fn did_key_sasl_resolves_locally_without_pds() {
    // Proves the path the JS SDK now uses: auth with a did:key the
    // server has never seen before, no static_map entry, no PDS, no
    // OAuth. The server resolves did:key locally from the public key
    // embedded in the DID string itself — and verify_crypto signs the
    // raw challenge bytes against that key.
    //
    // This is the wire-level equivalent of `generateDidKey()` +
    // `setSaslCredentials({method: "crypto", signer})` from the JS
    // SDK — if the Rust verifier accepts it, the JS bot will too.
    let resolver = DidResolver::http(); // production resolver, no static map
    let tmp = tempfile::Builder::new()
        .prefix("freeq-didkey-")
        .suffix(".db")
        .tempfile()
        .unwrap();
    let db_path = tmp.path().to_str().unwrap().to_string();
    std::mem::forget(tmp);
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "didkey-test".to_string(),
        challenge_timeout_secs: 60,
        db_path: Some(db_path),
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    let (irc, web, _handle, _state) = server.start_with_web_state().await.unwrap();

    // Generate a fresh ed25519 key + derive did:key from its multibase
    // public form — same algorithm as `generateDidKey()` in the JS
    // SDK. The DID is `did:key:<multibase>` and the server resolves it
    // by parsing that string alone.
    let key = PrivateKey::generate_ed25519();
    let multibase = key.public_key_multibase();
    let did = format!("did:key:{multibase}");

    let irc_addr = irc;
    let did_for_thread = did.clone();
    let (bearer, _w, _r) = tokio::task::spawn_blocking(move || {
        auth_and_capture_bearer_with_did(irc_addr, &did_for_thread, key)
    })
    .await
    .unwrap();

    // The server must have accepted the did:key SASL response and
    // emitted the API-BEARER NOTICE. Use the bearer to call a
    // SELF_ONLY-gated tool.
    let resp = call_tool(
        web,
        &bearer,
        "inspect_my_session",
        serde_json::json!({"account": did}),
    )
    .await;
    assert_eq!(
        resp["diagnosis"]["code"], "SESSION_REPORTED",
        "did:key SASL must unlock the diagnostic surface; got {resp:#}"
    );
}

/// Same as `auth_and_capture_bearer` but takes an explicit DID — used
/// when the test generates its own did:key rather than using the static
/// `DID_BOT` constant.
fn auth_and_capture_bearer_with_did(
    addr: SocketAddr,
    did: &str,
    key: PrivateKey,
) -> (String, TcpStream, BufReader<TcpStream>) {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let writer = s.try_clone().unwrap();
    let mut reader = BufReader::new(s);
    let mut writer = writer;

    fn tx(w: &mut TcpStream, line: &str) {
        writeln!(w, "{line}\r").unwrap();
        w.flush().ok();
    }
    fn rx_until(r: &mut BufReader<TcpStream>, p: impl Fn(&str) -> bool) -> String {
        loop {
            let mut b = String::new();
            if r.read_line(&mut b).unwrap() == 0 {
                panic!("EOF");
            }
            let l = b.trim_end().to_string();
            if p(&l) {
                return l;
            }
        }
    }

    tx(&mut writer, "CAP LS 302");
    tx(&mut writer, "NICK didkeybot");
    tx(&mut writer, "USER didkeybot 0 * :test");
    tx(&mut writer, "CAP REQ :sasl message-tags server-time");
    rx_until(&mut reader, |l| l.contains("ACK"));
    tx(&mut writer, "AUTHENTICATE ATPROTO-CHALLENGE");
    let challenge_line = rx_until(&mut reader, |l| l.starts_with("AUTHENTICATE "));
    let challenge = challenge_line.strip_prefix("AUTHENTICATE ").unwrap();
    let bytes = auth::decode_challenge_bytes(challenge).unwrap();
    let signer = KeySigner::new(did.to_string(), key);
    let resp = signer.respond(&bytes).unwrap();
    tx(
        &mut writer,
        &format!("AUTHENTICATE {}", auth::encode_response(&resp)),
    );
    let notice = rx_until(&mut reader, |l| l.contains("API-BEARER"));
    let bearer = notice.split_whitespace().last().unwrap().to_string();
    tx(&mut writer, "CAP END");
    rx_until(&mut reader, |l| l.contains(" 001 "));
    (bearer, writer, reader)
}

#[tokio::test]
async fn bot_cannot_use_someone_elses_bearer_to_inspect_their_session() {
    // Defense in depth: make sure the bearer authenticates ONLY the
    // session that minted it. A bot using its own bearer to inspect
    // a different DID should still be denied.
    let (irc, web, _state, _h, key) = start().await;
    let (bearer, _w, _r) = tokio::task::spawn_blocking(move || auth_and_capture_bearer(irc, key))
        .await
        .unwrap();

    // Try to inspect did:plc:somebody-else with our own bearer.
    let resp = call_tool(
        web,
        &bearer,
        "inspect_my_session",
        serde_json::json!({"account": "did:plc:notthebot"}),
    )
    .await;
    assert_eq!(
        resp["diagnosis"]["code"], "INSPECT_MY_SESSION_SELF_ONLY",
        "bearer must scope to the issuing DID — can't inspect arbitrary accounts; got {resp:#}"
    );
}
