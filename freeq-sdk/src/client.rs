//! IRC client with ATPROTO-CHALLENGE SASL support.
//!
//! This is the main entry point for SDK consumers. It manages the TCP
//! connection, IRC registration, CAP/SASL negotiation, and emits events.
//! Supports both plaintext and TLS connections.
//!
//! ## SASL Authentication
//!
//! Two SASL methods are supported:
//!
//! - **`web-token`**: A short-lived token minted by the auth broker after OAuth.
//!   Set `config.sasl_token` and `config.sasl_method = "web-token"`. The token is
//!   sent as the SASL payload and verified against the server's in-memory token map.
//!   Tokens expire after 5 minutes. Best for web and mobile clients that go through
//!   the OAuth broker flow.
//!
//! - **`crypto`**: Direct cryptographic challenge-response using the user's AT Protocol
//!   signing key. Set `config.sasl_method = "crypto"` and provide a DID + signing key.
//!   The server sends a challenge; the client signs it; the server verifies against the
//!   DID document. Best for bots and CLI tools with direct key access.
//!
//! ## Reconnection
//!
//! The SDK does not implement automatic reconnection. Consumers should implement
//! their own reconnect logic with exponential backoff (e.g., 2→4→8→16→30s cap)
//! to avoid overwhelming the server. Listen for [`Event::Disconnected`] and retry.

use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::rustls;
use tokio_rustls::TlsConnector;

use crate::auth::{self, ChallengeSigner};
use crate::event::Event;
use crate::irc::Message;

/// Configuration for connecting to an IRC server.
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    /// Server address (host:port).
    pub server_addr: String,
    /// Desired nickname.
    pub nick: String,
    /// Username (ident).
    pub user: String,
    /// Real name.
    pub realname: String,
    /// Use TLS.
    pub tls: bool,
    /// Skip TLS certificate verification (for self-signed certs).
    pub tls_insecure: bool,
    /// One-time web-token for SASL WEB-TOKEN authentication (from OAuth flow).
    pub web_token: Option<String>,
}

impl Default for ConnectConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:6667".to_string(),
            nick: "user".to_string(),
            user: "user".to_string(),
            realname: "IRC AT SDK User".to_string(),
            tls: false,
            tls_insecure: false,
            web_token: None,
        }
    }
}

/// Commands the consumer can send to the client.
#[derive(Debug)]
pub enum Command {
    Join(String),
    Privmsg { target: String, text: String },
    Raw(String),
    Quit(Option<String>),
}

/// A handle to a running IRC client connection.
#[derive(Clone)]
pub struct ClientHandle {
    cmd_tx: mpsc::Sender<Command>,
}

impl ClientHandle {
    pub async fn join(&self, channel: &str) -> Result<()> {
        self.cmd_tx.send(Command::Join(channel.to_string())).await?;
        Ok(())
    }

    pub async fn privmsg(&self, target: &str, text: &str) -> Result<()> {
        self.cmd_tx
            .send(Command::Privmsg {
                target: target.to_string(),
                text: text.to_string(),
            })
            .await?;
        Ok(())
    }

    pub async fn quit(&self, message: Option<&str>) -> Result<()> {
        self.cmd_tx
            .send(Command::Quit(message.map(|s| s.to_string())))
            .await?;
        Ok(())
    }

    pub async fn raw(&self, line: &str) -> Result<()> {
        self.cmd_tx.send(Command::Raw(line.to_string())).await?;
        Ok(())
    }

    /// Send a message with IRCv3 tags (for rich media).
    pub async fn send_tagged(
        &self,
        target: &str,
        text: &str,
        tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let msg = crate::irc::Message {
            tags,
            prefix: None,
            command: "PRIVMSG".to_string(),
            params: vec![target.to_string(), text.to_string()],
        };
        self.cmd_tx.send(Command::Raw(msg.to_string())).await?;
        Ok(())
    }

    /// Send a media attachment to a target (channel or user).
    pub async fn send_media(
        &self,
        target: &str,
        media: &crate::media::MediaAttachment,
    ) -> Result<()> {
        self.send_tagged(target, &media.fallback_text(), media.to_tags()).await
    }

    /// Send a TAGMSG (tags-only, no body) to a target.
    pub async fn send_tagmsg(
        &self,
        target: &str,
        tags: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let msg = crate::irc::Message {
            tags,
            prefix: None,
            command: "TAGMSG".to_string(),
            params: vec![target.to_string()],
        };
        self.cmd_tx.send(Command::Raw(msg.to_string())).await?;
        Ok(())
    }

    /// Send a reaction to a target (channel or user).
    /// Falls back to PRIVMSG for plain clients.
    pub async fn send_reaction(
        &self,
        target: &str,
        reaction: &crate::media::Reaction,
    ) -> Result<()> {
        self.send_tagmsg(target, reaction.to_tags()).await
    }

    /// Send a link preview as a tagged message.
    pub async fn send_link_preview(
        &self,
        target: &str,
        preview: &crate::media::LinkPreview,
    ) -> Result<()> {
        let fallback = match (&preview.title, &preview.description) {
            (Some(t), Some(d)) => format!("🔗 {} — {} ({})", t, d, preview.url),
            (Some(t), None) => format!("🔗 {} ({})", t, preview.url),
            _ => format!("🔗 {}", preview.url),
        };
        self.send_tagged(target, &fallback, preview.to_tags()).await
    }

    // ── Convenience helpers ──

    /// Send a reply to a specific message (adds +draft/reply tag).
    pub async fn reply(&self, target: &str, msgid: &str, text: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/reply".to_string(), msgid.to_string());
        self.send_tagged(target, text, tags).await
    }

    /// Send a reply in a thread (same as reply — thread parent is the msgid).
    pub async fn reply_in_thread(&self, target: &str, parent_msgid: &str, text: &str) -> Result<()> {
        self.reply(target, parent_msgid, text).await
    }

    /// Send a typing indicator start.
    pub async fn typing_start(&self, target: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+typing".to_string(), "active".to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Send a typing indicator stop.
    pub async fn typing_stop(&self, target: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+typing".to_string(), "done".to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Join multiple channels at once.
    pub async fn join_many(&self, channels: &[&str]) -> Result<()> {
        if channels.is_empty() { return Ok(()); }
        // IRC allows comma-separated JOIN
        let joined = channels.join(",");
        self.raw(&format!("JOIN {joined}")).await
    }

    /// Set a channel mode. Examples: `mode("#chan", "+o", Some("nick"))`.
    pub async fn mode(&self, channel: &str, flags: &str, arg: Option<&str>) -> Result<()> {
        match arg {
            Some(a) => self.raw(&format!("MODE {channel} {flags} {a}")).await,
            None => self.raw(&format!("MODE {channel} {flags}")).await,
        }
    }

    /// Request latest N messages of history (CHATHISTORY LATEST).
    pub async fn history_latest(&self, target: &str, count: usize) -> Result<()> {
        self.raw(&format!("CHATHISTORY LATEST {target} * {count}")).await
    }

    /// Request N messages before a given msgid (CHATHISTORY BEFORE).
    pub async fn history_before(&self, target: &str, msgid: &str, count: usize) -> Result<()> {
        self.raw(&format!("CHATHISTORY BEFORE {target} msgid={msgid} {count}")).await
    }

    /// Request N messages after a given msgid (CHATHISTORY AFTER).
    pub async fn history_after(&self, target: &str, msgid: &str, count: usize) -> Result<()> {
        self.raw(&format!("CHATHISTORY AFTER {target} msgid={msgid} {count}")).await
    }

    /// Send a reaction emoji to a specific message.
    pub async fn react(&self, target: &str, emoji: &str, msgid: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/react".to_string(), emoji.to_string());
        tags.insert("+draft/reply".to_string(), msgid.to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Edit a previously sent message.
    pub async fn edit_message(&self, target: &str, original_msgid: &str, new_text: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/edit".to_string(), original_msgid.to_string());
        self.send_tagged(target, new_text, tags).await
    }

    /// Delete a previously sent message (via TAGMSG).
    pub async fn delete_message(&self, target: &str, msgid: &str) -> Result<()> {
        let mut tags = std::collections::HashMap::new();
        tags.insert("+draft/delete".to_string(), msgid.to_string());
        self.send_tagmsg(target, tags).await
    }

    /// Pin a message in a channel.
    pub async fn pin(&self, channel: &str, msgid: &str) -> Result<()> {
        self.raw(&format!("PIN {channel} {msgid}")).await
    }

    /// Unpin a message in a channel.
    pub async fn unpin(&self, channel: &str, msgid: &str) -> Result<()> {
        self.raw(&format!("UNPIN {channel} {msgid}")).await
    }

    /// Set the channel topic.
    pub async fn topic(&self, channel: &str, topic: &str) -> Result<()> {
        self.raw(&format!("TOPIC {channel} :{topic}")).await
    }
}

/// Establish TCP (and optionally TLS) connection to the server.
///
/// This is done **before** the TUI starts so that connection errors
/// are visible on stderr. Returns the established connection for
/// `connect_with_stream` to use.
pub async fn establish_connection(
    config: &ConnectConfig,
) -> Result<EstablishedConnection> {
    // Auto-detect TLS from port if not explicitly set
    let use_tls = config.tls || config.server_addr.ends_with(":6697");
    let mode = if use_tls { "TLS" } else { "plain" };

    tracing::debug!("Resolving {}...", config.server_addr);
    let tcp = TcpStream::connect(&config.server_addr).await
        .map_err(|e| anyhow::anyhow!("TCP connect to {} failed: {e}", config.server_addr))?;
    tracing::debug!("TCP connected to {} ({mode})", config.server_addr);

    if use_tls {
        let tls_config = if config.tls_insecure {
            tracing::debug!("TLS: insecure mode (skipping cert verification)");
            rustls_insecure_config()
        } else {
            tracing::debug!("TLS: verifying server certificate...");
            rustls_default_config()
        };
        let connector = TlsConnector::from(Arc::new(tls_config));
        let server_name = config
            .server_addr
            .split(':')
            .next()
            .unwrap_or("localhost");
        let dns_name = rustls::pki_types::ServerName::try_from(server_name.to_string())?;
        let tls_stream = connector.connect(dns_name, tcp).await
            .map_err(|e| anyhow::anyhow!("TLS handshake with {} failed: {e}", config.server_addr))?;
        tracing::debug!("TLS handshake complete");
        Ok(EstablishedConnection::Tls(tls_stream))
    } else {
        Ok(EstablishedConnection::Plain(tcp))
    }
}

/// A connection that has completed TCP (and optionally TLS) but hasn't
/// started IRC registration yet.
pub enum EstablishedConnection {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
    /// Iroh QUIC connection (already encrypted, NAT-traversing).
    #[cfg(feature = "iroh-transport")]
    Iroh(tokio::io::DuplexStream),
}

/// ALPN for IRC-over-iroh (must match server).
#[cfg(feature = "iroh-transport")]
pub const IROH_ALPN: &[u8] = b"freeq/iroh/1";

#[cfg(feature = "iroh-transport")]
/// Establish a connection to an IRC server via iroh.
///
/// `addr` is the iroh endpoint address string (EndpointAddr format).
pub async fn establish_iroh_connection(addr: &str) -> Result<EstablishedConnection> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tracing::debug!("Creating iroh endpoint...");
    let endpoint = iroh::Endpoint::bind().await?;

    tracing::debug!("Connecting to iroh peer {addr}...");
    // Parse the endpoint ID (public key) and create an address.
    // Iroh's relay/discovery system handles finding the actual network path.
    let endpoint_id: iroh::EndpointId = addr.parse()
        .map_err(|e| anyhow::anyhow!("Invalid iroh endpoint ID '{addr}': {e}"))?;
    let endpoint_addr = iroh::EndpointAddr::new(endpoint_id);
    let conn = endpoint.connect(endpoint_addr, IROH_ALPN).await?;
    tracing::debug!("Iroh QUIC connection established (encrypted)");

    let (send, recv) = conn.open_bi().await
        .map_err(|e| anyhow::anyhow!("Failed to open bidirectional stream: {e}"))?;
    tracing::debug!("Bidirectional stream open, ready for IRC");

    // Bridge QUIC send/recv to a DuplexStream that the IRC handler can use.
    // irc_side goes to the IRC protocol handler.
    // bridge_side is shuttled to/from QUIC by two background tasks.
    let (irc_side, bridge_side) = tokio::io::duplex(16384);
    let (mut bridge_read, mut bridge_write) = tokio::io::split(bridge_side);

    // QUIC recv → bridge_write → IRC handler reads from irc_side
    tokio::spawn(async move {
        let mut recv = recv;
        let mut buf = vec![0u8; 4096];
        while let Ok(Some(n)) = recv.read(&mut buf).await {
            if bridge_write.write_all(&buf[..n]).await.is_err() { break; }
        }
        let _ = bridge_write.shutdown().await;
    });

    // IRC handler writes to irc_side → bridge_read → QUIC send
    tokio::spawn(async move {
        let mut send = send;
        let mut buf = vec![0u8; 4096];
        loop {
            match bridge_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if send.write_all(&buf[..n]).await.is_err() { break; }
                }
                Err(_) => break,
            }
        }
        let _ = send.finish();
    });

    // Keep endpoint + connection alive for the lifetime of the session
    tokio::spawn(async move {
        let _endpoint = endpoint;
        let _conn = conn;
        loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
    });

    Ok(EstablishedConnection::Iroh(irc_side))
}

/// Probe an IRC server for iroh endpoint ID via CAP LS.
///
/// Connects via TCP (or TLS for port 6697), sends CAP LS, reads the response,
/// extracts `iroh=<endpoint-id>` if present, and disconnects cleanly.
/// Returns `None` if the server doesn't advertise iroh.
///
/// This enables automatic iroh transport upgrade: connect cheap (TCP),
/// discover capabilities, reconnect optimal (iroh QUIC).
#[cfg(feature = "iroh-transport")]
pub async fn discover_iroh_id(server_addr: &str, tls: bool, tls_insecure: bool) -> Option<String> {
    use tokio::time::timeout;
    use std::time::Duration;

    let use_tls = tls || server_addr.ends_with(":6697");

    // Give the probe 5 seconds max
    let result = timeout(Duration::from_secs(5), async {
        let tcp = TcpStream::connect(server_addr).await.ok()?;

        if use_tls {
            let tls_config = if tls_insecure {
                rustls_insecure_config()
            } else {
                rustls_default_config()
            };
            let connector = TlsConnector::from(Arc::new(tls_config));
            let host = server_addr.split(':').next().unwrap_or("localhost");
            let dns_name = rustls::pki_types::ServerName::try_from(host.to_string()).ok()?;
            let tls_stream = connector.connect(dns_name, tcp).await.ok()?;
            probe_cap_ls(tls_stream).await
        } else {
            probe_cap_ls(tcp).await
        }
    }).await;

    result.ok().flatten()
}

/// Send CAP LS and parse iroh endpoint ID from response.
async fn probe_cap_ls<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(stream: S) -> Option<String> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Send CAP LS and a throwaway NICK/USER so the server doesn't time us out
    writer.write_all(b"CAP LS 302\r\nNICK _probe\r\nUSER _probe 0 * :probe\r\n").await.ok()?;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.ok()?;
        if n == 0 { return None; }

        // Look for CAP * LS :...
        if line.contains("CAP") && line.contains("LS") {
            // Find iroh=<id> in the caps string
            for token in line.split_whitespace() {
                if let Some(id) = token.strip_prefix("iroh=") {
                    // Clean up: send QUIT
                    let _ = writer.write_all(b"QUIT\r\n").await;
                    let _ = writer.shutdown().await;
                    return Some(id.trim().to_string());
                }
            }
            // Server responded to CAP LS but no iroh — done
            let _ = writer.write_all(b"QUIT\r\n").await;
            let _ = writer.shutdown().await;
            return None;
        }
    }
}

/// Connect using an already-established connection.
///
/// Returns a handle for sending commands and a receiver for events.
/// The IRC protocol runs in a spawned task.
pub fn connect_with_stream(
    conn: EstablishedConnection,
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
) -> (ClientHandle, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(4096);
    let (cmd_tx, cmd_rx) = mpsc::channel(256);

    let handle = ClientHandle {
        cmd_tx: cmd_tx.clone(),
    };

    tokio::spawn(async move {
        let _ = event_tx.send(Event::Connected).await;
        let result = match conn {
            EstablishedConnection::Plain(tcp) => {
                let (reader, writer) = tokio::io::split(tcp);
                run_irc(BufReader::new(reader), writer, &config, signer, event_tx.clone(), cmd_rx).await
            }
            EstablishedConnection::Tls(tls) => {
                let (reader, writer) = tokio::io::split(tls);
                run_irc(BufReader::new(reader), writer, &config, signer, event_tx.clone(), cmd_rx).await
            }
            #[cfg(feature = "iroh-transport")]
            EstablishedConnection::Iroh(duplex) => {
                let (reader, writer) = tokio::io::split(duplex);
                run_irc(BufReader::new(reader), writer, &config, signer, event_tx.clone(), cmd_rx).await
            }
        };
        if let Err(e) = result {
            let _ = event_tx
                .send(Event::Disconnected {
                    reason: e.to_string(),
                })
                .await;
        }
    });

    (handle, event_rx)
}

/// Connect to an IRC server and run the client.
///
/// Returns a handle for sending commands and a receiver for events.
/// The connection runs in a spawned task.
///
/// Note: prefer `establish_connection` + `connect_with_stream` for better
/// error reporting (connection errors happen before the TUI starts).
pub fn connect(
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
) -> (ClientHandle, mpsc::Receiver<Event>) {
    let (event_tx, event_rx) = mpsc::channel(4096);
    let (cmd_tx, cmd_rx) = mpsc::channel(256);

    let handle = ClientHandle {
        cmd_tx: cmd_tx.clone(),
    };

    tokio::spawn(async move {
        if let Err(e) = run_client(config, signer, event_tx.clone(), cmd_rx).await {
            let _ = event_tx
                .send(Event::Disconnected {
                    reason: e.to_string(),
                })
                .await;
        }
    });

    (handle, event_rx)
}

async fn run_client(
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
    event_tx: mpsc::Sender<Event>,
    cmd_rx: mpsc::Receiver<Command>,
) -> Result<()> {
    let conn = establish_connection(&config).await?;
    let _ = event_tx.send(Event::Connected).await;
    match conn {
        EstablishedConnection::Plain(tcp) => {
            let (reader, writer) = tokio::io::split(tcp);
            run_irc(BufReader::new(reader), writer, &config, signer, event_tx, cmd_rx).await
        }
        EstablishedConnection::Tls(tls) => {
            let (reader, writer) = tokio::io::split(tls);
            run_irc(BufReader::new(reader), writer, &config, signer, event_tx, cmd_rx).await
        }
        #[cfg(feature = "iroh-transport")]
        EstablishedConnection::Iroh(duplex) => {
            let (reader, writer) = tokio::io::split(duplex);
            run_irc(BufReader::new(reader), writer, &config, signer, event_tx, cmd_rx).await
        }
    }
}

fn install_crypto_provider() {
    // Install a crypto provider for rustls.
    // ring is preferred (works on iOS); aws-lc-rs is the default on desktop.
    #[cfg(feature = "ring")]
    { let _ = rustls::crypto::ring::default_provider().install_default(); }
    #[cfg(all(feature = "aws-lc-rs", not(feature = "ring")))]
    { let _ = rustls::crypto::aws_lc_rs::default_provider().install_default(); }
}

fn rustls_default_config() -> rustls::ClientConfig {
    install_crypto_provider();

    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

fn rustls_insecure_config() -> rustls::ClientConfig {
    install_crypto_provider();
    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
        .with_no_client_auth()
}

#[derive(Debug)]
struct InsecureVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms.supported_schemes())
            .unwrap_or_default()
    }
}

async fn run_irc<R, W>(
    mut reader: R,
    mut writer: W,
    config: &ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
    event_tx: mpsc::Sender<Event>,
    mut cmd_rx: mpsc::Receiver<Command>,
) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // Always negotiate capabilities (message-tags, and optionally sasl)
    writer.write_all(b"CAP LS 302\r\n").await?;

    writer
        .write_all(format!("NICK {}\r\n", config.nick).as_bytes())
        .await?;
    writer
        .write_all(format!("USER {} 0 * :{}\r\n", config.user, config.realname).as_bytes())
        .await?;

    let mut sasl_in_progress = false;
    let mut registered = false;
    let mut nick_tries: u32 = 0;
    let mut web_token = config.web_token.clone();
    let mut authenticated_did: Option<String> = None;
    let mut pending_commands: Vec<Command> = Vec::new();
    // Session message-signing keypair (generated after SASL success)
    let mut msg_signing_key: Option<ed25519_dalek::SigningKey> = None;
    let mut msg_signing_did: Option<String> = None;
    let mut line_buf = String::new();
    let mut last_activity = tokio::time::Instant::now();
    let ping_interval = tokio::time::Duration::from_secs(60);
    let ping_timeout = tokio::time::Duration::from_secs(120);

    loop {
        tokio::select! {
            result = reader.read_line(&mut line_buf) => {
                let n = result?;
                if n == 0 {
                    let _ = event_tx.send(Event::Disconnected { reason: "EOF".to_string() }).await;
                    break;
                }

                last_activity = tokio::time::Instant::now();
                let raw = line_buf.trim_end().to_string();
                let _ = event_tx.send(Event::RawLine(raw.clone())).await;

                if let Some(msg) = Message::parse(&line_buf) {
                    match msg.command.as_str() {
                        // ERR_NICKNAMEINUSE
                        "433" => {
                            // Nickname is already in use; try a variant before registration completes.
                            // Use base nick from config and append a short suffix.
                            nick_tries = nick_tries.saturating_add(1);
                            if nick_tries <= 5 {
                                let base = &config.nick;
                                let alt = if nick_tries == 1 {
                                    format!("{}1", base)
                                } else {
                                    format!("{}{}", base, nick_tries)
                                };
                                // Best-effort: attempt new nick immediately.
                                writer.write_all(format!("NICK {}\r\n", alt).as_bytes()).await?;
                            } else {
                                // Give up; let reconnect logic handle it.
                                let _ = event_tx.send(Event::Disconnected { reason: "Nick in use".to_string() }).await;
                                break;
                            }
                        }
                        "CAP" => {
                            handle_cap_response(&msg, &signer, &web_token, &mut writer, &mut sasl_in_progress).await?;
                        }
                        "AUTHENTICATE" => {
                            if let Some(ref token) = web_token {
                                // Web-token SASL: server sends challenge, we respond with JSON
                                // containing method:"web-token" and the token as signature.
                                // The DID is extracted by the server from the token lookup.
                                let payload = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                                if payload == "+" || !payload.is_empty() {
                                    // For web-token, we need the DID from the token lookup.
                                    // Send a JSON response matching ChallengeResponse format.
                                    // The DID comes from the server's token store, but we need
                                    // to send *something* — use a placeholder that matches.
                                    // Actually, we need the DID. Extract from config or just
                                    // send with empty DID — server validates via token lookup.
                                    let response = serde_json::json!({
                                        "did": "", // Server fills from token lookup
                                        "method": "web-token",
                                        "signature": token,
                                    });
                                    use base64::Engine;
                                    let json_bytes = response.to_string();
                                    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json_bytes.as_bytes());
                                    writer.write_all(format!("AUTHENTICATE {encoded}\r\n").as_bytes()).await?;
                                }
                            } else if let Some(ref signer) = signer {
                                handle_authenticate_challenge(&msg, signer.as_ref(), &mut writer).await?;
                            }
                        }
                        // Handle DPOP_NONCE notice during SASL — update signer nonce
                        "NOTICE" if sasl_in_progress => {
                            if let Some(text) = msg.params.last()
                                && let Some(nonce) = text.strip_prefix("DPOP_NONCE ")
                                    && let Some(ref s) = signer {
                                        s.set_dpop_nonce(nonce.trim());
                                    }
                                    // Server will re-issue AUTHENTICATE challenge next
                        }
                        // 900 RPL_LOGGEDIN — server tells us our authenticated DID
                        "900" => {
                            // :server 900 nick :You are now logged in as did:plc:...
                            if let Some(text) = msg.params.last()
                                && let Some(did) = text.split("as ").last() {
                                    let did = did.trim().to_string();
                                    if did.starts_with("did:") {
                                        authenticated_did = Some(did);
                                    }
                                }
                        }
                        "903" => {
                            sasl_in_progress = false;
                            let did = authenticated_did.take()
                                .or_else(|| signer.as_ref().map(|s| s.did().to_string()))
                                .unwrap_or_default();
                            if !did.is_empty() {
                                let _ = event_tx.send(Event::Authenticated { did: did.clone() }).await;
                                // Generate session message-signing keypair
                                let key = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
                                let pubkey_bytes = key.verifying_key().as_bytes().to_vec();
                                use base64::Engine;
                                let pubkey_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&pubkey_bytes);
                                msg_signing_key = Some(key);
                                msg_signing_did = Some(did);
                                writer.write_all(format!("MSGSIG {pubkey_b64}\r\n").as_bytes()).await?;
                            }
                            web_token = None;
                            writer.write_all(b"CAP END\r\n").await?;
                        }
                        "904" => {
                            sasl_in_progress = false;
                            let reason = msg.params.get(1).cloned().unwrap_or_else(|| "Unknown".to_string());
                            // eprintln!("  SASL authentication FAILED: {reason}");
                            let _ = event_tx.send(Event::AuthFailed { reason }).await;
                            writer.write_all(b"CAP END\r\n").await?;
                        }
                        "BATCH" => {
                            if let Some(ref_id) = msg.params.first() {
                                if let Some(id) = ref_id.strip_prefix('+') {
                                    let batch_type = msg.params.get(1).cloned().unwrap_or_default();
                                    let target = msg.params.get(2).cloned().unwrap_or_default();
                                    let _ = event_tx.send(Event::BatchStart {
                                        id: id.to_string(),
                                        batch_type,
                                        target,
                                    }).await;
                                } else if let Some(id) = ref_id.strip_prefix('-') {
                                    let _ = event_tx.send(Event::BatchEnd { id: id.to_string() }).await;
                                }
                            }
                        }
                        "001" => {
                            let nick = msg.params.first().cloned().unwrap_or_default();
                            let _ = event_tx.send(Event::Registered { nick }).await;
                            registered = true;
                            // Flush any commands that were queued before registration
                            for cmd in pending_commands.drain(..) {
                                execute_command(&mut writer, cmd, &msg_signing_key, &msg_signing_did).await?;
                            }
                        }
                        "353" => {
                            if msg.params.len() >= 4 {
                                let channel = msg.params[2].clone();
                                let nicks: Vec<String> = msg.params[3].split_whitespace().map(|s| s.to_string()).collect();
                                let _ = event_tx.send(Event::Names { channel, nicks }).await;
                            }
                        }
                        "PING" => {
                            let token = msg.params.first().map(|s| s.as_str()).unwrap_or("");
                            writer.write_all(format!("PONG :{token}\r\n").as_bytes()).await?;
                        }
                        "JOIN" => {
                            let channel = msg.params.first().cloned().unwrap_or_default();
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let _ = event_tx.send(Event::Joined { channel, nick }).await;
                        }
                        "PART" => {
                            let channel = msg.params.first().cloned().unwrap_or_default();
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let _ = event_tx.send(Event::Parted { channel, nick }).await;
                        }
                        "NICK" => {
                            let old_nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let new_nick = msg.params.first().cloned().unwrap_or_default();
                            if !old_nick.is_empty() && !new_nick.is_empty() {
                                let _ = event_tx.send(Event::NickChanged { old_nick, new_nick }).await;
                            }
                        }
                        // MODE change
                        "MODE" => {
                            if msg.params.len() >= 2 {
                                let target = &msg.params[0];
                                if target.starts_with('#') || target.starts_with('&') {
                                    let mode = msg.params[1].clone();
                                    let arg = msg.params.get(2).cloned();
                                    let set_by = msg.prefix.as_deref()
                                        .and_then(|p| p.split('!').next())
                                        .unwrap_or("server")
                                        .to_string();
                                    let _ = event_tx.send(Event::ModeChanged {
                                        channel: target.clone(),
                                        mode,
                                        arg,
                                        set_by,
                                    }).await;
                                }
                            }
                        }
                        // KICK
                        "KICK" => {
                            if msg.params.len() >= 2 {
                                let channel = msg.params[0].clone();
                                let kicked_nick = msg.params[1].clone();
                                let reason = msg.params.get(2).cloned().unwrap_or_default();
                                let by = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .unwrap_or("server")
                                    .to_string();
                                let _ = event_tx.send(Event::Kicked {
                                    channel,
                                    nick: kicked_nick,
                                    by,
                                    reason,
                                }).await;
                            }
                        }
                        // INVITE
                        "INVITE" => {
                            if msg.params.len() >= 2 {
                                let channel = msg.params[1].clone();
                                let by = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .unwrap_or("someone")
                                    .to_string();
                                let _ = event_tx.send(Event::Invited { channel, by }).await;
                            }
                        }
                        // AWAY (away-notify broadcast from shared channels)
                        "AWAY" => {
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let away_msg = msg.params.first().cloned();
                            let _ = event_tx.send(Event::AwayChanged { nick, away_msg }).await;
                        }
                        // TOPIC (live change from another user)
                        "TOPIC" => {
                            if let Some(channel) = msg.params.first() {
                                let topic = msg.params.get(1).cloned().unwrap_or_default();
                                let set_by = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .map(|s| s.to_string());
                                let _ = event_tx.send(Event::TopicChanged {
                                    channel: channel.clone(),
                                    topic,
                                    set_by,
                                }).await;
                            }
                        }
                        // RPL_TOPIC (on join or TOPIC query)
                        "332" => {
                            if msg.params.len() >= 3 {
                                let channel = msg.params[1].clone();
                                let topic = msg.params[2].clone();
                                let _ = event_tx.send(Event::TopicChanged {
                                    channel,
                                    topic,
                                    set_by: None,
                                }).await;
                            }
                        }
                        "331" => {
                            // RPL_NOTOPIC — no topic set, ignore or clear
                        }
                        "333" => {
                            // RPL_TOPICWHOTIME — ignore for now (info only)
                        }
                        // WHOIS numerics
                        "311" => {
                            // RPL_WHOISUSER: <nick> <user> <host> * :<realname>
                            if msg.params.len() >= 5 {
                                let nick = msg.params[1].clone();
                                let user = &msg.params[2];
                                let host = &msg.params[3];
                                let realname = &msg.params[4]; // skip the "*" at [3] — it's actually nick user host * :realname
                                let info = format!("{nick} is {user}@{host} ({realname})");
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "312" => {
                            // RPL_WHOISSERVER: <nick> <server> :<server info>
                            if msg.params.len() >= 4 {
                                let nick = msg.params[1].clone();
                                let server = &msg.params[2];
                                let info_text = &msg.params[3];
                                let info = format!("{nick} using {server} ({info_text})");
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "319" => {
                            // RPL_WHOISCHANNELS: <nick> :<channels>
                            if msg.params.len() >= 3 {
                                let nick = msg.params[1].clone();
                                let info = format!("{nick} on {}", msg.params[2]);
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "330" => {
                            // RPL_WHOISACCOUNT: <nick> <account> :is logged in as
                            if msg.params.len() >= 3 {
                                let nick = msg.params[1].clone();
                                let account = &msg.params[2];
                                let label = msg.params.get(3).map(|s| s.as_str()).unwrap_or("is authenticated as");
                                let info = format!("{nick} {label} {account}");
                                let _ = event_tx.send(Event::WhoisReply { nick, info }).await;
                            }
                        }
                        "318" => {
                            // RPL_ENDOFWHOIS — ignore silently
                        }
                        "401" => {
                            // ERR_NOSUCHNICK
                            if msg.params.len() >= 3 {
                                let nick = msg.params[1].clone();
                                let _ = event_tx.send(Event::WhoisReply {
                                    nick: nick.clone(),
                                    info: format!("{nick}: No such nick"),
                                }).await;
                            }
                        }
                        "QUIT" => {
                            let nick = msg.prefix.as_deref()
                                .and_then(|p| p.split('!').next())
                                .unwrap_or("")
                                .to_string();
                            let reason = msg.params.first().cloned().unwrap_or_default();
                            let _ = event_tx.send(Event::UserQuit { nick, reason }).await;
                        }
                        "PRIVMSG" | "NOTICE" => {
                            if msg.params.len() >= 2 {
                                let prefix = msg.prefix.as_deref().unwrap_or("");
                                let is_server_notice = msg.command == "NOTICE"
                                    && !prefix.contains('!');
                                if is_server_notice {
                                    // Server NOTICE (no hostmask in prefix) → ServerNotice
                                    let text = msg.params[1].clone();
                                    let _ = event_tx.send(Event::ServerNotice { text }).await;
                                } else {
                                    let from = prefix.split('!').next()
                                        .unwrap_or("")
                                        .to_string();
                                    let target = msg.params[0].clone();
                                    let text = msg.params[1].clone();
                                    let tags = msg.tags.clone();
                                    let _ = event_tx.send(Event::Message { from, target, text, tags }).await;
                                }
                            }
                        }
                        "TAGMSG" => {
                            if !msg.params.is_empty() {
                                let from = msg.prefix.as_deref()
                                    .and_then(|p| p.split('!').next())
                                    .unwrap_or("")
                                    .to_string();
                                let target = msg.params[0].clone();
                                let _ = event_tx.send(Event::TagMsg { from, target, tags: msg.tags.clone() }).await;
                            }
                        }
                        "FAIL" => {
                            // IRCv3 FAIL command — emit as ServerNotice
                            let text = msg.params.join(" ");
                            let _ = event_tx.send(Event::ServerNotice { text }).await;
                        }
                        _ => {
                            // Emit server error numerics (4xx, 5xx, 6xx, 9xx),
                            // MOTD lines (372/375/376), and unrecognized commands
                            // as ServerNotice so the UI can display them.
                            if let Ok(num) = msg.command.parse::<u16>() {
                                if (400..700).contains(&num) || (900..1000).contains(&num) {
                                    // Skip our nick (param[0]) and join the rest
                                    let text = if msg.params.len() > 1 {
                                        msg.params[1..].join(" ")
                                    } else {
                                        msg.params.join(" ")
                                    };
                                    let _ = event_tx.send(Event::ServerNotice { text }).await;
                                } else if num == 372 {
                                    // MOTD body line — strip "- " prefix
                                    let text = if msg.params.len() > 1 {
                                        let body = msg.params[1..].join(" ");
                                        let stripped = body.strip_prefix("- ").unwrap_or(&body);
                                        format!("MOTD:{}", stripped)
                                    } else {
                                        "MOTD:".to_string()
                                    };
                                    let _ = event_tx.send(Event::ServerNotice { text }).await;
                                } else if num == 375 {
                                    let _ = event_tx.send(Event::ServerNotice { text: "MOTD:START".to_string() }).await;
                                } else if num == 376 {
                                    let _ = event_tx.send(Event::ServerNotice { text: "MOTD:END".to_string() }).await;
                                }
                            }
                        }
                    }
                }

                line_buf.clear();
            }
            Some(cmd) = cmd_rx.recv() => {
                if registered || matches!(cmd, Command::Quit(_)) {
                    execute_command(&mut writer, cmd, &msg_signing_key, &msg_signing_did).await?;
                    if !registered {
                        break; // Quit before registration
                    }
                } else {
                    // Queue until registered — commands silently wait
                    pending_commands.push(cmd);
                }
            }
            // Periodic client-to-server PING and timeout detection
            _ = tokio::time::sleep_until(last_activity + ping_interval) => {
                if last_activity.elapsed() > ping_timeout {
                    let _ = event_tx.send(Event::Disconnected { reason: "Ping timeout".to_string() }).await;
                    break;
                }
                writer.write_all(b"PING :keepalive\r\n").await?;
            }
        }
    }

    Ok(())
}

/// Execute a single IRC command on the wire.
/// Execute a single IRC command on the wire.
/// If `signing_key` and `signing_did` are set, PRIVMSG gets a `+freeq.at/sig` tag.
async fn execute_command<W: AsyncWrite + Unpin>(
    writer: &mut W,
    cmd: Command,
    signing_key: &Option<ed25519_dalek::SigningKey>,
    signing_did: &Option<String>,
) -> Result<()> {
    match cmd {
        Command::Join(channel) => {
            writer.write_all(format!("JOIN {channel}\r\n").as_bytes()).await?;
        }
        Command::Privmsg { target, text } => {
            if let (Some(key), Some(did)) = (signing_key, signing_did) {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let canonical = format!("{did}\0{target}\0{text}\0{timestamp}");
                use ed25519_dalek::Signer;
                let sig = key.sign(canonical.as_bytes());
                use base64::Engine;
                let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
                // Send with IRCv3 message tag
                writer.write_all(
                    format!("@+freeq.at/sig={sig_b64} PRIVMSG {target} :{text}\r\n").as_bytes()
                ).await?;
            } else {
                writer.write_all(format!("PRIVMSG {target} :{text}\r\n").as_bytes()).await?;
            }
        }
        Command::Raw(line) => {
            writer.write_all(format!("{line}\r\n").as_bytes()).await?;
        }
        Command::Quit(msg) => {
            let quit_line = match msg {
                Some(m) => format!("QUIT :{m}\r\n"),
                None => "QUIT\r\n".to_string(),
            };
            writer.write_all(quit_line.as_bytes()).await?;
        }
    }
    Ok(())
}

async fn handle_cap_response<W: AsyncWrite + Unpin>(
    msg: &Message,
    signer: &Option<Arc<dyn ChallengeSigner>>,
    web_token: &Option<String>,
    writer: &mut W,
    sasl_in_progress: &mut bool,
) -> Result<()> {
    let subcmd = msg.params.get(1).map(|s| s.to_ascii_uppercase());
    match subcmd.as_deref() {
        Some("LS") => {
            let caps_str = msg.params.last().map(|s| s.as_str()).unwrap_or("");
            let mut req_caps = Vec::new();
            if caps_str.contains("message-tags") {
                req_caps.push("message-tags");
            }
            for cap in &["server-time", "batch", "echo-message", "away-notify", "account-notify", "extended-join", "draft/chathistory"] {
                if caps_str.contains(cap) {
                    req_caps.push(cap);
                }
            }
            if caps_str.contains("sasl") && (signer.is_some() || web_token.is_some()) {
                req_caps.push("sasl");
            } 
            if req_caps.is_empty() {
                // eprintln!("  No caps to request, sending CAP END");
                writer.write_all(b"CAP END\r\n").await?;
            } else {
                // eprintln!("  Requesting: {}", req_caps.join(" "));
                let req = format!("CAP REQ :{}\r\n", req_caps.join(" "));
                writer.write_all(req.as_bytes()).await?;
            }
        }
        Some("ACK") => {
            let caps = msg.params.last().map(|s| s.as_str()).unwrap_or("");
            if caps.contains("sasl") {
                *sasl_in_progress = true;
                // Both web-token and ATPROTO-CHALLENGE use the same SASL mechanism;
                // the method field in the JSON payload distinguishes them.
                writer.write_all(b"AUTHENTICATE ATPROTO-CHALLENGE\r\n").await?;
            } else {
                writer.write_all(b"CAP END\r\n").await?;
            }
        }
        Some("NAK") => {
            // eprintln!("  Capabilities rejected, sending CAP END");
            writer.write_all(b"CAP END\r\n").await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_authenticate_challenge<W: AsyncWrite + Unpin>(
    msg: &Message,
    signer: &dyn ChallengeSigner,
    writer: &mut W,
) -> Result<()> {
    let encoded_challenge = msg.params.first().map(|s| s.as_str()).unwrap_or("");
    // eprintln!("  Received SASL challenge ({} bytes encoded)", encoded_challenge.len());

    // Decode the challenge to raw bytes — these are what we sign
    let challenge_bytes = auth::decode_challenge_bytes(encoded_challenge)?;
    // eprintln!("  Challenge decoded ({} bytes), signing with {}...", challenge_bytes.len(), signer.did());

    // Produce the response using the signer
    let response = signer.respond(&challenge_bytes)?;
    let encoded = auth::encode_response(&response);
    // eprintln!("  Sending AUTHENTICATE response ({} bytes)", encoded.len());

    writer
        .write_all(format!("AUTHENTICATE {encoded}\r\n").as_bytes())
        .await?;

    Ok(())
}

// ── Reconnect helper ──

/// Configuration for automatic reconnection.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial delay before first reconnect attempt.
    pub initial_delay: std::time::Duration,
    /// Maximum delay between reconnect attempts.
    pub max_delay: std::time::Duration,
    /// Multiplier for exponential backoff.
    pub backoff_factor: f64,
    /// Channels to rejoin after reconnecting.
    pub channels: Vec<String>,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay: std::time::Duration::from_secs(2),
            max_delay: std::time::Duration::from_secs(30),
            backoff_factor: 2.0,
            channels: Vec::new(),
        }
    }
}

/// Run an event loop with automatic reconnection.
///
/// The `handler` is called for each event. When disconnected, the loop
/// reconnects with exponential backoff and rejoins configured channels.
///
/// Returns only on unrecoverable errors or when the handler returns `Err`.
///
/// # Example
///
/// ```rust,no_run
/// use freeq_sdk::client::{ConnectConfig, ReconnectConfig, run_with_reconnect};
///
/// # async fn example() -> anyhow::Result<()> {
/// let config = ConnectConfig { /* ... */ ..Default::default() };
/// let reconnect = ReconnectConfig {
///     channels: vec!["#bots".into()],
///     ..Default::default()
/// };
///
/// run_with_reconnect(config, None, reconnect, |handle, event| {
///     Box::pin(async move {
///         // handle event
///         Ok(())
///     })
/// }).await
/// # }
/// ```
pub async fn run_with_reconnect<F>(
    config: ConnectConfig,
    signer: Option<Arc<dyn ChallengeSigner>>,
    reconnect_config: ReconnectConfig,
    handler: F,
) -> Result<()>
where
    F: Fn(ClientHandle, Event) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> + Send + Sync,
{
    let mut delay = reconnect_config.initial_delay;
    let mut consecutive_failures = 0u32;

    loop {
        // Connect
        let conn = match establish_connection(&config).await {
            Ok(c) => {
                consecutive_failures = 0;
                delay = reconnect_config.initial_delay;
                c
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!(
                    error = %e,
                    attempt = consecutive_failures,
                    delay_secs = delay.as_secs(),
                    "Connection failed, retrying"
                );
                tokio::time::sleep(delay).await;
                // Exponential backoff with jitter
                let jitter = rand_jitter(delay.as_millis() as u64 / 4);
                delay = std::time::Duration::from_millis(
                    ((delay.as_millis() as f64 * reconnect_config.backoff_factor) as u64 + jitter)
                        .min(reconnect_config.max_delay.as_millis() as u64),
                );
                continue;
            }
        };

        let (handle, mut events) = connect_with_stream(conn, config.clone(), signer.clone());

        // Event loop
        let mut disconnected = false;
        while let Some(event) = events.recv().await {
            // Join configured channels once registered with the server.
            // (JOINs sent before registration are silently dropped by IRC servers.)
            if matches!(&event, Event::Registered { .. }) {
                for ch in &reconnect_config.channels {
                    let _ = handle.join(ch).await;
                }
            }
            if matches!(&event, Event::Disconnected { .. }) {
                disconnected = true;
            }
            if let Err(e) = handler(handle.clone(), event).await {
                tracing::error!(error = %e, "Handler error");
                // Non-fatal: continue processing
            }
            if disconnected {
                break;
            }
        }

        tracing::info!(delay_secs = delay.as_secs(), "Disconnected, will reconnect");
        tokio::time::sleep(delay).await;
        let jitter = rand_jitter(delay.as_millis() as u64 / 4);
        delay = std::time::Duration::from_millis(
            ((delay.as_millis() as f64 * reconnect_config.backoff_factor) as u64 + jitter)
                .min(reconnect_config.max_delay.as_millis() as u64),
        );
    }
}

/// Simple jitter: random value 0..max using thread_rng.
fn rand_jitter(max: u64) -> u64 {
    if max == 0 { return 0; }
    // Simple pseudo-random using current time
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    nanos % max
}
