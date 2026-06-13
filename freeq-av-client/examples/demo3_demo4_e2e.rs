//! E2E tests for Demo 3 (P2P native clients) and Demo 4 (mixed call).
//!
//! Tests against a running freeq-server with av-native and iroh.
//!
//! Usage:
//!   # Local:
//!   cargo run --example demo3_demo4_e2e -- --irc-addr 127.0.0.1:16667 --web-url http://127.0.0.1:18080
//!   # Staging:
//!   cargo run --example demo3_demo4_e2e -- --web-url https://staging.freeq.at

use anyhow::{Result, bail};
use std::time::Duration;
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("demo3_demo4_e2e=info".parse()?)
                .add_directive("iroh_live=info".parse()?)
                .add_directive("moq=info".parse()?)
                .add_directive("warn".parse()?),
        )
        .init();

    let irc_addr = std::env::args()
        .position(|a| a == "--irc-addr")
        .and_then(|i| std::env::args().nth(i + 1));
    let web_url = std::env::args()
        .position(|a| a == "--web-url")
        .and_then(|i| std::env::args().nth(i + 1))
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    println!("\n=== Demo 3 & 4 E2E Tests ===");
    println!("  Web: {web_url}\n");

    // ── Demo 3: Two native clients P2P ────────────────────────────
    println!("━━━ Demo 3: Two native clients P2P ━━━\n");
    let demo3 = run_demo3(&irc_addr, &web_url).await;
    match &demo3 {
        Ok(true) => println!("\n  Demo 3: PASS\n"),
        Ok(false) => println!("\n  Demo 3: FAIL\n"),
        Err(e) => println!("\n  Demo 3: ERROR — {e}\n"),
    }

    // ── Demo 4: Mixed call (2 browsers + 1 native) ───────────────
    println!("━━━ Demo 4: Mixed call (2 browsers + 1 native) ━━━\n");
    let demo4 = run_demo4(&irc_addr, &web_url).await;
    match &demo4 {
        Ok(true) => println!("\n  Demo 4: PASS\n"),
        Ok(false) => println!("\n  Demo 4: FAIL\n"),
        Err(e) => println!("\n  Demo 4: ERROR — {e}\n"),
    }

    // ── Demo 5: Latecomer joins mid-call ────────────────────────
    println!("━━━ Demo 5: Latecomer joins mid-call ━━━\n");
    let demo5 = run_demo5(&irc_addr, &web_url).await;
    match &demo5 {
        Ok(true) => println!("\n  Demo 5: PASS\n"),
        Ok(false) => println!("\n  Demo 5: FAIL\n"),
        Err(e) => println!("\n  Demo 5: ERROR — {e}\n"),
    }

    // ── Demo 6: Ungraceful disconnect ─────────────────────────────
    println!("━━━ Demo 6: Ungraceful disconnect ━━━\n");
    let demo6 = run_demo6(&irc_addr, &web_url).await;
    match &demo6 {
        Ok(true) => println!("\n  Demo 6: PASS\n"),
        Ok(false) => println!("\n  Demo 6: FAIL\n"),
        Err(e) => println!("\n  Demo 6: ERROR — {e}\n"),
    }

    // ── Summary ───────────────────────────────────────────────────
    println!("━━━ Results ━━━");
    let d3 = demo3.is_ok_and(|b| b);
    let d4 = demo4.is_ok_and(|b| b);
    let d5 = demo5.is_ok_and(|b| b);
    let d6 = demo6.is_ok_and(|b| b);
    println!(
        "  Demo 3 (P2P native clients): {}",
        if d3 { "PASS" } else { "FAIL" }
    );
    println!(
        "  Demo 4 (mixed call):         {}",
        if d4 { "PASS" } else { "FAIL" }
    );
    println!(
        "  Demo 5 (latecomer):          {}",
        if d5 { "PASS" } else { "FAIL" }
    );
    println!(
        "  Demo 6 (disconnect):         {}",
        if d6 { "PASS" } else { "FAIL" }
    );
    if d3 && d4 && d5 && d6 {
        println!("\n  ALL PASS\n");
    } else {
        println!("\n  SOME FAILURES\n");
    }

    Ok(())
}

/// Demo 3: Start an AV session, have two iroh-live Room clients exchange audio.
/// The server bridges MoQ↔Room but the two native clients talk through the Room directly.
async fn run_demo3(irc_addr: &Option<String>, web_url: &str) -> Result<bool> {
    // Start session via IRC
    println!("[1/4] Starting AV session...");
    let (session_id, iroh_ticket) = start_av_session(irc_addr, web_url, "d3-starter").await?;
    println!("  Session: {session_id}");

    let room_ticket: iroh_live::rooms::RoomTicket = iroh_ticket
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid ticket: {e}"))?;

    // Native client A joins Room
    println!("[2/4] Native client A joining Room...");
    let ep_a = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let live_a = iroh_live::Live::builder(ep_a)
        .with_router()
        .with_gossip()
        .spawn();
    let room_a = iroh_live::rooms::Room::new(&live_a, room_ticket.clone()).await?;
    let (mut events_a, handle_a) = room_a.split();
    handle_a.set_display_name("alice").await?;

    // Publish synthetic audio from A
    let mut prod_a = moq_lite::Broadcast::produce();
    let ct = moq_lite::Track::new("catalog.json");
    let mut cw = prod_a.create_track(ct)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from_static(
        b"{\"audio\":{\"renditions\":{\"audio\":{\"codec\":\"opus\",\"sampleRate\":48000,\"numberOfChannels\":1,\"bitrate\":128000,\"container\":{\"kind\":\"legacy\"}}}}}",
    ))?;
    g.finish().ok();
    let at = moq_lite::Track::new("audio");
    let mut aw = prod_a.create_track(at)?;
    for seq in 0..5u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xAAu8; 960]))?;
        g.finish().ok();
    }
    handle_a.publish_producer("alice", prod_a.clone()).await?;
    println!("  Alice published audio to Room");

    // Native client B joins same Room
    println!("[3/4] Native client B joining Room...");
    let ep_b = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let live_b = iroh_live::Live::builder(ep_b)
        .with_router()
        .with_gossip()
        .spawn();
    let room_b = iroh_live::rooms::Room::new(&live_b, room_ticket).await?;
    let (mut events_b, handle_b) = room_b.split();
    handle_b.set_display_name("bob").await?;

    // Publish from B
    let mut prod_b = moq_lite::Broadcast::produce();
    let ct = moq_lite::Track::new("catalog.json");
    let mut cw2 = prod_b.create_track(ct)?;
    let mut g = cw2.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from_static(
        b"{\"audio\":{\"renditions\":{\"audio\":{\"codec\":\"opus\",\"sampleRate\":48000,\"numberOfChannels\":1,\"bitrate\":128000,\"container\":{\"kind\":\"legacy\"}}}}}",
    ))?;
    g.finish().ok();
    let at = moq_lite::Track::new("audio");
    let mut aw2 = prod_b.create_track(at)?;
    for seq in 0..5u64 {
        let mut g = aw2.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xBBu8; 480]))?;
        g.finish().ok();
    }
    handle_b.publish_producer("bob", prod_b.clone()).await?;
    println!("  Bob published audio to Room");

    // Check: A sees B's broadcast, B sees A's broadcast
    println!("[4/4] Checking P2P audio exchange...");

    let a_sees_b = timeout(TIMEOUT, async {
        loop {
            match events_a.recv().await {
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    if name.contains("bob") {
                        let consumer = broadcast.consumer().clone();
                        let at = moq_lite::Track::new("audio");
                        if let Ok(mut track) = consumer.subscribe_track(&at) {
                            if let Ok(Some(mut group)) = track.next_group().await {
                                if let Ok(Some(frame)) = group.read_frame().await {
                                    println!(
                                        "  Alice got Bob's audio: {} bytes (0x{:02X})",
                                        frame.len(),
                                        frame[0]
                                    );
                                    return frame[0] == 0xBB;
                                }
                            }
                        }
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    let b_sees_a = timeout(TIMEOUT, async {
        loop {
            match events_b.recv().await {
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    if name.contains("alice") {
                        let consumer = broadcast.consumer().clone();
                        let at = moq_lite::Track::new("audio");
                        if let Ok(mut track) = consumer.subscribe_track(&at) {
                            if let Ok(Some(mut group)) = track.next_group().await {
                                if let Ok(Some(frame)) = group.read_frame().await {
                                    println!(
                                        "  Bob got Alice's audio: {} bytes (0x{:02X})",
                                        frame.len(),
                                        frame[0]
                                    );
                                    return frame[0] == 0xAA;
                                }
                            }
                        }
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    println!("  Alice sees Bob: {}", if a_sees_b { "YES" } else { "NO" });
    println!("  Bob sees Alice: {}", if b_sees_a { "YES" } else { "NO" });

    live_a.shutdown().await;
    live_b.shutdown().await;

    Ok(a_sees_b && b_sees_a)
}

/// Demo 4: Mixed call — 2 MoQ WebSocket "browsers" + 1 iroh Room "native" client.
/// Verify: browsers see each other (MoQ), native sees browsers (MoQ→Room bridge),
/// browsers see native (Room→MoQ bridge).
async fn run_demo4(irc_addr: &Option<String>, web_url: &str) -> Result<bool> {
    let moq_url: url::Url = format!("{web_url}/av/moq").parse()?;

    // Start session
    println!("[1/5] Starting AV session...");
    let (session_id, iroh_ticket) = start_av_session(irc_addr, web_url, "d4-start").await?;
    println!("  Session: {session_id}");

    // Browser A publishes to MoQ
    println!("[2/5] Browser A publishing to MoQ...");
    let _browser_a = publish_moq(&moq_url, &format!("{session_id}/browser-alice"), 0xA1).await?;
    println!("  Browser Alice published");

    // Browser B publishes to MoQ
    println!("[3/5] Browser B publishing to MoQ...");
    let _browser_b = publish_moq(&moq_url, &format!("{session_id}/browser-bob"), 0xB2).await?;
    println!("  Browser Bob published");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Native client joins Room
    println!("[4/5] Native client joining Room...");
    let room_ticket: iroh_live::rooms::RoomTicket = iroh_ticket
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid ticket: {e}"))?;
    let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let live = iroh_live::Live::builder(ep)
        .with_router()
        .with_gossip()
        .spawn();
    let room = iroh_live::rooms::Room::new(&live, room_ticket).await?;
    let (mut events, handle) = room.split();
    handle.set_display_name("native-carol").await?;

    // Native publishes to Room
    let mut prod_n = moq_lite::Broadcast::produce();
    let ct = moq_lite::Track::new("catalog.json");
    let mut cw = prod_n.create_track(ct)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from_static(
        b"{\"audio\":{\"renditions\":{\"audio\":{\"codec\":\"opus\",\"sampleRate\":48000,\"numberOfChannels\":1,\"bitrate\":128000,\"container\":{\"kind\":\"legacy\"}}}}}",
    ))?;
    g.finish().ok();
    let at = moq_lite::Track::new("audio");
    let mut aw = prod_n.create_track(at)?;
    for seq in 0..5u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xC3u8; 480]))?;
        g.finish().ok();
    }
    handle
        .publish_producer("native-carol", prod_n.clone())
        .await?;
    println!("  Native Carol published to Room");

    // Check: native sees both browsers via bridge (MoQ→Room)
    println!("[5/5] Checking mixed-call audio routing...");

    let mut native_saw = std::collections::HashSet::new();
    let native_result = timeout(TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    println!("  Native saw broadcast: {name}");
                    let consumer = broadcast.consumer().clone();
                    let at = moq_lite::Track::new("audio");
                    if let Ok(mut track) = consumer.subscribe_track(&at) {
                        if let Ok(Some(mut group)) = track.next_group().await {
                            if let Ok(Some(frame)) = group.read_frame().await {
                                println!(
                                    "  Native got audio from {name}: {} bytes (0x{:02X})",
                                    frame.len(),
                                    frame[0]
                                );
                                native_saw.insert(name);
                            }
                        }
                    }
                    if native_saw.len() >= 2 {
                        return true;
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    // Check: MoQ subscriber sees all 3 broadcasts (browsers + native via bridge)
    let (_sub_session, mut sub_consumer) = subscribe_moq(&moq_url).await?;
    let mut moq_saw = std::collections::HashSet::new();
    let moq_result = timeout(TIMEOUT, async {
        while let Some((path, announce)) = sub_consumer.announced().await {
            let path_str = path.to_string();
            if path_str.starts_with(&session_id) && announce.is_some() {
                println!("  MoQ subscriber saw: {path_str}");
                moq_saw.insert(path_str);
                if moq_saw.len() >= 3 {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    let alice_path = format!("{session_id}/browser-alice");
    let bob_path = format!("{session_id}/browser-bob");
    let carol_path = format!("{session_id}/native-carol");

    println!(
        "\n  Native sees browsers (MoQ→Room): {}",
        if native_result { "YES" } else { "NO" }
    );
    println!(
        "  MoQ has all 3 broadcasts:        {}",
        if moq_result { "YES" } else { "NO" }
    );
    if moq_result {
        println!(
            "    alice:  {}",
            if moq_saw.contains(&alice_path) {
                "YES"
            } else {
                "NO"
            }
        );
        println!(
            "    bob:    {}",
            if moq_saw.contains(&bob_path) {
                "YES"
            } else {
                "NO"
            }
        );
        println!(
            "    carol:  {}",
            if moq_saw.contains(&carol_path) {
                "YES"
            } else {
                "NO"
            }
        );
    }

    live.shutdown().await;

    Ok(native_result && moq_result)
}

/// Demo 5: Latecomer joins mid-call.
/// Browser A publishes to MoQ, then native B joins late and sees A's broadcast
/// through the bridge. Then MoQ subscriber C joins late and sees both A (direct)
/// and B (via Room→MoQ bridge).
async fn run_demo5(irc_addr: &Option<String>, web_url: &str) -> Result<bool> {
    let moq_url: url::Url = format!("{web_url}/av/moq").parse()?;

    // Start session
    println!("[1/4] Starting AV session...");
    let (session_id, iroh_ticket) = start_av_session(irc_addr, web_url, "d5-start").await?;
    println!("  Session: {session_id}");

    // Browser A publishes FIRST
    println!("[2/4] Browser A publishes (early bird)...");
    let _browser_a = publish_moq(&moq_url, &format!("{session_id}/early-alice"), 0xA1).await?;
    println!("  Alice published");

    // Wait to simulate mid-call delay
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Native B joins LATE
    println!("[3/4] Native B joins late...");
    let room_ticket: iroh_live::rooms::RoomTicket = iroh_ticket
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid ticket: {e}"))?;
    let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let live = iroh_live::Live::builder(ep)
        .with_router()
        .with_gossip()
        .spawn();
    let room = iroh_live::rooms::Room::new(&live, room_ticket).await?;
    let (mut events, handle) = room.split();
    handle.set_display_name("late-bob").await?;

    // Publish from B (so we can test the reverse too)
    let mut prod_b = moq_lite::Broadcast::produce();
    let ct = moq_lite::Track::new("catalog.json");
    let mut cw = prod_b.create_track(ct)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from_static(
        b"{\"audio\":{\"renditions\":{\"audio\":{\"codec\":\"opus\",\"sampleRate\":48000,\"numberOfChannels\":1,\"bitrate\":128000,\"container\":{\"kind\":\"legacy\"}}}}}",
    ))?;
    g.finish().ok();
    let at = moq_lite::Track::new("audio");
    let mut aw = prod_b.create_track(at)?;
    for seq in 0..5u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xB2u8; 480]))?;
        g.finish().ok();
    }
    handle.publish_producer("late-bob", prod_b.clone()).await?;

    // Check: late native B sees early browser A via bridge
    println!("[4/4] Checking latecomer sees existing broadcast...");
    let native_sees_alice = timeout(TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    if name.contains("early-alice") {
                        let consumer = broadcast.consumer().clone();
                        let at = moq_lite::Track::new("audio");
                        if let Ok(mut track) = consumer.subscribe_track(&at) {
                            if let Ok(Some(mut group)) = track.next_group().await {
                                if let Ok(Some(frame)) = group.read_frame().await {
                                    println!(
                                        "  Late Bob got early Alice's audio: {} bytes (0x{:02X})",
                                        frame.len(),
                                        frame[0]
                                    );
                                    return frame[0] == 0xA1;
                                }
                            }
                        }
                        return false;
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    // Check: MoQ subscriber C joining even later sees both Alice and Bob
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (_sub_session, mut sub_consumer) = subscribe_moq(&moq_url).await?;
    let mut late_moq_saw = std::collections::HashSet::new();
    let moq_sees_both = timeout(TIMEOUT, async {
        while let Some((path, announce)) = sub_consumer.announced().await {
            let p = path.to_string();
            if p.starts_with(&session_id) && announce.is_some() {
                late_moq_saw.insert(p);
                if late_moq_saw.len() >= 2 {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    println!(
        "  Late native sees early browser: {}",
        if native_sees_alice { "YES" } else { "NO" }
    );
    println!(
        "  Late MoQ subscriber sees both:  {} ({:?})",
        if moq_sees_both { "YES" } else { "NO" },
        late_moq_saw
    );

    live.shutdown().await;
    Ok(native_sees_alice && moq_sees_both)
}

/// Demo 6: Ungraceful disconnect — drop a participant and verify others continue.
/// Browser A + Native B in call. Drop A. Verify B's broadcast is still in the
/// MoQ cluster and the session doesn't crash.
async fn run_demo6(irc_addr: &Option<String>, web_url: &str) -> Result<bool> {
    let moq_url: url::Url = format!("{web_url}/av/moq").parse()?;

    println!("[1/4] Starting AV session...");
    let (session_id, iroh_ticket) = start_av_session(irc_addr, web_url, "d6-start").await?;
    println!("  Session: {session_id}");

    // Browser A publishes
    println!("[2/4] Browser A publishing...");
    let browser_a = publish_moq(&moq_url, &format!("{session_id}/browser-alice"), 0xA1).await?;
    println!("  Alice published");

    // Native B joins and publishes
    println!("[3/4] Native B joining...");
    let room_ticket: iroh_live::rooms::RoomTicket = iroh_ticket
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid ticket: {e}"))?;
    let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let live = iroh_live::Live::builder(ep)
        .with_router()
        .with_gossip()
        .spawn();
    let room = iroh_live::rooms::Room::new(&live, room_ticket).await?;
    let (_events, handle) = room.split();
    handle.set_display_name("native-bob").await?;

    let mut prod_b = moq_lite::Broadcast::produce();
    let ct = moq_lite::Track::new("catalog.json");
    let mut cw = prod_b.create_track(ct)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from_static(
        b"{\"audio\":{\"renditions\":{\"audio\":{\"codec\":\"opus\",\"sampleRate\":48000,\"numberOfChannels\":1,\"bitrate\":128000,\"container\":{\"kind\":\"legacy\"}}}}}",
    ))?;
    g.finish().ok();
    let at = moq_lite::Track::new("audio");
    let mut aw = prod_b.create_track(at)?;
    for seq in 0..5u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xB2u8; 480]))?;
        g.finish().ok();
    }
    handle
        .publish_producer("native-bob", prod_b.clone())
        .await?;
    println!("  Bob published");

    // Wait for bridge to settle
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify both visible before disconnect
    let (_sub1, mut sub1_consumer) = subscribe_moq(&moq_url).await?;
    let mut before = std::collections::HashSet::new();
    timeout(Duration::from_secs(5), async {
        while let Some((path, announce)) = sub1_consumer.announced().await {
            let p = path.to_string();
            if p.starts_with(&session_id) && announce.is_some() {
                before.insert(p);
                if before.len() >= 2 {
                    break;
                }
            }
        }
    })
    .await
    .ok();
    println!("  Before disconnect: {:?}", before);

    // ── DROP browser A (simulates ungraceful disconnect) ──────────
    println!("[4/4] Dropping Browser A (ungraceful disconnect)...");
    drop(browser_a);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify: Bob is still visible in MoQ cluster
    let bob_path = format!("{session_id}/native-bob");
    let (_sub2, mut sub2_consumer) = subscribe_moq(&moq_url).await?;
    let bob_still_there = timeout(Duration::from_secs(10), async {
        while let Some((path, announce)) = sub2_consumer.announced().await {
            let p = path.to_string();
            if p == bob_path && announce.is_some() {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    // Verify: session API still works
    let session_ok = reqwest::get(format!("{web_url}/api/v1/sessions/{session_id}"))
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    println!(
        "  Bob still in MoQ cluster: {}",
        if bob_still_there { "YES" } else { "NO" }
    );
    println!(
        "  Session API still works:  {}",
        if session_ok { "YES" } else { "NO" }
    );

    live.shutdown().await;
    Ok(bob_still_there && session_ok)
}

// ── Helpers ───────────────────────────────────────────────────────

async fn start_av_session(
    irc_addr: &Option<String>,
    web_url: &str,
    nick: &str,
) -> Result<(String, String)> {
    match irc_addr {
        Some(addr) => start_av_session_tcp(addr, nick).await,
        None => start_av_session_ws(web_url, nick).await,
    }
}

async fn start_av_session_tcp(addr: &str, nick: &str) -> Result<(String, String)> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let channel = &format!(
        "#d-{}-{}",
        std::process::id(),
        nick.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    );
    let stream = TcpStream::connect(addr).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    writer
        .write_all(format!("NICK {nick}\r\nUSER {nick} 0 * :test\r\n").as_bytes())
        .await?;

    let mut registered = false;
    let mut ticket: Option<String> = None;
    let mut session_id: Option<String> = None;

    let result = timeout(Duration::from_secs(10), async {
        while let Some(line) = lines.next_line().await? {
            if line.starts_with("PING") {
                writer
                    .write_all(format!("{}\r\n", line.replace("PING", "PONG")).as_bytes())
                    .await?;
                continue;
            }
            if !registered && line.contains(" 001 ") {
                registered = true;
                writer
                    .write_all(format!("CAP REQ :message-tags\r\nJOIN {channel}\r\n").as_bytes())
                    .await?;
                tokio::time::sleep(Duration::from_millis(200)).await;
                writer
                    .write_all(format!("@+freeq.at/av-start TAGMSG {channel}\r\n").as_bytes())
                    .await?;
            }
            if line.contains("AV session started:") {
                if let Some(id) = line.split("AV session started: ").nth(1) {
                    session_id = Some(id.trim().to_string());
                }
            }
            if line.contains("AV ticket:") {
                if let Some(t) = line.split("AV ticket: ").nth(1) {
                    ticket = Some(t.trim().to_string());
                }
            }
            if ticket.is_some() && session_id.is_some() {
                break;
            }
        }
        Ok::<_, anyhow::Error>((session_id, ticket))
    })
    .await??;

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let _ = writer.write_all(b"PING :k\r\n").await;
        }
    });

    match result {
        (Some(s), Some(t)) => Ok((s, t)),
        _ => bail!("Failed to get session ID and ticket"),
    }
}

async fn start_av_session_ws(web_url: &str, nick: &str) -> Result<(String, String)> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let channel = &format!(
        "#d-{}-{}",
        std::process::id(),
        nick.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    );
    let ws_url = if web_url.starts_with("https://") {
        format!(
            "wss://{}/irc",
            web_url.trim_start_matches("https://").trim_end_matches('/')
        )
    } else {
        format!(
            "ws://{}/irc",
            web_url.trim_start_matches("http://").trim_end_matches('/')
        )
    };

    let (ws, _) = tokio_tungstenite::connect_async(&ws_url).await?;
    let (mut ws_write, mut ws_read) = ws.split();

    ws_write
        .send(Message::Text(
            format!("NICK {nick}\r\nUSER {nick} 0 * :test\r\n").into(),
        ))
        .await?;

    let mut registered = false;
    let mut ticket: Option<String> = None;
    let mut session_id: Option<String> = None;

    let result = timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws_read.next().await {
            let text = match msg? {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with("PING") {
                    ws_write
                        .send(Message::Text(
                            format!("{}\r\n", line.replace("PING", "PONG")).into(),
                        ))
                        .await?;
                    continue;
                }
                if !registered && line.contains(" 001 ") {
                    registered = true;
                    ws_write
                        .send(Message::Text(
                            format!("CAP REQ :message-tags\r\nJOIN {channel}\r\n").into(),
                        ))
                        .await?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    ws_write
                        .send(Message::Text(
                            format!("@+freeq.at/av-start TAGMSG {channel}\r\n").into(),
                        ))
                        .await?;
                }
                if line.contains("AV session started:") {
                    if let Some(id) = line.split("AV session started: ").nth(1) {
                        session_id = Some(id.trim().to_string());
                    }
                }
                if line.contains("AV ticket:") {
                    if let Some(t) = line.split("AV ticket: ").nth(1) {
                        ticket = Some(t.trim().to_string());
                    }
                }
                if ticket.is_some() && session_id.is_some() {
                    break;
                }
            }
            if ticket.is_some() && session_id.is_some() {
                break;
            }
        }
        Ok::<_, anyhow::Error>((session_id, ticket))
    })
    .await??;

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if ws_write
                .send(Message::Text("PING :k\r\n".into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    match result {
        (Some(s), Some(t)) => Ok((s, t)),
        _ => bail!("Failed to get session ID and ticket"),
    }
}

struct MoqHandle {
    _session: moq_lite::Session,
    _origin: moq_lite::OriginProducer,
    _producer: moq_lite::BroadcastProducer,
    _catalog_track: moq_lite::TrackProducer,
    _audio_track: moq_lite::TrackProducer,
}

async fn publish_moq(moq_url: &url::Url, broadcast_name: &str, marker: u8) -> Result<MoqHandle> {
    let mut cfg = moq_native::ClientConfig::default();
    cfg.tls.disable_verify = Some(true);
    cfg.backend = Some(moq_native::QuicBackend::Noq);
    let client = cfg.init()?;

    let hang_catalog = r#"{"audio":{"renditions":{"audio":{"codec":"opus","sampleRate":48000,"numberOfChannels":2,"bitrate":128000,"container":{"kind":"legacy"}}}}}"#;

    let mut producer = moq_lite::Broadcast::produce();
    let ct = moq_lite::Track::new("catalog.json");
    let mut cw = producer.create_track(ct)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from(
        hang_catalog.as_bytes().to_vec(),
    ))?;
    g.finish().ok();

    let at = moq_lite::Track::new("audio");
    let mut aw = producer.create_track(at)?;
    for seq in 0..10u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![marker; 960]))?;
        g.finish().ok();
    }

    let origin = moq_lite::Origin::produce();
    origin.publish_broadcast(broadcast_name, producer.consume());
    let sub_origin = moq_lite::Origin::produce();

    let session = client
        .with_publish(origin.consume())
        .with_consume(sub_origin)
        .connect(moq_url.clone())
        .await?;

    Ok(MoqHandle {
        _session: session,
        _origin: origin,
        _producer: producer,
        _catalog_track: cw,
        _audio_track: aw,
    })
}

async fn subscribe_moq(
    moq_url: &url::Url,
) -> Result<(moq_lite::Session, moq_lite::OriginConsumer)> {
    let mut cfg = moq_native::ClientConfig::default();
    cfg.tls.disable_verify = Some(true);
    cfg.backend = Some(moq_native::QuicBackend::Noq);
    let client = cfg.init()?;

    let pub_origin = moq_lite::Origin::produce();
    let sub_origin = moq_lite::Origin::produce();
    let sub_consumer = sub_origin.consume();

    let session = client
        .with_publish(pub_origin.consume())
        .with_consume(sub_origin)
        .connect(moq_url.clone())
        .await?;

    Ok((session, sub_consumer))
}
