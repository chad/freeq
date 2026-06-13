//! Full E2E: Browser (Playwright/Chromium) → MoQ → Bridge → Room → Native client
//!
//! This reproduces the exact user scenario:
//! 1. Start server with av-native
//! 2. Browser opens call.html with fake audio device (440Hz sine)
//! 3. Native client joins the same Room via iroh-live
//! 4. Verify native client receives audio frames through the bridge
//!
//! Requires: freeq-server running on 127.0.0.1:8080 with --iroh
//!
//! Usage:
//!   cargo run --example browser_to_native_e2e -- [--web-url http://127.0.0.1:8080]

use anyhow::{Result, bail};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("browser_to_native=info".parse()?)
                .add_directive("iroh_live=info".parse()?)
                .add_directive("moq=info".parse()?)
                .add_directive("warn".parse()?),
        )
        .init();

    let web_url = std::env::args()
        .position(|a| a == "--web-url")
        .and_then(|i| std::env::args().nth(i + 1))
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    println!("\n=== Browser → Native E2E Test ===");
    println!("  Server: {web_url}");
    println!("  Tests the EXACT path: Browser mic → MoQ → Bridge → Room → Native\n");

    // ── Step 1: Create AV session via IRC ─────────────────────
    println!("[1/5] Starting AV session via IRC...");
    let (session_id, iroh_ticket) = start_session(&web_url).await?;
    println!("  Session: {session_id}");

    // ── Step 2: Launch browser with fake audio, open call.html ─
    println!("[2/5] Launching browser with fake audio device...");
    let call_url = format!("{web_url}/av/call.html?session={session_id}&nick=browser-test");
    println!("  Opening: {call_url}");

    // Launch chromium with fake audio via command line
    // This uses the same approach as Playwright but via raw process
    let chrome_path = find_chromium()?;
    let mut chrome = tokio::process::Command::new(&chrome_path)
        .args([
            "--headless=new",
            "--use-fake-ui-for-media-stream",
            "--use-fake-device-for-media-stream",
            "--autoplay-policy=no-user-gesture-required",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-extensions",
            &call_url,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    println!("  Browser launched (headless Chromium with 440Hz fake audio)");

    // Give browser time to connect and start publishing
    println!("  Waiting 5s for browser to start publishing...");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // ── Step 3: Verify MoQ SFU has the browser's broadcast ────
    println!("[3/5] Checking MoQ SFU for browser's broadcast...");
    let moq_url: url::Url = format!("{web_url}/av/moq").parse()?;
    let mut cfg = moq_native::ClientConfig::default();
    cfg.tls.disable_verify = Some(true);
    cfg.backend = Some(moq_native::QuicBackend::Noq);
    let client = cfg.init()?;

    let pub_origin = moq_lite::Origin::produce();
    let sub_origin = moq_lite::Origin::produce();
    let mut sub_consumer = sub_origin.consume();

    let _moq_session = client
        .with_publish(pub_origin.consume())
        .with_consume(sub_origin)
        .connect(moq_url)
        .await?;

    let browser_broadcast = format!("{session_id}/browser-test");
    let found_broadcast = timeout(Duration::from_secs(10), async {
        while let Some((path, announce)) = sub_consumer.announced().await {
            let p = path.to_string();
            println!("  MoQ announced: {p}");
            if p == browser_broadcast && announce.is_some() {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    if found_broadcast {
        println!("  Browser's broadcast IS in MoQ cluster");
    } else {
        println!("  FAIL: Browser's broadcast NOT found in MoQ cluster");
        println!("  (Browser may not have published — check that call.html auto-joins)");
        chrome.kill().await.ok();
        return Ok(());
    }

    // ── Step 4: Join Room as native client ─────────────────────
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
    handle.set_display_name("native-listener").await?;
    println!("  Joined Room as 'native-listener'");

    // ── Step 5: Wait for bridged browser audio ─────────────────
    println!("[5/5] Waiting for browser audio via bridge...");
    let frames_received = Arc::new(AtomicU64::new(0));
    let fr = frames_received.clone();

    let result = timeout(TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(iroh_live::rooms::RoomEvent::PeerJoined { display_name, .. }) => {
                    println!("  Peer joined: {}", display_name.as_deref().unwrap_or("?"));
                }
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    println!("  Broadcast received in Room: {name}");

                    if !name.contains("browser-test") {
                        continue;
                    }

                    // Try to read catalog
                    let consumer = broadcast.consumer().clone();
                    let ct = moq_lite::Track::new("catalog.json");
                    match consumer.subscribe_track(&ct) {
                        Ok(mut track) => {
                            match timeout(Duration::from_secs(5), track.next_group()).await {
                                Ok(Ok(Some(mut g))) => {
                                    if let Ok(Some(frame)) = g.read_frame().await {
                                        println!(
                                            "  catalog.json: {}",
                                            String::from_utf8_lossy(&frame)
                                        );
                                    }
                                }
                                _ => println!("  catalog.json: no data (timeout or empty)"),
                            }
                        }
                        Err(e) => {
                            println!("  FAIL: catalog subscribe error: {e}");
                            return false;
                        }
                    }

                    // Read audio frames — try track names from the catalog
                    // Browser's moq-publish uses "audio/data" as the track name
                    let track_names = ["audio/data", "audio", "audio/opus-hq"];
                    let mut audio_track = None;
                    for tn in &track_names {
                        let t = moq_lite::Track::new(*tn);
                        if let Ok(track) = consumer.subscribe_track(&t) {
                            println!("  Subscribed to audio track: {tn}");
                            audio_track = Some(track);
                            break;
                        }
                    }

                    match audio_track {
                        Some(mut track) => {
                            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                            while tokio::time::Instant::now() < deadline {
                                match timeout(Duration::from_secs(3), track.next_group()).await {
                                    Ok(Ok(Some(mut g))) => {
                                        while let Ok(Some(frame)) = g.read_frame().await {
                                            let count = fr.fetch_add(1, Ordering::Relaxed);
                                            if count == 0 {
                                                println!(
                                                    "  FIRST audio frame: {} bytes",
                                                    frame.len()
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
                                        println!("  Audio timeout (3s no frames)");
                                        break;
                                    }
                                }
                            }
                            return true;
                        }
                        None => {
                            println!("  FAIL: could not subscribe to any audio track");
                            return false;
                        }
                    }
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

    // ── Results ───────────────────────────────────────────────
    let received = frames_received.load(Ordering::Relaxed);
    let got_broadcast = result.unwrap_or(false);

    println!("\n=== Results ===");
    println!("  Browser broadcast in MoQ: YES");
    println!(
        "  Broadcast arrived in Room: {}",
        if got_broadcast { "YES" } else { "NO" }
    );
    println!("  Audio frames received: {received}");

    if received > 10 {
        println!("\n  PASS: Browser audio flows through bridge to native ({received} frames)\n");
    } else if got_broadcast {
        println!("\n  FAIL: Broadcast arrived but no/few audio frames ({received})\n");
    } else {
        println!("\n  FAIL: Broadcast never arrived in Room\n");
    }

    chrome.kill().await.ok();
    live.shutdown().await;
    Ok(())
}

fn find_chromium() -> Result<String> {
    // macOS paths
    for path in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    ] {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }
    // Try PATH
    if let Ok(output) = std::process::Command::new("which").arg("chromium").output() {
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
    }
    bail!("Could not find Chromium/Chrome. Install Chrome or set path manually.")
}

async fn start_session(web_url: &str) -> Result<(String, String)> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let nick = "e2e-browser-native";
    let channel = &format!("#btn-{}", std::process::id());

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
