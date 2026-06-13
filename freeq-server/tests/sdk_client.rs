//! Tests for the SDK client state machine (client.rs, hotspot #6, gamma 104).
//!
//! Tests ConnectConfig validation, ClientHandle methods, and the full
//! connect → register → channel lifecycle via a live test server.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use freeq_sdk::auth::{ChallengeSigner, KeySigner};
use freeq_sdk::client::{self, ConnectConfig};
use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};
use freeq_sdk::event::Event;
use tokio::sync::mpsc;
use tokio::time::timeout;

const DID: &str = "did:plc:sdk_test";

async fn start() -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let resolver = DidResolver::static_map(HashMap::new());
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-sdk".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    freeq_server::server::Server::with_resolver(config, resolver)
        .start()
        .await
        .unwrap()
}

async fn start_with_did(
    key: &PrivateKey,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let doc = did::make_test_did_document(DID, &key.public_key_multibase());
    let mut docs = HashMap::new();
    docs.insert(DID.to_string(), doc);
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-sdk-auth".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    freeq_server::server::Server::with_resolver(config, DidResolver::static_map(docs))
        .start()
        .await
        .unwrap()
}

async fn wait(rx: &mut mpsc::Receiver<Event>, pred: impl Fn(&Event) -> bool, desc: &str) -> Event {
    timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Some(e) if pred(&e) => return e,
                Some(_) => continue,
                None => panic!("Closed: {desc}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Timeout: {desc}"))
}

// ═══════════════════════════════════════════════════════════════
// CONNECT CONFIG VALIDATION
// ═══════════════════════════════════════════════════════════════

#[test]
fn config_default_valid() {
    assert!(ConnectConfig::default().validate().is_ok());
}

#[test]
fn config_empty_addr_invalid() {
    let mut c = ConnectConfig::default();
    c.server_addr = String::new();
    assert!(c.validate().is_err());
}

#[test]
fn config_empty_nick_invalid() {
    let mut c = ConnectConfig::default();
    c.nick = String::new();
    assert!(c.validate().is_err());
}

#[test]
fn config_long_nick_invalid() {
    let mut c = ConnectConfig::default();
    c.nick = "a".repeat(65);
    assert!(c.validate().is_err());
}

#[test]
fn config_nick_with_space_invalid() {
    let mut c = ConnectConfig::default();
    c.nick = "has space".to_string();
    assert!(c.validate().is_err());
}

#[test]
fn config_nick_with_comma_invalid() {
    let mut c = ConnectConfig::default();
    c.nick = "has,comma".to_string();
    assert!(c.validate().is_err());
}

#[test]
fn config_nick_with_at_invalid() {
    let mut c = ConnectConfig::default();
    c.nick = "has@at".to_string();
    assert!(c.validate().is_err());
}

#[test]
fn config_nick_with_hash_invalid() {
    let mut c = ConnectConfig::default();
    c.nick = "#channel".to_string();
    assert!(c.validate().is_err());
}

#[test]
fn config_empty_user_invalid() {
    let mut c = ConnectConfig::default();
    c.user = String::new();
    assert!(c.validate().is_err());
}

#[test]
fn config_valid_nick() {
    let mut c = ConnectConfig::default();
    c.nick = "valid-nick_123".to_string();
    assert!(c.validate().is_ok());
}

#[test]
fn config_unicode_nick_valid() {
    let mut c = ConnectConfig::default();
    c.nick = "café".to_string();
    assert!(c.validate().is_ok());
}

#[test]
fn config_64_char_nick_valid() {
    let mut c = ConnectConfig::default();
    c.nick = "a".repeat(64);
    assert!(c.validate().is_ok());
}

// ═══════════════════════════════════════════════════════════════
// GUEST CONNECTION LIFECYCLE
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn guest_connect_register() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkguest".to_string(),
        user: "sdkguest".to_string(),
        realname: "test".to_string(),
        ..Default::default()
    };
    let (_handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Connected), "Connected").await;
    let reg = wait(
        &mut rx,
        |e| matches!(e, Event::Registered { .. }),
        "Registered",
    )
    .await;
    if let Event::Registered { nick } = reg {
        assert_eq!(nick, "sdkguest");
    }
}

#[tokio::test]
async fn guest_join_channel() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkjoin".to_string(),
        user: "sdkjoin".to_string(),
        realname: "test".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(
        &mut rx,
        |e| matches!(e, Event::Registered { .. }),
        "Registered",
    )
    .await;
    handle.join("#sdktest").await.unwrap();
    wait(
        &mut rx,
        |e| matches!(e, Event::Joined { channel, .. } if channel == "#sdktest"),
        "Joined",
    )
    .await;
}

#[tokio::test]
async fn guest_send_receive_message() {
    let (addr, _h) = start().await;
    // Two clients in same channel
    let c1 = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdk1".to_string(),
        user: "sdk1".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let c2 = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdk2".to_string(),
        user: "sdk2".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (h1, mut rx1) = client::connect(c1, None);
    let (h2, mut rx2) = client::connect(c2, None);
    wait(&mut rx1, |e| matches!(e, Event::Registered { .. }), "Reg1").await;
    wait(&mut rx2, |e| matches!(e, Event::Registered { .. }), "Reg2").await;
    h1.join("#sdkmsg").await.unwrap();
    h2.join("#sdkmsg").await.unwrap();
    wait(
        &mut rx1,
        |e| matches!(e, Event::Joined { channel, .. } if channel == "#sdkmsg"),
        "J1",
    )
    .await;
    wait(
        &mut rx2,
        |e| matches!(e, Event::Joined { channel, .. } if channel == "#sdkmsg"),
        "J2",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    h1.privmsg("#sdkmsg", "hello from sdk").await.unwrap();
    let msg = wait(
        &mut rx2,
        |e| matches!(e, Event::Message { text, .. } if text == "hello from sdk"),
        "Msg",
    )
    .await;
    if let Event::Message { from, text, .. } = msg {
        assert_eq!(from, "sdk1");
        assert_eq!(text, "hello from sdk");
    }
}

#[tokio::test]
async fn guest_quit() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkquit".to_string(),
        user: "sdkquit".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Registered { .. }), "Reg").await;
    handle.quit(Some("goodbye")).await.unwrap();
    wait(
        &mut rx,
        |e| matches!(e, Event::Disconnected { .. }),
        "Disconnected",
    )
    .await;
}

// ═══════════════════════════════════════════════════════════════
// AUTHENTICATED CONNECTION
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn authenticated_connect() {
    let key = PrivateKey::generate_ed25519();
    let (addr, _h) = start_with_did(&key).await;
    let signer: Arc<dyn ChallengeSigner> = Arc::new(KeySigner::new(DID.to_string(), key));
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkauth".to_string(),
        user: "sdkauth".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (_handle, mut rx) = client::connect(config, Some(signer));
    wait(&mut rx, |e| matches!(e, Event::Connected), "Connected").await;
    wait(
        &mut rx,
        |e| matches!(e, Event::Authenticated { .. }),
        "Authenticated",
    )
    .await;
    let reg = wait(
        &mut rx,
        |e| matches!(e, Event::Registered { .. }),
        "Registered",
    )
    .await;
    if let Event::Registered { nick } = reg {
        assert_eq!(nick, "sdkauth");
    }
}

// ═══════════════════════════════════════════════════════════════
// CLIENT HANDLE METHODS
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn handle_raw_command() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkraw".to_string(),
        user: "sdkraw".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Registered { .. }), "Reg").await;
    // Raw PING should get PONG back (handled internally by client loop)
    handle.raw("PING :testraw").await.unwrap();
    // The client handles PONG internally — just verify no crash
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn handle_raw_crlf_stripped() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkcrlf".to_string(),
        user: "sdkcrlf".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Registered { .. }), "Reg").await;
    // CRLF injection attempt — should be stripped
    handle
        .raw("PRIVMSG #test :hello\r\nQUIT :pwned")
        .await
        .unwrap();
    // Should NOT disconnect (QUIT stripped)
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Verify still connected by sending another command
    handle.raw("PING :alive").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn handle_typing_indicators() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdktype".to_string(),
        user: "sdktype".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Registered { .. }), "Reg").await;
    handle.join("#sdktype").await.unwrap();
    wait(&mut rx, |e| matches!(e, Event::Joined { .. }), "Joined").await;
    // Typing start and stop should not crash
    handle.typing_start("#sdktype").await.unwrap();
    handle.typing_stop("#sdktype").await.unwrap();
}

#[tokio::test]
async fn handle_reply() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkreply".to_string(),
        user: "sdkreply".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Registered { .. }), "Reg").await;
    handle.join("#sdkreply").await.unwrap();
    wait(&mut rx, |e| matches!(e, Event::Joined { .. }), "Joined").await;
    // Reply with msgid tag
    handle
        .reply("#sdkreply", "test-msgid-123", "this is a reply")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn handle_react() {
    let (addr, _h) = start().await;
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "sdkreact".to_string(),
        user: "sdkreact".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (handle, mut rx) = client::connect(config, None);
    wait(&mut rx, |e| matches!(e, Event::Registered { .. }), "Reg").await;
    handle.join("#sdkreact").await.unwrap();
    wait(&mut rx, |e| matches!(e, Event::Joined { .. }), "Joined").await;
    handle.react("#sdkreact", "👍", "test-msgid").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

// ═══════════════════════════════════════════════════════════════
// NICK COLLISION
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn nick_collision_gets_alternate() {
    let (addr, _h) = start().await;
    // First client takes the nick
    let c1 = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "taken".to_string(),
        user: "u".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (_h1, mut rx1) = client::connect(c1, None);
    wait(&mut rx1, |e| matches!(e, Event::Registered { .. }), "Reg1").await;

    // Second client tries the same nick
    let c2 = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "taken".to_string(),
        user: "u".to_string(),
        realname: "t".to_string(),
        ..Default::default()
    };
    let (_h2, mut rx2) = client::connect(c2, None);
    let reg = wait(&mut rx2, |e| matches!(e, Event::Registered { .. }), "Reg2").await;
    if let Event::Registered { nick } = reg {
        // Should get an alternate nick (taken + suffix)
        assert_ne!(nick, "taken", "Should get alternate nick, got: {nick}");
        assert!(
            nick.starts_with("taken"),
            "Alternate should be based on original: {nick}"
        );
    }
}
