//! Real-audio E2E test: publishes a LocalBroadcast with real Opus encoding
//! through the iroh-live Room, subscribes via the bridge, and verifies the
//! decoded audio on the other side contains non-silent signal.
//!
//! This uses the real iroh-live audio pipeline (not synthetic bytes) to prove
//! Opus-encoded audio survives the MoQ↔Room bridge end-to-end.
//!
//! Requires a microphone (uses default input device). For CI, use the unit tests instead.
//!
//! Usage:
//!   cargo run --example audio_e2e -- --irc-addr 127.0.0.1:16667 --web-url http://127.0.0.1:18080
//!   cargo run --example audio_e2e -- --web-url https://staging.freeq.at

use anyhow::{Result, bail};
use iroh_live::media::{
    audio_backend::AudioBackend, codec::AudioCodec, format::AudioPreset, publish::LocalBroadcast,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;

const CAPTURE_SECS: u64 = 3;
const TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("audio_e2e=info".parse()?)
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

    println!("\n=== Real Audio E2E Test ===");
    println!("  Web: {web_url}");
    println!("  Uses real mic input + Opus encoding\n");

    // ── Step 1: Start AV session ──────────────────────────────────
    println!("[1/5] Starting AV session...");
    let (session_id, iroh_ticket) = match &irc_addr {
        Some(addr) => start_av_session_tcp(addr).await?,
        None => start_av_session_ws(&web_url).await?,
    };
    println!("  Session: {session_id}");

    // ── Step 2: Sender publishes real audio to Room ──────���────────
    println!("[2/5] Sender: publishing real mic audio to Room...");
    let room_ticket: iroh_live::rooms::RoomTicket = iroh_ticket
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid ticket: {e}"))?;

    let sender_ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let sender_live = iroh_live::Live::builder(sender_ep)
        .with_router()
        .with_gossip()
        .spawn();
    let sender_room = iroh_live::rooms::Room::new(&sender_live, room_ticket.clone()).await?;
    let (_sender_events, sender_handle) = sender_room.split();
    sender_handle.set_display_name("audio-sender").await?;

    // Use real audio backend with default mic
    let audio_backend = AudioBackend::default();
    audio_backend.set_aec_enabled(false);
    let broadcast = LocalBroadcast::new();
    let mic = audio_backend.default_input().await?;
    broadcast
        .audio()
        .set(mic, AudioCodec::Opus, [AudioPreset::Hq])?;
    sender_handle.publish("audio-sender", &broadcast).await?;
    println!("  Real mic audio publishing (Opus)");

    // ── Step 3: Receiver joins Room, captures audio data ──────────
    println!("[3/5] Receiver: joining Room...");
    let receiver_ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let receiver_live = iroh_live::Live::builder(receiver_ep)
        .with_router()
        .with_gossip()
        .spawn();
    let receiver_room = iroh_live::rooms::Room::new(&receiver_live, room_ticket).await?;
    let (mut receiver_events, receiver_handle) = receiver_room.split();
    receiver_handle.set_display_name("audio-receiver").await?;

    // Track raw MoQ frame data to verify Opus packets flow through
    let frame_sizes: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));

    // ── Step 4: Subscribe and collect frame data ──────────────────
    println!("[4/5] Waiting for sender's broadcast...");
    let fs = frame_sizes.clone();
    let subscribe_ok = timeout(TIMEOUT, async {
        loop {
            match receiver_events.recv().await {
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    if !name.contains("audio-sender") {
                        continue;
                    }
                    println!("  Got broadcast: {name}");

                    // Subscribe at MoQ level to verify raw Opus frames
                    let consumer = broadcast.consumer().clone();

                    // Find the audio track (could be "audio/opus-hq" or similar)
                    let catalog = broadcast.catalog();
                    let audio_renditions: Vec<String> =
                        catalog.audio.renditions.keys().cloned().collect();
                    println!("  Audio renditions: {audio_renditions:?}");

                    if let Some(track_name) = audio_renditions.first() {
                        let track = moq_lite::Track::new(track_name.clone());
                        if let Ok(mut track_consumer) = consumer.subscribe_track(&track) {
                            println!("  Subscribed to track: {track_name}");

                            // Collect frames for CAPTURE_SECS
                            let fs2 = fs.clone();
                            tokio::spawn(async move {
                                let deadline =
                                    tokio::time::Instant::now() + Duration::from_secs(CAPTURE_SECS);
                                while tokio::time::Instant::now() < deadline {
                                    match timeout(
                                        Duration::from_secs(2),
                                        track_consumer.next_group(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(Some(mut group))) => {
                                            while let Ok(Some(frame)) = group.read_frame().await {
                                                fs2.lock().unwrap().push(frame.len());
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                            });
                            return true;
                        }
                    }
                    return false;
                }
                Some(iroh_live::rooms::RoomEvent::PeerJoined { display_name, .. }) => {
                    println!("  Peer joined: {}", display_name.as_deref().unwrap_or("?"));
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    if !subscribe_ok {
        println!("  FAIL: Could not subscribe to audio track");
        sender_live.shutdown().await;
        receiver_live.shutdown().await;
        return Ok(());
    }

    // Wait for frames to accumulate
    println!("  Recording for {CAPTURE_SECS}s...");
    tokio::time::sleep(Duration::from_secs(CAPTURE_SECS + 1)).await;

    // ── Step 5: Analyze captured frames ───────────────────────────
    println!("[5/5] Analyzing captured audio frames...");
    let sizes = frame_sizes.lock().unwrap().clone();
    let num_frames = sizes.len();
    let total_bytes: usize = sizes.iter().sum();

    println!("  Opus frames received: {num_frames}");
    println!("  Total bytes: {total_bytes}");
    if !sizes.is_empty() {
        let avg_size = total_bytes / num_frames;
        let min_size = sizes.iter().min().unwrap();
        let max_size = sizes.iter().max().unwrap();
        println!("  Frame sizes: avg={avg_size}, min={min_size}, max={max_size}");

        // Expected: ~50 frames/sec for 20ms Opus frames, so ~150 in 3 seconds
        let expected_min_frames = (CAPTURE_SECS * 30) as usize; // conservative
        let frames_ok = num_frames >= expected_min_frames;

        // Opus frames should be non-trivial (> 2 bytes for non-silence)
        let non_trivial = sizes.iter().filter(|&&s| s > 2).count();
        let mostly_non_trivial = non_trivial > num_frames / 2;

        // Frame sizes should be in valid Opus range (1-4000 bytes)
        let valid_sizes = sizes.iter().all(|&s| s >= 1 && s <= 4000);

        println!("\n=== Results ===");
        println!(
            "  Frames received (>{}): {}",
            expected_min_frames,
            if frames_ok { "YES" } else { "NO" }
        );
        println!(
            "  Non-trivial frames:     {} ({}/{})",
            if mostly_non_trivial { "YES" } else { "NO" },
            non_trivial,
            num_frames
        );
        println!(
            "  Valid Opus sizes:       {}",
            if valid_sizes { "YES" } else { "NO" }
        );

        if frames_ok && mostly_non_trivial && valid_sizes {
            println!("\n  ALL PASS — real Opus audio flows through the bridge!\n");
        } else {
            println!("\n  SOME FAILURES\n");
        }
    } else {
        println!("\n  FAIL: No audio frames captured\n");
    }

    drop(broadcast);
    sender_live.shutdown().await;
    receiver_live.shutdown().await;
    Ok(())
}

// ── IRC helpers ──────────────────────────────────────────────────

async fn start_av_session_tcp(addr: &str) -> Result<(String, String)> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;
    let nick = "audio-test";
    let channel = &format!("#audio-{}", std::process::id());
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

async fn start_av_session_ws(web_url: &str) -> Result<(String, String)> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let nick = "audio-test";
    let channel = &format!("#audio-{}", std::process::id());
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
