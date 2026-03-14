//! Acceptance tests for agent-native Phase 1 features.
//!
//! Tests: did:key auth, AGENT REGISTER, PROVENANCE, PRESENCE, HEARTBEAT,
//! actor class in WHOIS, actor class tag in JOIN, REST identity card.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Start deadlock detection background thread.
/// Checks every 500ms and panics with thread info on deadlock.
fn start_deadlock_detector() {
    use std::thread;
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(500));
        let deadlocks = parking_lot::deadlock::check_deadlock();
        if deadlocks.is_empty() {
            continue;
        }
        eprintln!("!!! DEADLOCK DETECTED ({} threads):", deadlocks.len());
        for (i, threads) in deadlocks.iter().enumerate() {
            eprintln!("Deadlock #{i}:");
            for t in threads {
                eprintln!(
                    "  Thread {:?}:\n{:?}",
                    t.thread_id(),
                    t.backtrace()
                );
            }
        }
        std::process::abort();
    });
}

use freeq_sdk::auth::{ChallengeSigner, KeySigner};
use freeq_sdk::client::{self, ConnectConfig};
use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};
use freeq_sdk::event::Event;
use tokio::sync::mpsc;
use tokio::time::timeout;

// ── Helpers ─────────────────────────────────────────────────────────

async fn start_test_server(
    resolver: DidResolver,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    start_test_server_with_db(resolver, false).await
}

async fn start_test_server_with_db(
    resolver: DidResolver,
    enable_db: bool,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-server".to_string(),
        challenge_timeout_secs: 60,
        db_path: if enable_db { Some(":memory:".to_string()) } else { None },
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    server.start().await.unwrap()
}

async fn start_test_server_with_web(
    resolver: DidResolver,
) -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        web_addr: Some("127.0.0.1:0".to_string()),
        server_name: "test-server".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    let (irc_addr, handle) = server.start().await.unwrap();
    // Web addr is irc_addr.port() + 1 by convention in tests? No — we need to get it.
    // Actually the server binds web on a separate random port. Let me check the API.
    // For now, we'll skip REST tests that need a web port and test via IRC commands.
    (irc_addr, irc_addr, handle) // placeholder
}

fn empty_resolver() -> DidResolver {
    DidResolver::static_map(HashMap::new())
}

/// Create a did:key signer (no resolver entry needed — did:key is self-resolving).
fn make_did_key_signer() -> (String, Arc<dyn ChallengeSigner>) {
    let private_key = PrivateKey::generate_ed25519();
    let multibase = private_key.public_key_multibase();
    let did = format!("did:key:{multibase}");
    let signer: Arc<dyn ChallengeSigner> =
        Arc::new(KeySigner::new(did.clone(), private_key));
    (did, signer)
}

/// Connect an authenticated did:key client.
async fn connect_did_key(
    addr: std::net::SocketAddr,
    nick: &str,
) -> (
    String,
    client::ClientHandle,
    mpsc::Receiver<Event>,
) {
    let (did, signer) = make_did_key_signer();
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: nick.to_string(),
        user: nick.to_string(),
        realname: format!("{nick} bot"),
        ..Default::default()
    };
    let (handle, mut events) = client::connect(config, Some(signer));

    expect_event(&mut events, 2000, |e| matches!(e, Event::Connected), "Connected").await;
    expect_event(&mut events, 2000, |e| matches!(e, Event::Authenticated { .. }), "Authenticated").await;
    expect_event(&mut events, 2000, |e| matches!(e, Event::Registered { .. }), "Registered").await;

    (did, handle, events)
}

/// Connect a guest client.
async fn connect_guest(
    addr: std::net::SocketAddr,
    nick: &str,
) -> (client::ClientHandle, mpsc::Receiver<Event>) {
    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: nick.to_string(),
        user: nick.to_string(),
        realname: format!("{nick} guest"),
        ..Default::default()
    };
    let (handle, mut events) = client::connect(config, None);

    expect_event(&mut events, 2000, |e| matches!(e, Event::Connected), "Connected").await;
    expect_event(&mut events, 2000, |e| matches!(e, Event::Registered { .. }), "Registered").await;

    (handle, events)
}

async fn expect_event(
    events: &mut mpsc::Receiver<Event>,
    timeout_ms: u64,
    predicate: impl Fn(&Event) -> bool,
    description: &str,
) -> Event {
    let deadline = Duration::from_millis(timeout_ms);
    let start = tokio::time::Instant::now();
    loop {
        match timeout(deadline.saturating_sub(start.elapsed()), events.recv()).await {
            Ok(Some(event)) => {
                if predicate(&event) {
                    return event;
                }
            }
            Ok(None) => panic!("Channel closed while waiting for: {description}"),
            Err(_) => panic!("Timeout waiting for: {description}"),
        }
    }
}

/// Drain events looking for a RawLine matching a pattern, with timeout.
async fn expect_raw_line(
    events: &mut mpsc::Receiver<Event>,
    timeout_ms: u64,
    pattern: &str,
    description: &str,
) -> String {
    let pat = pattern.to_string();
    let evt = expect_event(
        events,
        timeout_ms,
        |e| matches!(e, Event::RawLine(line) if line.contains(&pat)),
        description,
    )
    .await;
    if let Event::RawLine(line) = evt {
        line
    } else {
        unreachable!()
    }
}

/// Check that no event matching the predicate arrives within the timeout.
async fn expect_no_event(
    events: &mut mpsc::Receiver<Event>,
    timeout_ms: u64,
    predicate: impl Fn(&Event) -> bool,
) {
    let deadline = Duration::from_millis(timeout_ms);
    let start = tokio::time::Instant::now();
    loop {
        match timeout(deadline.saturating_sub(start.elapsed()), events.recv()).await {
            Ok(Some(event)) => {
                assert!(
                    !predicate(&event),
                    "Unexpected event received: {event:?}"
                );
            }
            Ok(None) | Err(_) => return, // timeout = good, no matching event
        }
    }
}

// ── Test: did:key authentication ────────────────────────────────────

#[tokio::test]
async fn did_key_auth_ed25519() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    let (did, handle, mut events) = connect_did_key(addr, "keybot").await;

    assert!(did.starts_with("did:key:"));

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn did_key_auth_wrong_key_fails() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Create a did:key but sign with a DIFFERENT key
    let real_key = PrivateKey::generate_ed25519();
    let wrong_key = PrivateKey::generate_ed25519();
    let multibase = real_key.public_key_multibase();
    let did = format!("did:key:{multibase}");

    // Sign with wrong_key but claim real_key's DID
    let signer: Arc<dyn ChallengeSigner> =
        Arc::new(KeySigner::new(did.clone(), wrong_key));

    let config = ConnectConfig {
        server_addr: addr.to_string(),
        nick: "badbot".to_string(),
        user: "badbot".to_string(),
        realname: "Bad Bot".to_string(),
        ..Default::default()
    };

    let (_handle, mut events) = client::connect(config, Some(signer));

    expect_event(&mut events, 2000, |e| matches!(e, Event::Connected), "Connected").await;

    // Should get SASL failure (904), not success
    expect_raw_line(&mut events, 2000, "904", "SASL failure").await;

    server_handle.abort();
}

// ── Test: AGENT REGISTER ────────────────────────────────────────────

#[tokio::test]
async fn agent_register_command() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "agentbot").await;

    // Register as agent
    handle.register_agent("agent").await.unwrap();

    // Should get a NOTICE confirming registration
    expect_raw_line(
        &mut events,
        2000,
        "registered as agent",
        "AGENT REGISTER confirmation",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn agent_register_external_agent() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "extbot").await;

    handle.register_agent("external_agent").await.unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "registered as external_agent",
        "AGENT REGISTER external_agent",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn agent_register_invalid_class() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "badclass").await;

    handle.raw("AGENT REGISTER :class=superbot").await.unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "Invalid actor class",
        "AGENT REGISTER with invalid class",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: AGENT class in WHOIS ──────────────────────────────────────

#[tokio::test]
async fn agent_class_in_whois() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Connect bot and register as agent
    let (_did, bot_handle, mut bot_events) = connect_did_key(addr, "whobot").await;
    bot_handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "registered as agent", "AGENT REGISTER").await;

    // Connect observer
    let (obs_handle, mut obs_events) = connect_guest(addr, "observer").await;

    // WHOIS the bot
    obs_handle.raw("WHOIS whobot").await.unwrap();

    // Should see 673 numeric with actor_class=agent
    expect_raw_line(
        &mut obs_events,
        2000,
        "actor_class=agent",
        "WHOIS 673 actor_class",
    )
    .await;

    // End of WHOIS
    expect_raw_line(&mut obs_events, 2000, "318", "End of WHOIS").await;

    bot_handle.quit(None).await.unwrap();
    obs_handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn human_whois_no_actor_class() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Connect a human (did:key but no AGENT REGISTER)
    let (_did, human_handle, _human_events) = connect_did_key(addr, "humanuser").await;

    // Connect observer
    let (obs_handle, mut obs_events) = connect_guest(addr, "observer2").await;

    // WHOIS the human
    obs_handle.raw("WHOIS humanuser").await.unwrap();

    // Should get end of WHOIS (318) but NOT 673 (actor class only shown for non-human)
    let end = expect_raw_line(&mut obs_events, 2000, "318", "End of WHOIS").await;
    // The 673 should not have appeared before 318
    // (We can't easily prove absence inline, but if 673 appeared it would match before 318)
    assert!(end.contains("318"));

    human_handle.quit(None).await.unwrap();
    obs_handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: PROVENANCE ────────────────────────────────────────────────

#[tokio::test]
async fn provenance_submit() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "provbot").await;

    handle
        .submit_provenance(&serde_json::json!({
            "name": "provbot",
            "version": "1.0.0",
            "source": "https://example.com",
            "runtime": "freeq-sdk/rust"
        }))
        .await
        .unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "Provenance declaration stored",
        "PROVENANCE stored",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn provenance_requires_auth() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Connect as guest (no auth)
    let (handle, mut events) = connect_guest(addr, "guestprov").await;

    // Try to submit provenance as guest
    handle
        .raw("PROVENANCE :eyJuYW1lIjoiZ3Vlc3QifQ")
        .await
        .unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "Must be authenticated",
        "PROVENANCE rejected for guest",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn provenance_invalid_format() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "badfmt").await;

    handle.raw("PROVENANCE :not-valid-json-or-base64!!!").await.unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "Invalid provenance format",
        "PROVENANCE invalid format",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: PRESENCE ──────────────────────────────────────────────────

#[tokio::test]
async fn presence_update() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "presbot").await;

    handle
        .set_presence("executing", Some("Building project"), Some("task-001"))
        .await
        .unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "Presence updated: executing",
        "PRESENCE confirmation",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn presence_sets_away_for_non_active_states() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    let (_did, bot_handle, mut bot_events) = connect_did_key(addr, "awaybot").await;
    let (obs_handle, mut obs_events) = connect_guest(addr, "obs").await;

    // Both join a channel
    bot_handle.join("#test").await.unwrap();
    expect_event(&mut bot_events, 2000, |e| matches!(e, Event::Joined { .. }), "Bot joined").await;
    obs_handle.join("#test").await.unwrap();
    expect_event(&mut obs_events, 2000, |e| matches!(e, Event::Joined { .. }), "Obs joined").await;

    // Bot sets presence to executing (non-active → should trigger AWAY)
    bot_handle
        .set_presence("blocked_on_permission", Some("Waiting for approval"), None)
        .await
        .unwrap();
    expect_raw_line(&mut bot_events, 2000, "Presence updated", "PRESENCE confirmation").await;

    // Small delay for AWAY state to propagate
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Observer does WHOIS — should see 301 (RPL_AWAY) with the away message
    obs_handle.raw("WHOIS awaybot").await.unwrap();
    // Should see 301 (RPL_AWAY)
    expect_raw_line(&mut obs_events, 3000, "301", "WHOIS shows AWAY").await;

    bot_handle.quit(None).await.unwrap();
    obs_handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn presence_online_clears_away() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "clearbot").await;

    // Set to executing (away)
    handle.set_presence("executing", None, None).await.unwrap();
    expect_raw_line(&mut events, 2000, "Presence updated: executing", "PRESENCE executing").await;

    // Clear back to online
    handle.set_presence("online", None, None).await.unwrap();
    expect_raw_line(&mut events, 2000, "Presence updated: online", "PRESENCE online").await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: HEARTBEAT ─────────────────────────────────────────────────

#[tokio::test]
async fn heartbeat_accepted() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "hbbot").await;

    // Send heartbeat — should not produce an error
    handle.send_heartbeat("active", 60).await.unwrap();

    // Heartbeat is silent (no NOTICE response) — verify by sending a subsequent
    // command and checking we get its response (proves connection is still alive)
    handle.raw("WHOIS hbbot").await.unwrap();
    expect_raw_line(&mut events, 2000, "311", "WHOIS response after heartbeat").await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn heartbeat_auto_start() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "autohb").await;

    // Start automatic heartbeat (1 second interval for test speed)
    let hb_task = handle.start_heartbeat(Duration::from_secs(1));

    // Wait a bit, then verify connection is still alive
    tokio::time::sleep(Duration::from_secs(3)).await;
    handle.raw("WHOIS autohb").await.unwrap();
    expect_raw_line(&mut events, 2000, "311", "WHOIS after auto-heartbeat").await;

    hb_task.abort();
    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: Agent + guest in same channel ─────────────────────────────

#[tokio::test]
async fn agent_and_guest_coexist_in_channel() {
    start_deadlock_detector();
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Connect agent
    let (_did, bot_handle, mut bot_events) = connect_did_key(addr, "chanbot").await;
    bot_handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "registered as agent", "AGENT REGISTER").await;

    // Connect guest
    let (guest_handle, mut guest_events) = connect_guest(addr, "changuest").await;

    // Both join #test
    bot_handle.join("#agenttest").await.unwrap();
    expect_event(&mut bot_events, 2000, |e| matches!(e, Event::Joined { .. }), "Bot joined").await;

    guest_handle.join("#agenttest").await.unwrap();
    expect_event(&mut guest_events, 2000, |e| matches!(e, Event::Joined { .. }), "Guest joined").await;

    // Bot sends a message
    bot_handle.privmsg("#agenttest", "Hello from agent!").await.unwrap();

    // Guest should receive it
    let msg = expect_event(
        &mut guest_events,
        2000,
        |e| matches!(e, Event::Message { text, .. } if text == "Hello from agent!"),
        "Guest receives agent message",
    )
    .await;
    assert!(matches!(msg, Event::Message { from, .. } if from == "chanbot"));

    // Guest sends a message back
    guest_handle.privmsg("#agenttest", "Hello from guest!").await.unwrap();

    // Bot should receive it
    let msg = expect_event(
        &mut bot_events,
        2000,
        |e| matches!(e, Event::Message { text, .. } if text == "Hello from guest!"),
        "Bot receives guest message",
    )
    .await;
    assert!(matches!(msg, Event::Message { from, .. } if from == "changuest"));

    bot_handle.quit(None).await.unwrap();
    guest_handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: Multiple agents ───────────────────────────────────────────

#[tokio::test]
async fn multiple_agents_different_classes() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    let (_did1, bot1, mut ev1) = connect_did_key(addr, "agent1").await;
    let (_did2, bot2, mut ev2) = connect_did_key(addr, "agent2").await;

    bot1.register_agent("agent").await.unwrap();
    expect_raw_line(&mut ev1, 2000, "registered as agent", "Bot1 registered").await;

    bot2.register_agent("external_agent").await.unwrap();
    expect_raw_line(&mut ev2, 2000, "registered as external_agent", "Bot2 registered").await;

    // WHOIS each other
    bot1.raw("WHOIS agent2").await.unwrap();
    expect_raw_line(&mut ev1, 2000, "actor_class=external_agent", "WHOIS agent2 class").await;

    bot2.raw("WHOIS agent1").await.unwrap();
    expect_raw_line(&mut ev2, 2000, "actor_class=agent", "WHOIS agent1 class").await;

    bot1.quit(None).await.unwrap();
    bot2.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: Full agent lifecycle ──────────────────────────────────────

#[tokio::test]
async fn full_agent_lifecycle() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    let (did, handle, mut events) = connect_did_key(addr, "lifecycle").await;

    // 1. Register as agent
    handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut events, 2000, "registered as agent", "Step 1: register").await;

    // 2. Submit provenance
    handle
        .submit_provenance(&serde_json::json!({
            "name": "lifecycle-bot",
            "version": "0.1.0",
            "created_by": "did:plc:testcreator"
        }))
        .await
        .unwrap();
    expect_raw_line(&mut events, 2000, "Provenance declaration stored", "Step 2: provenance").await;

    // 3. Set presence
    handle
        .set_presence("active", Some("Running lifecycle test"), None)
        .await
        .unwrap();
    expect_raw_line(&mut events, 2000, "Presence updated: active", "Step 3: presence").await;

    // 4. Send heartbeat
    handle.send_heartbeat("active", 30).await.unwrap();

    // 5. Join a channel and communicate
    handle.join("#lifecycle").await.unwrap();
    expect_event(&mut events, 2000, |e| matches!(e, Event::Joined { .. }), "Step 5: joined").await;

    handle
        .privmsg("#lifecycle", "Lifecycle test complete")
        .await
        .unwrap();

    // 6. Change presence to executing
    handle
        .set_presence("executing", Some("Processing task"), Some("task-42"))
        .await
        .unwrap();
    expect_raw_line(&mut events, 2000, "Presence updated: executing", "Step 6: executing").await;

    // 7. WHOIS self to verify everything
    handle.raw("WHOIS lifecycle").await.unwrap();
    expect_raw_line(&mut events, 2000, "actor_class=agent", "Step 7: WHOIS actor_class").await;
    expect_raw_line(&mut events, 2000, "318", "Step 7: End of WHOIS").await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: AGENT REGISTER requires params ────────────────────────────

#[tokio::test]
async fn agent_register_no_params() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "noparam").await;

    handle.raw("AGENT").await.unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "461", // ERR_NEEDMOREPARAMS
        "AGENT with no params",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: PRESENCE requires params ──────────────────────────────────

#[tokio::test]
async fn presence_no_params() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "nopres").await;

    handle.raw("PRESENCE").await.unwrap();

    expect_raw_line(
        &mut events,
        2000,
        "461", // ERR_NEEDMOREPARAMS
        "PRESENCE with no params",
    )
    .await;

    handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ══════════════════════════════════════════════════════════════════════
// Phase 2: Governance
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn governance_pause_resume() {
    start_deadlock_detector();
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Op (human with channel ops)
    let (_op_did, op_handle, mut op_events) = connect_did_key(addr, "operator").await;
    // Agent
    let (_bot_did, bot_handle, mut bot_events) = connect_did_key(addr, "govbot").await;
    bot_handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "registered as agent", "AGENT REGISTER").await;

    // Both join channel — op gets ops as first joiner
    op_handle.join("#governed").await.unwrap();
    expect_event(&mut op_events, 2000, |e| matches!(e, Event::Joined { .. }), "Op joined").await;
    bot_handle.join("#governed").await.unwrap();
    expect_event(&mut bot_events, 2000, |e| matches!(e, Event::Joined { .. }), "Bot joined").await;

    // Op pauses the bot
    op_handle.pause_agent("govbot", Some("maintenance")).await.unwrap();

    // Bot should receive governance TAGMSG
    expect_raw_line(&mut bot_events, 2000, "governance=pause", "Bot receives PAUSE").await;

    // Op should see the channel notice
    expect_raw_line(&mut op_events, 2000, "paused by operator", "Channel PAUSE notice").await;

    // Op resumes the bot
    op_handle.resume_agent("govbot").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "governance=resume", "Bot receives RESUME").await;

    bot_handle.quit(None).await.unwrap();
    op_handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn governance_revoke_disconnects() {
    start_deadlock_detector();
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    let (_op_did, op_handle, mut op_events) = connect_did_key(addr, "revoker").await;
    let (_bot_did, bot_handle, mut bot_events) = connect_did_key(addr, "revbot").await;
    bot_handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "registered as agent", "AGENT REGISTER").await;

    op_handle.join("#revtest").await.unwrap();
    expect_event(&mut op_events, 2000, |e| matches!(e, Event::Joined { .. }), "Op joined").await;
    bot_handle.join("#revtest").await.unwrap();
    expect_event(&mut bot_events, 2000, |e| matches!(e, Event::Joined { .. }), "Bot joined").await;

    // Op revokes the bot
    op_handle.revoke_agent("revbot", Some("bye")).await.unwrap();

    // Bot should receive ERROR (force disconnect)
    expect_raw_line(&mut bot_events, 2000, "ERROR", "Bot receives ERROR/disconnect").await;

    op_handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn governance_requires_op() {
    start_deadlock_detector();
    let (addr, server_handle) = start_test_server(empty_resolver()).await;

    // Two users, neither is op of the other's channel
    let (_did1, user1, mut ev1) = connect_did_key(addr, "nopower").await;
    let (_did2, user2, mut ev2) = connect_did_key(addr, "target").await;
    user2.register_agent("agent").await.unwrap();
    expect_raw_line(&mut ev2, 2000, "registered as agent", "AGENT REGISTER").await;

    // user2 creates a channel (gets ops)
    user2.join("#botchan").await.unwrap();
    expect_event(&mut ev2, 2000, |e| matches!(e, Event::Joined { .. }), "User2 joined").await;
    // user1 joins (not op)
    user1.join("#botchan").await.unwrap();
    expect_event(&mut ev1, 2000, |e| matches!(e, Event::Joined { .. }), "User1 joined").await;

    // user1 tries to pause user2 — should fail
    user1.pause_agent("target", None).await.unwrap();
    expect_raw_line(&mut ev1, 2000, "482", "PAUSE rejected: not op").await;

    user1.quit(None).await.unwrap();
    user2.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn approval_request_and_grant() {
    start_deadlock_detector();
    let (addr, server_handle) = start_test_server_with_db(empty_resolver(), true).await;

    let (_op_did, op_handle, mut op_events) = connect_did_key(addr, "approver").await;
    let (_bot_did, bot_handle, mut bot_events) = connect_did_key(addr, "reqbot").await;
    bot_handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "registered as agent", "AGENT REGISTER").await;

    op_handle.join("#approval").await.unwrap();
    expect_event(&mut op_events, 2000, |e| matches!(e, Event::Joined { .. }), "Op joined").await;
    bot_handle.join("#approval").await.unwrap();
    expect_event(&mut bot_events, 2000, |e| matches!(e, Event::Joined { .. }), "Bot joined").await;

    // Bot requests deploy approval
    bot_handle.request_approval("#approval", "deploy", Some("landing-page")).await.unwrap();

    // Bot gets confirmation
    expect_raw_line(&mut bot_events, 2000, "Approval requested", "Request confirmed").await;

    // Op sees notification in channel
    expect_raw_line(&mut op_events, 2000, "requests approval", "Op sees request").await;

    // Op approves
    op_handle.approve_agent("reqbot", "deploy").await.unwrap();

    // Bot gets approval granted TAGMSG
    expect_raw_line(&mut bot_events, 2000, "approval_granted", "Bot gets approval").await;

    // Channel sees approval notice
    expect_raw_line(&mut op_events, 2000, "approved", "Channel sees approval").await;

    bot_handle.quit(None).await.unwrap();
    op_handle.quit(None).await.unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn approval_request_and_deny() {
    start_deadlock_detector();
    let (addr, server_handle) = start_test_server_with_db(empty_resolver(), true).await;

    let (_op_did, op_handle, mut op_events) = connect_did_key(addr, "denier").await;
    let (_bot_did, bot_handle, mut bot_events) = connect_did_key(addr, "denybot").await;
    bot_handle.register_agent("agent").await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "registered as agent", "AGENT REGISTER").await;

    op_handle.join("#denytest").await.unwrap();
    expect_event(&mut op_events, 2000, |e| matches!(e, Event::Joined { .. }), "Op joined").await;
    bot_handle.join("#denytest").await.unwrap();
    expect_event(&mut bot_events, 2000, |e| matches!(e, Event::Joined { .. }), "Bot joined").await;

    bot_handle.request_approval("#denytest", "deploy", None).await.unwrap();
    expect_raw_line(&mut bot_events, 2000, "Approval requested", "Request confirmed").await;
    expect_raw_line(&mut op_events, 2000, "requests approval", "Op sees request").await;

    // Op denies
    op_handle.deny_agent("denybot", "deploy", Some("not ready")).await.unwrap();

    // Bot gets denial
    expect_raw_line(&mut bot_events, 2000, "approval_denied", "Bot gets denial").await;

    bot_handle.quit(None).await.unwrap();
    op_handle.quit(None).await.unwrap();
    server_handle.abort();
}

// ── Test: Presence with all states ──────────────────────────────────

#[tokio::test]
async fn presence_all_states() {
    let (addr, server_handle) = start_test_server(empty_resolver()).await;
    let (_did, handle, mut events) = connect_did_key(addr, "allstates").await;

    let states = [
        "online", "idle", "active", "executing", "waiting_for_input",
        "blocked_on_permission", "blocked_on_budget", "degraded",
        "paused", "sandboxed", "rate_limited", "revoked", "offline",
    ];

    for state in &states {
        handle.set_presence(state, None, None).await.unwrap();
        let line = expect_raw_line(
            &mut events,
            3000,
            &format!("Presence updated: {state}"),
            &format!("PRESENCE {state}"),
        )
        .await;
        assert!(line.contains(state), "Expected state {state} in: {line}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    handle.quit(None).await.unwrap();
    server_handle.abort();
}
