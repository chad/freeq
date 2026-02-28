//! Upload endpoint and HTTP API acceptance tests.
//!
//! Tests the /api/v1/upload auth, blob proxy SSRF, OG preview SSRF,
//! broker signature, CSP headers, and token lifecycle.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use freeq_sdk::auth::KeySigner;
use freeq_sdk::client::{self, ConnectConfig};
use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};
use freeq_sdk::event::Event;
use tokio::sync::mpsc;
use tokio::time::timeout;

const TEST_DID: &str = "did:plc:test1234upload";
const TIMEOUT_MS: u64 = 5000;

/// Start a test server with both IRC and HTTP.
async fn start_server() -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let key = PrivateKey::generate_ed25519();
    let did_doc = did::make_test_did_document(TEST_DID, &key.public_key_multibase());
    let mut docs = HashMap::new();
    docs.insert(TEST_DID.to_string(), did_doc);
    let resolver = DidResolver::static_map(docs);

    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-upload".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    server.start_with_web().await.unwrap()
}

/// Connect and authenticate an IRC client (populates session_dids on server).
async fn connect_authenticated(
    irc_addr: std::net::SocketAddr,
    key: PrivateKey,
) -> mpsc::Receiver<Event> {
    let signer: Arc<dyn freeq_sdk::auth::ChallengeSigner> =
        Arc::new(KeySigner::new(TEST_DID.to_string(), key));

    let config = ConnectConfig {
        server_addr: irc_addr.to_string(),
        nick: "testuploader".to_string(),
        user: "testuploader".to_string(),
        realname: "test".to_string(),
        ..Default::default()
    };

    let (_handle, rx) = client::connect(config, Some(signer));
    rx
}

/// Wait for matching event.
async fn wait_for(rx: &mut mpsc::Receiver<Event>, predicate: impl Fn(&Event) -> bool, desc: &str) {
    let deadline = Duration::from_millis(TIMEOUT_MS);
    let start = tokio::time::Instant::now();
    loop {
        match timeout(deadline.saturating_sub(start.elapsed()), rx.recv()).await {
            Ok(Some(ref event)) if predicate(event) => return,
            Ok(Some(_)) => continue,
            _ => panic!("Timeout waiting for: {desc}"),
        }
    }
}

// ── Upload auth tests ──────────────────────────────────────────────────

#[tokio::test]
async fn upload_rejects_without_auth() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let form = reqwest::multipart::Form::new()
        .text("did", TEST_DID.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(vec![0u8; 100])
                .file_name("test.bin")
                .mime_str("application/octet-stream")
                .unwrap(),
        );

    let resp = client
        .post(format!("http://{http}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "Upload without session should be 401");
}

#[tokio::test]
async fn upload_rejects_no_did() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(vec![0u8; 100])
            .file_name("test.bin")
            .mime_str("application/octet-stream")
            .unwrap(),
    );

    let resp = client
        .post(format!("http://{http}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn upload_rejects_no_file() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let form = reqwest::multipart::Form::new().text("did", TEST_DID.to_string());

    let resp = client
        .post(format!("http://{http}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn upload_rejects_oversized_file() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let big = vec![0u8; 11 * 1024 * 1024]; // 11MB > 10MB limit
    let form = reqwest::multipart::Form::new()
        .text("did", TEST_DID.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(big)
                .file_name("big.bin")
                .mime_str("application/octet-stream")
                .unwrap(),
        );

    let resp = client
        .post(format!("http://{http}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 413);
}

#[tokio::test]
async fn upload_with_active_session_passes_auth_gate() {
    let key = PrivateKey::generate_ed25519();
    let did_doc = did::make_test_did_document(TEST_DID, &key.public_key_multibase());
    let mut docs = HashMap::new();
    docs.insert(TEST_DID.to_string(), did_doc);
    let resolver = DidResolver::static_map(docs);

    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-upload-session".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    let (irc, http, _h) = server.start_with_web().await.unwrap();

    // Connect + authenticate via IRC to populate session_dids
    let mut rx = connect_authenticated(irc, key).await;
    wait_for(&mut rx, |e| matches!(e, Event::Connected), "connected").await;
    wait_for(
        &mut rx,
        |e| matches!(e, Event::Authenticated { .. }),
        "auth",
    )
    .await;
    wait_for(
        &mut rx,
        |e| matches!(e, Event::Registered { .. }),
        "registered",
    )
    .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .text("did", TEST_DID.to_string())
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"hello".to_vec())
                .file_name("test.txt")
                .mime_str("text/plain")
                .unwrap(),
        );

    let resp = client
        .post(format!("http://{http}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap();
    // Auth gate passed → "No active session for this DID" (no PDS creds)
    assert_eq!(status, 401);
    assert!(
        body.contains("No active session for this DID"),
        "Should pass auth gate but fail on PDS session; got: {body}"
    );
}

#[tokio::test]
async fn upload_with_wrong_did_rejected() {
    let key = PrivateKey::generate_ed25519();
    let did_doc = did::make_test_did_document(TEST_DID, &key.public_key_multibase());
    let mut docs = HashMap::new();
    docs.insert(TEST_DID.to_string(), did_doc);
    let resolver = DidResolver::static_map(docs);

    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-upload-wrong".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    let (irc, http, _h) = server.start_with_web().await.unwrap();

    let mut rx = connect_authenticated(irc, key).await;
    wait_for(&mut rx, |e| matches!(e, Event::Connected), "connected").await;
    wait_for(
        &mut rx,
        |e| matches!(e, Event::Authenticated { .. }),
        "auth",
    )
    .await;
    wait_for(
        &mut rx,
        |e| matches!(e, Event::Registered { .. }),
        "registered",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .text("did", "did:plc:someone_else")
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"hello".to_vec())
                .file_name("test.txt")
                .mime_str("text/plain")
                .unwrap(),
        );

    let resp = client
        .post(format!("http://{http}/api/v1/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "Upload with different DID should be 401"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("active connection"),
        "Should mention session requirement: {body}"
    );
}

// ── Blob proxy tests ───────────────────────────────────────────────────

#[tokio::test]
async fn blob_proxy_rejects_non_pds_url() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/blob?url={}",
            urlencoding::encode("https://evil.com/data")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn blob_proxy_rejects_wrong_host_with_pds_path() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/blob?url={}",
            urlencoding::encode("https://evil.com/xrpc/com.atproto.sync.getBlob?did=x&cid=y")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn blob_proxy_rejects_http_scheme() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client.get(format!("http://{http}/api/v1/blob?url={}", urlencoding::encode("http://puffball.us-east.host.bsky.network/xrpc/com.atproto.sync.getBlob?did=x&cid=y")))
        .send().await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn blob_proxy_accepts_valid_pds_url() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let url = "https://puffball.us-east.host.bsky.network/xrpc/com.atproto.sync.getBlob?did=did:plc:test&cid=bafytest";
    let resp = client
        .get(format!(
            "http://{http}/api/v1/blob?url={}",
            urlencoding::encode(url)
        ))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 400, "Valid PDS URL should pass validation");
}

#[tokio::test]
async fn blob_proxy_accepts_cdn_url() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let url = "https://cdn.bsky.app/img/feed_fullsize/plain/did:plc:test/bafytest@jpeg";
    let resp = client
        .get(format!(
            "http://{http}/api/v1/blob?url={}",
            urlencoding::encode(url)
        ))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 400, "CDN URL should pass validation");
}

#[tokio::test]
async fn blob_proxy_rejects_ssrf_host_trick() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/blob?url={}",
            urlencoding::encode("https://evil.com/cdn.bsky.app/img/test")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── OG preview SSRF tests ──────────────────────────────────────────────

#[tokio::test]
async fn og_preview_rejects_localhost() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/og?url={}",
            urlencoding::encode("http://localhost/admin")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn og_preview_rejects_loopback_ip() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/og?url={}",
            urlencoding::encode("http://127.0.0.1:6667/")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn og_preview_rejects_local_hostname() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/og?url={}",
            urlencoding::encode("http://router.local/admin")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn og_preview_rejects_file_scheme() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "http://{http}/api/v1/og?url={}",
            urlencoding::encode("file:///etc/passwd")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── Broker endpoint tests ──────────────────────────────────────────────

#[tokio::test]
async fn broker_web_token_rejects_without_signature() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{http}/auth/broker/web-token"))
        .header("content-type", "application/json")
        .body(r#"{"did":"did:plc:test","handle":"test","token":"tok123"}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 401 || status == 403,
        "Missing signature should be rejected, got {status}"
    );
}

#[tokio::test]
async fn broker_session_rejects_without_signature() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{http}/auth/broker/session"))
        .header("content-type", "application/json")
        .body(r#"{"did":"did:plc:test"}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 401 || status == 403,
        "Missing signature should be rejected, got {status}"
    );
}

// ── Security header tests ──────────────────────────────────────────────

#[tokio::test]
async fn security_headers_present() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{http}/api/v1/health"))
        .send()
        .await
        .unwrap();

    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(csp.contains("img-src"), "CSP should include img-src: {csp}");
    assert!(
        csp.contains("blob:"),
        "CSP img-src should allow blob: URLs: {csp}"
    );
    assert!(
        csp.contains("frame-ancestors 'none'"),
        "CSP should deny framing: {csp}"
    );

    assert!(resp.headers().contains_key("x-content-type-options"));
    assert!(resp.headers().contains_key("x-frame-options"));
}

#[tokio::test]
async fn health_endpoint_returns_json() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{http}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("connections").is_some());
    assert!(body.get("channels").is_some());
}

#[tokio::test]
async fn channels_api_returns_json() {
    let (_irc, http, _h) = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{http}/api/v1/channels"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array());
}
