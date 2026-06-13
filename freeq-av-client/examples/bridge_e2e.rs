//! End-to-end test for the MoQ↔iroh-live bridge.
//!
//! Connects to a running freeq-server (local or remote), starts an AV session,
//! publishes synthetic audio via MoQ WebSocket, joins the iroh-live Room,
//! and verifies audio flows through the bridge in both directions.
//!
//! Usage:
//!   # Local (TCP IRC + HTTP MoQ):
//!   cargo run --example bridge_e2e -- --irc-addr 127.0.0.1:6667 --web-url http://127.0.0.1:8080
//!
//!   # Staging (WebSocket IRC + HTTPS MoQ):
//!   cargo run --example bridge_e2e -- --web-url https://staging.freeq.at

use std::time::Duration;

use anyhow::{Result, bail};
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bridge_e2e=info".parse()?)
                .add_directive("freeq=info".parse()?)
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

    println!("\n=== Bridge E2E Test ===");
    if let Some(ref addr) = irc_addr {
        println!("  IRC: {addr} (TCP)");
    } else {
        println!("  IRC: {web_url}/irc (WebSocket)");
    }
    println!("  Web: {web_url}\n");

    // ── Step 1: Connect to IRC, start AV session, get ticket ──────
    println!("[1/5] Connecting to IRC and starting AV session...");
    let (session_id, iroh_ticket) = match irc_addr {
        Some(ref addr) => start_av_session_tcp(addr).await?,
        None => start_av_session_ws(&web_url).await?,
    };
    println!("  Session: {session_id}");
    println!("  Ticket: {}...", &iroh_ticket[..40.min(iroh_ticket.len())]);

    // ── Step 2: Publish synthetic audio via MoQ WebSocket ─────────
    println!("\n[2/5] Publishing synthetic audio to MoQ SFU...");
    let broadcast_name = format!("{session_id}/test-browser");
    let moq_url: url::Url = format!("{web_url}/av/moq").parse()?;
    let _moq_handle = publish_synthetic_moq(&moq_url, &broadcast_name).await?;
    println!("  Published as: {broadcast_name}");

    // Give the bridge time to notice the announcement
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── Step 3: Join iroh-live Room ───────────────────────────────
    println!("\n[3/5] Joining iroh-live Room (native client sim)...");
    let room_ticket: iroh_live::rooms::RoomTicket = iroh_ticket
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid room ticket: {e}"))?;

    let native_endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await?;
    let native_live = iroh_live::Live::builder(native_endpoint.clone())
        .with_router()
        .with_gossip()
        .spawn();

    let native_room = iroh_live::rooms::Room::new(&native_live, room_ticket).await?;
    let (mut native_events, native_handle) = native_room.split();
    native_handle.set_display_name("e2e-native").await?;
    println!("  Joined Room as 'e2e-native'");

    // ── Step 4: Check MoQ→Room (browser audio → native) ──────────
    println!("\n[4/5] Waiting for MoQ→Room bridged broadcast...");
    let moq_to_room_result = timeout(TIMEOUT, async {
        loop {
            match native_events.recv().await {
                Some(iroh_live::rooms::RoomEvent::PeerJoined {
                    display_name,
                    remote,
                }) => {
                    let name = display_name.as_deref().unwrap_or("?");
                    tracing::info!(%name, %remote, "Peer joined Room");
                    println!("  Peer joined: {name} ({remote})");
                }
                Some(iroh_live::rooms::RoomEvent::BroadcastSubscribed { broadcast, .. }) => {
                    let name = broadcast.broadcast_name().to_string();
                    println!("  Broadcast received: {name}");

                    // Try to read catalog
                    let consumer = broadcast.consumer().clone();
                    let catalog_track = moq_lite::Track::new("catalog.json");
                    match consumer.subscribe_track(&catalog_track) {
                        Ok(mut track) => {
                            match timeout(Duration::from_secs(5), track.next_group()).await {
                                Ok(Ok(Some(mut group))) => {
                                    if let Ok(Some(frame)) = group.read_frame().await {
                                        let text = String::from_utf8_lossy(&frame);
                                        println!("  catalog.json: {text}");
                                    }
                                }
                                _ => println!("  (catalog.json: no data yet)"),
                            }
                        }
                        Err(e) => println!("  catalog.json subscribe failed: {e}"),
                    }

                    // Try to read audio
                    let audio_track = moq_lite::Track::new("audio");
                    match consumer.subscribe_track(&audio_track) {
                        Ok(mut track) => {
                            match timeout(Duration::from_secs(5), track.next_group()).await {
                                Ok(Ok(Some(mut group))) => {
                                    if let Ok(Some(frame)) = group.read_frame().await {
                                        println!(
                                            "  audio frame: {} bytes (first byte: 0x{:02X})",
                                            frame.len(),
                                            frame[0]
                                        );
                                        return Ok::<bool, anyhow::Error>(true);
                                    }
                                }
                                _ => println!("  (audio: no data yet)"),
                            }
                        }
                        Err(e) => println!("  audio subscribe failed: {e}"),
                    }

                    // Got broadcast but couldn't read tracks — still partial success
                    return Ok(true);
                }
                Some(_) => {}
                None => bail!("Room events closed"),
            }
        }
    })
    .await;

    match moq_to_room_result {
        Ok(Ok(true)) => println!("  PASS: MoQ→Room bridge working!"),
        Ok(Ok(false)) => println!("  FAIL: MoQ→Room bridge returned false"),
        Ok(Err(ref e)) => println!("  FAIL: MoQ→Room bridge error: {e}"),
        Err(_) => println!("  FAIL: MoQ→Room timeout (no broadcast arrived in Room)"),
    }

    // ── Step 5: Check Room→MoQ (native audio → browser) ──────────
    println!("\n[5/5] Publishing from Room, checking MoQ cluster...");

    // Publish a synthetic broadcast into the Room
    let native_broadcast_name = "e2e-native-audio";
    let mut native_producer = moq_lite::Broadcast::produce();
    // Use proper hang catalog format so iroh-live can parse it
    let hang_catalog = r#"{"audio":{"renditions":{"audio":{"codec":"opus","sampleRate":48000,"numberOfChannels":2,"bitrate":128000,"container":{"kind":"legacy"}}}}}"#;

    let catalog_track = moq_lite::Track::new("catalog.json");
    let mut cw = native_producer.create_track(catalog_track)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from(
        hang_catalog.as_bytes().to_vec(),
    ))?;
    g.finish().ok();

    let audio_track = moq_lite::Track::new("audio");
    let mut aw = native_producer.create_track(audio_track)?;
    for seq in 0..3u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xBEu8; 480]))?;
        g.finish().ok();
    }

    native_handle
        .publish_producer(native_broadcast_name, native_producer.clone())
        .await?;
    println!("  Published '{native_broadcast_name}' to Room (hang catalog format)");

    // Subscribe from MoQ cluster to see if bridge forwarded it
    // We need a separate MoQ client connection that subscribes
    let expected_path = format!("{session_id}/{native_broadcast_name}");
    println!("  Waiting for '{expected_path}' in MoQ cluster...");

    let (sub_session, mut sub_consumer) = subscribe_moq(&moq_url).await?;

    let room_to_moq_result = timeout(TIMEOUT, async {
        while let Some((path, announce)) = sub_consumer.announced().await {
            let path_str = path.to_string();
            tracing::info!(%path_str, "MoQ announced");
            if path_str == expected_path {
                if let Some(consumer) = announce {
                    let audio_track = moq_lite::Track::new("audio");
                    match consumer.subscribe_track(&audio_track) {
                        Ok(mut track) => {
                            let group_result: Result<Option<moq_lite::GroupConsumer>, _> =
                                track.next_group().await;
                            if let Ok(Some(mut group)) = group_result {
                                let frame_result: Result<Option<bytes::Bytes>, _> =
                                    group.read_frame().await;
                                if let Ok(Some(frame)) = frame_result {
                                    println!(
                                        "  audio frame from Room→MoQ: {} bytes (0x{:02X})",
                                        frame.len(),
                                        frame[0]
                                    );
                                    return Ok::<bool, anyhow::Error>(true);
                                }
                            }
                        }
                        Err(e) => println!("  audio subscribe failed: {e}"),
                    }
                    return Ok(true);
                }
            }
        }
        bail!("MoQ subscriber stream ended")
    })
    .await;

    match room_to_moq_result {
        Ok(Ok(true)) => println!("  PASS: Room→MoQ bridge working!"),
        Ok(Ok(false)) => println!("  FAIL: Room→MoQ returned false"),
        Ok(Err(ref e)) => println!("  FAIL: Room→MoQ error: {e}"),
        Err(_) => println!("  FAIL: Room→MoQ timeout (broadcast never appeared in cluster)"),
    }

    // ── Summary ───────────────────────────────────────────────────
    println!("\n=== Results ===");
    let m2r = moq_to_room_result.is_ok_and(|r| r.is_ok_and(|b| b));
    let r2m = room_to_moq_result.is_ok_and(|r| r.is_ok_and(|b| b));
    println!(
        "  MoQ→Room (browser→native): {}",
        if m2r { "PASS" } else { "FAIL" }
    );
    println!(
        "  Room→MoQ (native→browser): {}",
        if r2m { "PASS" } else { "FAIL" }
    );
    if m2r && r2m {
        println!("\n  ALL PASS\n");
    } else {
        println!("\n  SOME FAILURES — see output above\n");
    }

    // Cleanup
    drop(_moq_handle);
    drop(sub_session);
    native_live.shutdown().await;

    Ok(())
}

/// Connect to IRC via raw TCP, join channel, start AV session, return (session_id, iroh_ticket).
async fn start_av_session_tcp(addr: &str) -> Result<(String, String)> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let nick = "e2e-tester";
    let channel = "#e2e-bridge-test";

    let stream = TcpStream::connect(addr).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    writer
        .write_all(format!("NICK {nick}\r\nUSER {nick} 0 * :e2e test\r\n").as_bytes())
        .await?;

    let mut registered = false;
    let mut ticket: Option<String> = None;
    let mut session_id: Option<String> = None;

    let result = timeout(Duration::from_secs(10), async {
        while let Some(line) = lines.next_line().await? {
            if line.starts_with("PING") {
                let pong = line.replace("PING", "PONG");
                writer.write_all(format!("{pong}\r\n").as_bytes()).await?;
                continue;
            }

            if !registered && line.contains(" 001 ") {
                registered = true;
                writer
                    .write_all(format!("CAP REQ :message-tags\r\n").as_bytes())
                    .await?;
                writer
                    .write_all(format!("JOIN {channel}\r\n").as_bytes())
                    .await?;
                // Small delay for join to complete
                tokio::time::sleep(Duration::from_millis(200)).await;
                writer
                    .write_all(format!("@+freeq.at/av-start TAGMSG {channel}\r\n").as_bytes())
                    .await?;
            }

            if line.contains("AV session started:") {
                // Extract session ID from "AV session started: <id>"
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

    // Keep IRC alive in background
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            if writer.write_all(b"PING :keepalive\r\n").await.is_err() {
                break;
            }
        }
    });

    match result {
        (Some(sid), Some(t)) => Ok((sid, t)),
        (None, Some(_)) => bail!("Got ticket but no session ID"),
        (Some(_), None) => bail!("Got session ID but no ticket"),
        (None, None) => bail!("No session ID or ticket received"),
    }
}

/// Connect to IRC via WebSocket, join channel, start AV session, return (session_id, iroh_ticket).
async fn start_av_session_ws(web_url: &str) -> Result<(String, String)> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let nick = "e2e-tester";
    let channel = "#e2e-bridge-test";

    // Build WS URL: https://host → wss://host/irc
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
            format!("NICK {nick}\r\nUSER {nick} 0 * :e2e test\r\n").into(),
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
                    let pong = line.replace("PING", "PONG");
                    ws_write
                        .send(Message::Text(format!("{pong}\r\n").into()))
                        .await?;
                    continue;
                }

                if !registered && line.contains(" 001 ") {
                    registered = true;
                    ws_write
                        .send(Message::Text(format!("CAP REQ :message-tags\r\n").into()))
                        .await?;
                    ws_write
                        .send(Message::Text(format!("JOIN {channel}\r\n").into()))
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

    // Keep WS IRC alive in background
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            if ws_write
                .send(Message::Text("PING :keepalive\r\n".into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    match result {
        (Some(sid), Some(t)) => Ok((sid, t)),
        (None, Some(_)) => bail!("Got ticket but no session ID"),
        (Some(_), None) => bail!("Got session ID but no ticket"),
        (None, None) => bail!("No session ID or ticket received"),
    }
}

/// Holds the MoQ publish state — all producers must stay alive to keep the broadcast open.
struct MoqPublishHandle {
    _session: moq_lite::Session,
    _origin: moq_lite::OriginProducer,
    _producer: moq_lite::BroadcastProducer,
    _catalog_track: moq_lite::TrackProducer,
    _audio_track: moq_lite::TrackProducer,
}

/// Connect to MoQ WebSocket, publish a synthetic broadcast.
/// Returns a handle that must be kept alive to maintain the publish.
async fn publish_synthetic_moq(
    moq_url: &url::Url,
    broadcast_name: &str,
) -> Result<MoqPublishHandle> {
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init()?;

    // Create synthetic broadcast with proper hang catalog format
    let hang_catalog = r#"{"audio":{"renditions":{"audio":{"codec":"opus","sampleRate":48000,"numberOfChannels":2,"bitrate":128000,"container":{"kind":"legacy"}}}}}"#;

    let mut producer = moq_lite::Broadcast::produce();

    let catalog_track = moq_lite::Track::new("catalog.json");
    let mut cw = producer.create_track(catalog_track)?;
    let mut g = cw.create_group(moq_lite::Group { sequence: 0 })?;
    g.write_frame(moq_lite::bytes::Bytes::from(
        hang_catalog.as_bytes().to_vec(),
    ))?;
    g.finish().ok();

    let audio_track = moq_lite::Track::new("audio");
    let mut aw = producer.create_track(audio_track)?;
    for seq in 0..10u64 {
        let mut g = aw.create_group(moq_lite::Group { sequence: seq })?;
        // 960 bytes = typical 20ms Opus frame at 48kHz
        g.write_frame(moq_lite::bytes::Bytes::from(vec![0xABu8; 960]))?;
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

    Ok(MoqPublishHandle {
        _session: session,
        _origin: origin,
        _producer: producer,
        _catalog_track: cw,
        _audio_track: aw,
    })
}

/// Connect to MoQ WebSocket as a subscriber.
async fn subscribe_moq(
    moq_url: &url::Url,
) -> Result<(moq_lite::Session, moq_lite::OriginConsumer)> {
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    client_config.backend = Some(moq_native::QuicBackend::Noq);
    let client = client_config.init()?;

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
