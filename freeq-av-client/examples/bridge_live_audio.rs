//! RED TEST: Simulates live browser audio through the bridge.
//!
//! Unlike bridge_e2e which pre-writes all frames before subscribing,
//! this test streams frames continuously (like a real browser mic)
//! and verifies a Room client receives them through the bridge.
//!
//! This should expose timing bugs where:
//! - catalog.json track finishes before Room client subscribes
//! - audio groups arrive too late for the bridge to forward
//! - the bridge's dynamic forwarding doesn't handle live streams
//!
//! Usage:
//!   cargo run --example bridge_live_audio -- --irc-addr 127.0.0.1:16667 --web-url http://127.0.0.1:18080

use anyhow::{Result, bail};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::timeout;

const TEST_DURATION: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bridge_live_audio=info".parse()?)
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
        .unwrap_or_else(|| "http://127.0.0.1:18080".to_string());

    println!("\n=== Bridge Live Audio Test ===");
    println!("  Simulates a real browser publishing live audio\n");

    // ── Step 1: Start session ─────────────────────────────────
    println!("[1/5] Starting AV session...");
    let (session_id, iroh_ticket) = match &irc_addr {
        Some(addr) => start_session_tcp(addr).await?,
        None => start_session_ws(&web_url).await?,
    };
    println!("  Session: {session_id}");

    // ── Step 2: Start LIVE streaming to MoQ (simulates browser) ──
    println!("[2/5] Starting live audio stream to MoQ...");
    let moq_url: url::Url = format!("{web_url}/av/moq").parse()?;
    let broadcast_name = format!("{session_id}/live-browser");

    let hang_catalog = r#"{"audio":{"renditions":{"audio":{"codec":"opus","sampleRate":48000,"numberOfChannels":2,"bitrate":128000,"container":{"kind":"legacy"}}}}}"#;

    let mut producer = moq_lite::Broadcast::produce();
    let catalog_track = moq_lite::Track::new("catalog.json");
    let mut cw = producer.create_track(catalog_track)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from(
        hang_catalog.as_bytes().to_vec(),
    ))?;
    g.finish().ok();

    // Audio track — write groups continuously in background (like real mic)
    let audio_track = moq_lite::Track::new("audio");
    let mut aw = producer.create_track(audio_track)?;
    let frames_written = Arc::new(AtomicU64::new(0));
    let fw = frames_written.clone();

    tokio::spawn(async move {
        let mut seq = 0u64;
        loop {
            let mut g = match aw.create_group(moq_lite::Group { sequence: seq }) {
                Ok(g) => g,
                Err(_) => break,
            };
            // 960 bytes = 20ms Opus frame at 48kHz
            if g.write_frame(moq_lite::bytes::Bytes::from(vec![0xABu8; 960]))
                .is_err()
            {
                break;
            }
            g.finish().ok();
            fw.fetch_add(1, Ordering::Relaxed);
            seq += 1;
            // Real mic sends frames every 20ms
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    // Connect to MoQ SFU
    let mut cfg = moq_native::ClientConfig::default();
    cfg.tls.disable_verify = Some(true);
    cfg.backend = Some(moq_native::QuicBackend::Noq);
    let client = cfg.init()?;

    let origin = moq_lite::Origin::produce();
    origin.publish_broadcast(&broadcast_name, producer.consume());
    let sub_origin = moq_lite::Origin::produce();

    let _session = client
        .with_publish(origin.consume())
        .with_consume(sub_origin)
        .connect(moq_url)
        .await?;
    println!("  Live streaming as: {broadcast_name}");

    // Wait for bridge to notice and forward
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!(
        "  Frames written so far: {}",
        frames_written.load(Ordering::Relaxed)
    );

    // ── Step 3: Join Room as native client (AFTER stream started) ──
    println!("[3/5] Native client joining Room (latecomer)...");
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
    handle.set_display_name("native-listener").await?;

    // ── Step 4: Wait for bridged broadcast in Room ────────────
    println!("[4/5] Waiting for live audio via bridge...");
    let frames_received = Arc::new(AtomicU64::new(0));
    let fr = frames_received.clone();

    let receive_result = timeout(TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    println!("  Broadcast received: {name}");

                    if !name.contains("live-browser") {
                        continue;
                    }

                    let consumer = broadcast.consumer().clone();

                    // Read catalog
                    let ct = moq_lite::Track::new("catalog.json");
                    match consumer.subscribe_track(&ct) {
                        Ok(mut track) => {
                            match timeout(Duration::from_secs(5), track.next_group()).await {
                                Ok(Ok(Some(mut group))) => {
                                    if let Ok(Some(frame)) = group.read_frame().await {
                                        let text = String::from_utf8_lossy(&frame);
                                        println!("  catalog.json: {text}");
                                    }
                                }
                                Ok(Ok(None)) => println!("  catalog.json: no groups"),
                                Ok(Err(e)) => println!("  catalog.json error: {e}"),
                                Err(_) => println!("  catalog.json: timeout"),
                            }
                        }
                        Err(e) => {
                            println!("  FAIL: catalog.json subscribe failed: {e}");
                            return false;
                        }
                    }

                    // Read live audio frames
                    let at = moq_lite::Track::new("audio");
                    match consumer.subscribe_track(&at) {
                        Ok(mut track) => {
                            println!("  Subscribed to audio track, reading live frames...");
                            let deadline = tokio::time::Instant::now() + TEST_DURATION;
                            while tokio::time::Instant::now() < deadline {
                                match timeout(Duration::from_secs(3), track.next_group()).await {
                                    Ok(Ok(Some(mut group))) => {
                                        while let Ok(Some(frame)) = group.read_frame().await {
                                            let count = fr.fetch_add(1, Ordering::Relaxed);
                                            if count == 0 {
                                                println!(
                                                    "  First audio frame: {} bytes (0x{:02X})",
                                                    frame.len(),
                                                    frame[0]
                                                );
                                            }
                                        }
                                    }
                                    Ok(Ok(None)) => {
                                        println!("  Audio track ended");
                                        break;
                                    }
                                    Ok(Err(e)) => {
                                        println!("  Audio error: {e}");
                                        break;
                                    }
                                    Err(_) => {
                                        println!("  Audio timeout (no frames for 3s)");
                                        break;
                                    }
                                }
                            }
                            return true;
                        }
                        Err(e) => {
                            println!("  FAIL: audio subscribe failed: {e}");
                            return false;
                        }
                    }
                }
                Some(iroh_live::rooms::RoomEvent::PeerJoined { display_name, .. }) => {
                    println!("  Peer joined: {}", display_name.as_deref().unwrap_or("?"));
                }
                Some(_) => {}
                None => {
                    println!("  Room events closed");
                    return false;
                }
            }
        }
    })
    .await;

    // ── Step 5: Results ───────────────────────────────────────
    println!("\n[5/5] Results:");
    let written = frames_written.load(Ordering::Relaxed);
    let received = frames_received.load(Ordering::Relaxed);
    println!("  Frames written (browser→MoQ): {written}");
    println!("  Frames received (Room→native): {received}");

    let got_broadcast = receive_result.unwrap_or(false);
    let audio_flowed = received > 0;

    println!(
        "  Broadcast arrived in Room: {}",
        if got_broadcast { "YES" } else { "NO" }
    );
    println!(
        "  Audio frames received:     {}",
        if audio_flowed { "YES" } else { "NO" }
    );

    if audio_flowed && received > 50 {
        println!("\n  PASS: Live browser audio flows through bridge ({received} frames)\n");
    } else if got_broadcast && !audio_flowed {
        println!("\n  FAIL: Broadcast arrived but no audio frames received\n");
    } else if !got_broadcast {
        println!("\n  FAIL: Broadcast never arrived in Room via bridge\n");
    } else {
        println!("\n  FAIL: Only {received} frames received (expected >50)\n");
    }

    live.shutdown().await;
    Ok(())
}

// ── IRC helpers (minimal) ────────────────────────────────────

async fn start_session_tcp(addr: &str) -> Result<(String, String)> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;
    let nick = "live-test";
    let channel = &format!("#live-{}", std::process::id());
    let stream = TcpStream::connect(addr).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    writer
        .write_all(format!("NICK {nick}\r\nUSER {nick} 0 * :test\r\n").as_bytes())
        .await?;
    let mut registered = false;
    let mut ticket = None;
    let mut session_id = None;
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
        _ => bail!("No session ID or ticket"),
    }
}

async fn start_session_ws(web_url: &str) -> Result<(String, String)> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let nick = "live-test";
    let channel = &format!("#live-{}", std::process::id());
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
    let mut ticket = None;
    let mut session_id = None;
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
        _ => bail!("No session ID or ticket"),
    }
}
