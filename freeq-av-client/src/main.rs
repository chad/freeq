//! freeq-av: Native audio client for freeq AV sessions.
//!
//! Supports two modes:
//! - Room mode: peer-to-peer via iroh-live Rooms (local network)
//! - SFU mode: via MoQ through the SFU (works across NAT, interops with browser)

mod sfu;

use anyhow::Result;
use clap::{Parser, Subcommand};
use iroh_live::{
    Live,
    media::{
        audio_backend::AudioBackend, codec::AudioCodec, format::AudioPreset,
        publish::LocalBroadcast,
    },
    rooms::{Room, RoomEvent, RoomTicket},
};

#[derive(Parser)]
#[command(name = "freeq-av", about = "Native audio client for freeq AV sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new room and print the ticket
    Room {
        /// Display name
        #[arg(short, long, default_value = "freeq-user")]
        name: String,
    },
    /// Join an existing room by ticket
    Join {
        /// Room ticket string
        ticket: String,
        /// Display name
        #[arg(short, long, default_value = "freeq-user")]
        name: String,
    },
    /// Connect to a freeq server, start/join an AV session, get iroh-live ticket
    Server {
        /// Server URL — use https://host for WebSocket IRC, or host:port for raw TCP
        #[arg(short, long, default_value = "127.0.0.1:6667")]
        url: String,
        /// Channel to join
        #[arg(short, long, default_value = "#freeq")]
        channel: String,
        /// Display name / nick
        #[arg(short, long, default_value = "freeq-av-user")]
        name: String,
        /// Join existing session instead of starting a new one
        #[arg(long)]
        join: bool,
    },
    /// Connect to SFU via MoQ (works across NAT, interops with browser)
    Sfu {
        /// SFU URL (e.g. https://staging.freeq.at)
        #[arg(short, long)]
        url: String,
        /// Session ID to join
        #[arg(short, long, default_value = "default")]
        session: String,
        /// Display name / nick
        #[arg(short, long, default_value = "freeq-av-user")]
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("freeq_av=info".parse()?)
                .add_directive("iroh_live=info".parse()?)
                .add_directive("moq=info".parse()?)
                .add_directive("warn".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Room { name } => run_room(name, None, None).await,
        Command::Join { ticket, name } => {
            let ticket: RoomTicket = ticket
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid room ticket: {e}"))?;
            run_room(name, Some(ticket), None).await
        }
        Command::Server {
            url,
            channel,
            name,
            join,
        } => run_server_session(&url, &channel, &name, join).await,
        Command::Sfu { url, session, name } => sfu::run_sfu(&url, &session, &name).await,
    }
}

async fn run_room(
    display_name: String,
    existing_ticket: Option<RoomTicket>,
    browser_url: Option<String>,
) -> Result<()> {
    tracing::info!("Starting iroh-live audio client...");

    let live = Live::from_env().await?.with_router().with_gossip().spawn();
    tracing::info!(id = %live.endpoint().id(), "Endpoint ready");

    // Create or join room
    let ticket = existing_ticket.unwrap_or_else(RoomTicket::generate);
    let room = Room::new(&live, ticket.clone()).await?;
    let room_ticket = room.ticket();

    println!("\n  Room ticket: {room_ticket}\n");
    println!("  Share this ticket with others to join the call.");
    println!("  Or join from freeq web: the ticket appears in the session.\n");

    // Set display name
    let (mut events, handle) = room.split();
    handle.set_display_name(&display_name).await?;

    // Set up audio
    let broadcast = LocalBroadcast::new();
    let audio_backend = AudioBackend::default();
    // Disable echo cancellation — sonora-aec3 has a slice bounds bug that crashes
    audio_backend.set_aec_enabled(false);

    // List available devices
    let inputs = AudioBackend::list_inputs();
    let outputs = AudioBackend::list_outputs();
    println!("  Audio devices:");
    for d in &inputs {
        println!("    Input:  {}", d.name);
    }
    for d in &outputs {
        println!("    Output: {}", d.name);
    }
    if inputs.is_empty() {
        println!("    WARNING: No input devices!");
    }
    if outputs.is_empty() {
        println!("    WARNING: No output devices!");
    }

    // Publish microphone
    let mic = audio_backend.default_input().await?;
    broadcast
        .audio()
        .set(mic, AudioCodec::Opus, [AudioPreset::Hq])?;
    // Use display name as broadcast name — matches browser convention where
    // broadcast name = nick (browsers subscribe to {session}/{nick})
    handle.publish(&display_name, &broadcast).await?;
    println!("  Microphone active (Opus). Press Ctrl+C to leave.");
    println!("  NOTE: Echo cancellation is disabled — use headphones to avoid feedback.\n");

    if let Some(ref url) = browser_url {
        println!("  -------------------------------------------------------");
        println!("  Browser call URL:");
        println!("  {url}");
        println!("  (Browser has built-in echo cancellation — no headphones needed)");
        println!("  -------------------------------------------------------\n");
    }

    // Keep track handles alive so playback doesn't stop
    let mut _active_tracks: Vec<iroh_live::media::subscribe::MediaTracks> = Vec::new();

    // Event loop — handle incoming participants
    loop {
        match events.recv().await {
            Some(event) => match event {
                RoomEvent::PeerJoined { display_name, .. } => {
                    let name = display_name.as_deref().unwrap_or("unknown");
                    println!("  + {name} joined");
                    tracing::info!(%name, "Peer joined");
                }
                RoomEvent::BroadcastSubscribed { session, broadcast } => {
                    let tracks = broadcast.media(&audio_backend, Default::default()).await?;
                    if tracks.audio.is_some() {
                        let remote = session.remote_id();
                        tracing::info!(%remote, "Receiving audio");
                        println!("  ~ Receiving audio from {remote}");
                    }
                    // Keep the tracks handle alive — dropping it stops playback
                    _active_tracks.push(tracks);
                }
                RoomEvent::PeerLeft { remote } => {
                    println!("  - {remote} left");
                    tracing::info!(%remote, "Peer left");
                }
                RoomEvent::ChatReceived { message, .. } => {
                    println!("  [chat] {message:?}");
                }
                _ => {}
            },
            None => {
                tracing::info!("Room closed");
                break;
            }
        }
    }

    live.shutdown().await;
    Ok(())
}

/// Connect to a freeq server via IRC (WebSocket or TCP), join AV session, get iroh-live ticket.
async fn run_server_session(
    url: &str,
    channel: &str,
    nick: &str,
    join_existing: bool,
) -> Result<()> {
    let use_websocket = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("wss://")
        || url.starts_with("ws://");

    if use_websocket {
        run_server_session_ws(url, channel, nick, join_existing).await
    } else {
        run_server_session_tcp(url, channel, nick, join_existing).await
    }
}

/// IRC session over raw TCP (for local dev or direct port access).
async fn run_server_session_tcp(
    url: &str,
    channel: &str,
    nick: &str,
    join_existing: bool,
) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let addr = url.trim_end_matches('/');
    let addr = if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:6667")
    };

    println!("  Connecting to {addr} (TCP)...");
    let stream = TcpStream::connect(&addr).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let result = irc_session_loop(&mut lines, &mut writer, nick, channel, join_existing).await?;

    if let Some(ticket_str) = result.ticket {
        // Build the browser call URL if we have a session ID
        let browser_url = result.session_id.as_ref().map(|sid| {
            // Derive the web origin from the server address
            format!(
                "http://{}/av/call.html?session={sid}",
                addr.split(':').next().unwrap_or("localhost")
            )
        });

        println!("  Joining iroh-live room...\n");
        let room_ticket: RoomTicket = ticket_str
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid room ticket from server: {e}"))?;

        // Keep IRC alive in background
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let _ = writer.write_all(b"PING :keepalive\r\n").await;
            }
        });

        run_room(nick.to_string(), Some(room_ticket), browser_url).await?;
    } else {
        println!("  No ticket received — server may not have iroh-live enabled.");
    }
    Ok(())
}

/// IRC session over WebSocket (for servers behind reverse proxy like Miren).
async fn run_server_session_ws(
    url: &str,
    channel: &str,
    nick: &str,
    join_existing: bool,
) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    // Build WebSocket URL: https://host → wss://host/irc
    let ws_url = if url.starts_with("https://") {
        format!(
            "wss://{}/irc",
            url.trim_start_matches("https://").trim_end_matches('/')
        )
    } else if url.starts_with("http://") {
        format!(
            "ws://{}/irc",
            url.trim_start_matches("http://").trim_end_matches('/')
        )
    } else {
        url.trim_end_matches('/').to_string()
    };

    println!("  Connecting to {ws_url} (WebSocket)...");
    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| anyhow::anyhow!("WebSocket connect failed: {e}"))?;
    let (mut ws_write, mut ws_read) = ws_stream.split();

    macro_rules! ws_send {
        ($line:expr) => {
            ws_write
                .send(Message::Text($line.into()))
                .await
                .map_err(|e| anyhow::anyhow!("WS send: {e}"))?
        };
    }

    // IRC registration
    ws_send!(format!("NICK {nick}\r\n"));
    ws_send!(format!("USER {nick} 0 * :freeq-av\r\n"));

    let mut registered = false;
    let mut ticket: Option<String> = None;
    let mut session_id: Option<String> = None;

    while let Some(msg) = ws_read.next().await {
        let msg = msg.map_err(|e| anyhow::anyhow!("WS read: {e}"))?;
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };

        // WebSocket may deliver multiple IRC lines in one message
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            tracing::debug!("< {line}");

            if line.starts_with("PING") {
                let pong = line.replace("PING", "PONG");
                ws_send!(format!("{pong}\r\n"));
                continue;
            }

            if !registered && line.contains(" 001 ") {
                registered = true;
                println!("  Connected as {nick}");
                ws_send!(format!("CAP REQ :message-tags\r\n"));
                ws_send!(format!("JOIN {channel}\r\n"));
                println!("  Joined {channel}");

                if join_existing {
                    ws_send!(format!("@+freeq.at/av-join TAGMSG {channel}\r\n"));
                    println!("  Joining AV session (waiting for ticket)...");
                } else {
                    ws_send!(format!("@+freeq.at/av-start TAGMSG {channel}\r\n"));
                    println!("  Starting AV session...");
                }
            }

            if line.contains("NOTICE") {
                if let Some(notice_text) = line.split(" :").nth(1) {
                    println!("  [server] {notice_text}");
                }
            }

            if line.contains("AV ticket:") {
                if let Some(t) = line.split("AV ticket: ").nth(1) {
                    let t = t.trim();
                    println!("  Got iroh-live ticket!");
                    ticket = Some(t.to_string());
                }
            }

            if line.contains("AV session started:") {
                if let Some(id) = line.split("AV session started: ").nth(1) {
                    session_id = Some(id.trim().to_string());
                }
                println!("  Session created, waiting for ticket...");
            }

            // Break out once we have the ticket
            if ticket.is_some() {
                break;
            }
        }
        if ticket.is_some() {
            break;
        }
    }

    if let Some(ticket_str) = ticket {
        // Build browser call URL from the server URL and session ID
        let browser_url = session_id.as_ref().map(|sid| {
            let host = url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/');
            let scheme = if url.starts_with("https://") {
                "https"
            } else {
                "http"
            };
            format!("{scheme}://{host}/av/call.html?session={sid}")
        });

        println!("  Joining iroh-live room...\n");
        let room_ticket: RoomTicket = ticket_str
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid room ticket from server: {e}"))?;

        // Keep WebSocket IRC alive in background
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
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

        run_room(nick.to_string(), Some(room_ticket), browser_url).await?;
    } else {
        println!("  No ticket received — server may not have iroh-live enabled.");
    }
    Ok(())
}

/// Result from IRC session setup: ticket + optional session ID.
struct IrcSessionResult {
    ticket: Option<String>,
    session_id: Option<String>,
}

/// Shared IRC session logic for TCP mode.
async fn irc_session_loop<R, W>(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<R>>,
    writer: &mut W,
    nick: &str,
    channel: &str,
    join_existing: bool,
) -> Result<IrcSessionResult>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    writer
        .write_all(format!("NICK {nick}\r\n").as_bytes())
        .await?;
    writer
        .write_all(format!("USER {nick} 0 * :freeq-av\r\n").as_bytes())
        .await?;

    let mut registered = false;
    let mut ticket: Option<String> = None;
    let mut session_id: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        tracing::debug!("< {line}");

        if line.starts_with("PING") {
            let pong = line.replace("PING", "PONG");
            writer.write_all(format!("{pong}\r\n").as_bytes()).await?;
            continue;
        }

        if !registered && line.contains(" 001 ") {
            registered = true;
            println!("  Connected as {nick}");
            writer
                .write_all(format!("CAP REQ :message-tags\r\n").as_bytes())
                .await?;
            writer
                .write_all(format!("JOIN {channel}\r\n").as_bytes())
                .await?;
            println!("  Joined {channel}");

            if join_existing {
                writer
                    .write_all(format!("@+freeq.at/av-join TAGMSG {channel}\r\n").as_bytes())
                    .await?;
                println!("  Joining AV session (waiting for ticket)...");
            } else {
                writer
                    .write_all(format!("@+freeq.at/av-start TAGMSG {channel}\r\n").as_bytes())
                    .await?;
                println!("  Starting AV session...");
            }
        }

        if line.contains("NOTICE") {
            if let Some(notice_text) = line.split(" :").nth(1) {
                println!("  [server] {notice_text}");
            }
        }

        if line.contains("AV ticket:") {
            if let Some(t) = line.split("AV ticket: ").nth(1) {
                let t = t.trim();
                println!("  Got iroh-live ticket!");
                ticket = Some(t.to_string());
                break;
            }
        }

        if line.contains("AV session started:") {
            if let Some(id) = line.split("AV session started: ").nth(1) {
                session_id = Some(id.trim().to_string());
            }
            println!("  Session created, waiting for ticket...");
        }
    }

    Ok(IrcSessionResult { ticket, session_id })
}
