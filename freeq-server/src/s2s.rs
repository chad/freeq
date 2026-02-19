//! Server-to-server (S2S) clustering via iroh.
//!
//! Servers connect to each other using iroh's QUIC transport, forming
//! a mesh network. State is propagated via a simple message protocol
//! on bidirectional streams.
//!
//! # Design (post-audit)
//!
//! - **Peer identity**: iroh endpoint ID is the root identity everywhere.
//!   `server_name` is untrusted display metadata included for logging.
//! - **Event dedup**: every S2S event carries an `event_id` (origin + counter).
//!   A bounded LRU per peer prevents duplicate application on reconnect.
//! - **Source of truth**: presence is S2S-event-only (not CRDT). Topic and
//!   durable authority use CRDT as the convergent source of truth; S2S
//!   events are notifications for immediate UX delivery.
//! - **CRDT sync keyed by iroh endpoint ID** (cryptographic identity).
//!
//! # Protocol
//!
//! Each S2S link uses a single bidirectional QUIC stream carrying
//! newline-delimited JSON messages. Messages are typed:
//!
//! ```json
//! {"type":"hello","peer_id":"44f1415c...","server_name":"freeq"}
//! {"type":"privmsg","event_id":"44f1415c:42","from":"nick!user@host","target":"#channel","text":"hello"}
//! ```
//!
//! # Topology
//!
//! Simple mesh: each server connects to all configured peers. Messages
//! are forwarded with origin tracking to prevent loops.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::server::SharedState;

/// ALPN for server-to-server links.
pub const S2S_ALPN: &[u8] = b"freeq/s2s/1";

/// Maximum number of event IDs to remember per peer for dedup.
const DEDUP_CAPACITY: usize = 10_000;

/// Messages exchanged between servers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum S2sMessage {
    /// Identity handshake — sent immediately on link establishment.
    /// Binds the transport identity (iroh endpoint ID) to the logical
    /// server_name. The peer_id is verified against the QUIC connection's
    /// remote_id() — spoofing is impossible.
    #[serde(rename = "hello")]
    Hello {
        /// Iroh endpoint ID (must match connection's remote_id).
        peer_id: String,
        /// Human-readable server name (untrusted display metadata).
        server_name: String,
    },

    /// A PRIVMSG or NOTICE relayed between servers.
    #[serde(rename = "privmsg")]
    Privmsg {
        /// Stable event ID for dedup: "{origin_peer_id}:{counter}".
        #[serde(default)]
        event_id: String,
        from: String,
        target: String,
        text: String,
        /// Origin iroh endpoint ID (to prevent relay loops).
        origin: String,
    },

    /// A user joined a channel.
    #[serde(rename = "join")]
    Join {
        #[serde(default)]
        event_id: String,
        nick: String,
        channel: String,
        /// Authenticated DID (if any) — used for DID-based ops.
        did: Option<String>,
        /// Resolved AT Protocol handle (e.g. "chadfowler.com").
        handle: Option<String>,
        /// Whether this user is an operator on their home server.
        #[serde(default)]
        is_op: bool,
        origin: String,
    },

    /// A channel was created (carries founder info for authority resolution).
    #[serde(rename = "channel_created")]
    ChannelCreated {
        #[serde(default)]
        event_id: String,
        channel: String,
        /// DID of the channel founder.
        founder_did: Option<String>,
        /// DIDs with operator status.
        did_ops: Vec<String>,
        /// Unix timestamp of channel creation (informational only).
        created_at: u64,
        origin: String,
    },

    /// A user left a channel.
    #[serde(rename = "part")]
    Part {
        #[serde(default)]
        event_id: String,
        nick: String,
        channel: String,
        origin: String,
    },

    /// A user quit.
    #[serde(rename = "quit")]
    Quit {
        #[serde(default)]
        event_id: String,
        nick: String,
        reason: String,
        origin: String,
    },

    /// A user changed nick.
    #[serde(rename = "nick_change")]
    NickChange {
        #[serde(default)]
        event_id: String,
        old: String,
        new: String,
        origin: String,
    },

    /// Channel topic changed.
    #[serde(rename = "topic")]
    Topic {
        #[serde(default)]
        event_id: String,
        channel: String,
        topic: String,
        set_by: String,
        origin: String,
    },

    /// Channel mode changed.
    #[serde(rename = "mode")]
    Mode {
        #[serde(default)]
        event_id: String,
        channel: String,
        mode: String,
        arg: Option<String>,
        set_by: String,
        origin: String,
    },

    /// Request full state sync (sent on initial link).
    #[serde(rename = "sync_request")]
    SyncRequest,

    /// Response with current server state.
    #[serde(rename = "sync_response")]
    SyncResponse {
        /// Server's iroh endpoint ID.
        server_id: String,
        /// Active channels and their topics.
        channels: Vec<ChannelInfo>,
    },

    /// Automerge CRDT sync message for convergent state.
    #[serde(rename = "crdt_sync")]
    CrdtSync {
        /// Base64-encoded Automerge sync message.
        data: String,
        /// Origin iroh endpoint ID (used to key sync state).
        origin: String,
    },

    /// A user was kicked from a channel.
    #[serde(rename = "kick")]
    Kick {
        #[serde(default)]
        event_id: String,
        /// Nick of the user being kicked.
        nick: String,
        channel: String,
        /// Nick of the op who kicked them.
        by: String,
        reason: String,
        origin: String,
    },

    /// Internal event: a peer's S2S link has disconnected.
    /// Not sent over the wire — synthesized locally so the event processor
    /// can clean up remote_members for that peer's origin.
    #[serde(rename = "peer_disconnected")]
    PeerDisconnected {
        /// The iroh endpoint ID of the peer that disconnected.
        peer_id: String,
    },
}

/// Per-user info in a channel sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncNick {
    pub nick: String,
    #[serde(default)]
    pub is_op: bool,
    pub did: Option<String>,
}

/// Channel info for sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub name: String,
    pub topic: Option<String>,
    /// Legacy field: plain nick list (for backward compat with old servers).
    #[serde(default)]
    pub nicks: Vec<String>,
    /// Rich nick list with per-user metadata (preferred over `nicks`).
    #[serde(default)]
    pub nick_info: Vec<SyncNick>,
    /// Channel founder DID.
    pub founder_did: Option<String>,
    /// DIDs with persistent operator status.
    pub did_ops: Vec<String>,
    /// Channel creation timestamp.
    pub created_at: u64,
    /// Channel modes: topic_locked, invite_only, no_ext_msg, moderated
    #[serde(default)]
    pub topic_locked: bool,
    #[serde(default)]
    pub invite_only: bool,
    #[serde(default)]
    pub no_ext_msg: bool,
    #[serde(default)]
    pub moderated: bool,
    #[serde(default)]
    pub key: Option<String>,
}

/// Bounded set for event dedup. Uses two layers:
/// 1. **Monotonic high-water mark** per peer: if the event_id counter
///    portion is ≤ the highest seen, reject it outright. This survives
///    beyond the ring buffer window.
/// 2. **Ring buffer** (VecDeque + HashSet): for non-monotonic or
///    near-duplicate IDs within the recent window.
///
/// Event ID format: `{origin_peer_id}:{counter}` where counter is
/// microseconds-since-epoch (monotonically increasing per sender).
pub struct DedupSet {
    /// Per-peer seen event IDs (ring buffer). Key = origin peer_id.
    seen: tokio::sync::Mutex<HashMap<String, HashSet<String>>>,
    /// Per-peer insertion order for bounded eviction (O(1) pop_front).
    order: tokio::sync::Mutex<HashMap<String, VecDeque<String>>>,
    /// Per-peer monotonic high-water mark: highest counter value seen.
    /// Any event with counter ≤ this is rejected, even outside the ring buffer.
    high_water: tokio::sync::Mutex<HashMap<String, u64>>,
}

impl DedupSet {
    pub fn new() -> Self {
        Self {
            seen: tokio::sync::Mutex::new(HashMap::new()),
            order: tokio::sync::Mutex::new(HashMap::new()),
            high_water: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Extract the counter portion from an event_id ("origin:counter" → counter).
    fn parse_counter(event_id: &str) -> Option<u64> {
        event_id.rsplit_once(':').and_then(|(_, c)| c.parse().ok())
    }

    /// Returns true if this event_id is new (not a duplicate).
    /// Returns true for empty event_ids (backward compat with old peers).
    pub async fn check_and_insert(&self, origin: &str, event_id: &str) -> bool {
        if event_id.is_empty() {
            return true; // No dedup for legacy messages
        }

        // Layer 1: monotonic high-water mark check
        if let Some(counter) = Self::parse_counter(event_id) {
            let mut hw = self.high_water.lock().await;
            let mark = hw.entry(origin.to_string()).or_insert(0);
            if counter <= *mark {
                return false; // Counter is ≤ highest seen — stale/replay
            }
            *mark = counter;
        }

        // Layer 2: ring buffer for exact-match dedup
        let mut seen = self.seen.lock().await;
        let mut order = self.order.lock().await;

        let peer_seen = seen.entry(origin.to_string()).or_default();
        let peer_order = order.entry(origin.to_string()).or_default();

        if peer_seen.contains(event_id) {
            return false; // Exact duplicate in ring buffer
        }

        // Evict oldest if at capacity — O(1) with VecDeque
        if peer_seen.len() >= DEDUP_CAPACITY {
            if let Some(oldest) = peer_order.pop_front() {
                peer_seen.remove(&oldest);
            }
        }

        peer_seen.insert(event_id.to_string());
        peer_order.push_back(event_id.to_string());
        true
    }

    /// Remove all state for a disconnected peer.
    pub async fn remove_peer(&self, origin: &str) {
        self.seen.lock().await.remove(origin);
        self.order.lock().await.remove(origin);
        // Reset high-water mark on disconnect. The old rationale was that
        // time-seeded counters always increase across restarts, but that
        // assumption fails on NTP backward steps, VM resume, or clock skew.
        // After a full disconnect+reconnect, the peer sends a SyncResponse
        // anyway, so we don't need the high-water to protect against replays
        // from the previous session — the ring buffer handles near-term dedup.
        self.high_water.lock().await.remove(origin);
    }
}

/// State for managing S2S links.
/// A peer connection entry with a generation counter for safe cleanup.
#[derive(Clone)]
pub struct PeerEntry {
    pub tx: mpsc::Sender<S2sMessage>,
    pub conn_gen: u64,
}

pub struct S2sManager {
    /// Our server's iroh endpoint ID (cryptographic identity).
    pub server_id: String,
    /// Our server's human-readable name (for Hello messages).
    pub server_name: String,
    /// Connected peer servers: peer_id (iroh endpoint ID) → sender + generation.
    pub peers: Arc<tokio::sync::Mutex<HashMap<String, PeerEntry>>>,
    /// Mapping: iroh endpoint ID → server_name (populated from Hello handshake).
    pub peer_names: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// Channel for S2S events that need to be applied to server state.
    pub event_tx: mpsc::Sender<AuthenticatedS2sEvent>,
    /// Monotonic counter for generating unique event IDs.
    pub event_counter: AtomicU64,
    /// Event dedup set.
    pub dedup: Arc<DedupSet>,
    /// Ordered broadcast queue: ensures messages are sent to peers in the
    /// same order their event IDs were assigned.  Without this, independent
    /// tokio::spawn tasks can reorder messages, causing the receiver's
    /// monotonic high-water-mark dedup to reject out-of-order events.
    pub broadcast_tx: mpsc::Sender<S2sMessage>,
    /// Monotonic counter for connection generations — used to ensure cleanup
    /// only removes its own peer entry, not a replacement's.
    pub conn_gen: Arc<AtomicU64>,
}

impl S2sManager {
    /// Generate a unique event ID for outgoing messages.
    pub fn next_event_id(&self) -> String {
        let counter = self.event_counter.fetch_add(1, Ordering::Relaxed);
        format!("{}:{}", self.server_id, counter)
    }

    /// Queue a message for ordered broadcast to all peer servers.
    /// Messages are processed by a single task to preserve event ID ordering.
    pub fn broadcast(&self, msg: S2sMessage) {
        if self.broadcast_tx.try_send(msg).is_err() {
            tracing::warn!("S2S broadcast queue full or closed");
        }
    }

    /// Internal: send a message directly to all connected peers (called by broadcast worker).
    async fn broadcast_to_peers(&self, msg: S2sMessage) {
        let peers = self.peers.lock().await;
        if peers.is_empty() {
            return;
        }
        for (peer_id, entry) in peers.iter() {
            if entry.tx.send(msg.clone()).await.is_err() {
                tracing::warn!(peer = %peer_id, "S2S broadcast: failed to send to peer");
            }
        }
    }

    /// Look up the human-readable name for a peer (from Hello handshake).
    pub async fn peer_display_name(&self, peer_id: &str) -> String {
        self.peer_names.lock().await
            .get(peer_id)
            .cloned()
            .unwrap_or_else(|| peer_id[..8.min(peer_id.len())].to_string())
    }
}

/// An S2S message annotated with the transport-authenticated peer identity.
/// The `authenticated_peer_id` comes from `conn.remote_id()` (iroh's
/// cryptographic endpoint ID) — it cannot be spoofed by the payload.
#[derive(Debug, Clone)]
pub struct AuthenticatedS2sEvent {
    /// The iroh endpoint ID of the peer that sent this message,
    /// verified by the QUIC transport layer.
    pub authenticated_peer_id: String,
    /// The deserialized S2S message from the peer.
    pub msg: S2sMessage,
}

/// Start the S2S subsystem.
///
/// Returns the manager + event receiver.
pub async fn start(
    state: Arc<SharedState>,
    endpoint: iroh::Endpoint,
) -> Result<(Arc<S2sManager>, mpsc::Receiver<AuthenticatedS2sEvent>)> {
    let (event_tx, event_rx) = mpsc::channel(1024);
    let server_id = endpoint.id().to_string();

    let (broadcast_tx, mut broadcast_rx) = mpsc::channel::<S2sMessage>(1024);

    let manager = Arc::new(S2sManager {
        server_id: server_id.clone(),
        server_name: state.server_name.clone(),
        peers: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        peer_names: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        event_tx: event_tx.clone(),
        // Initialize counter from wall clock (microseconds since epoch) so
        // restarts produce strictly increasing event IDs. Peers can use the
        // counter portion for monotonic dedup even across our restarts.
        event_counter: AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64
        ),
        dedup: Arc::new(DedupSet::new()),
        broadcast_tx,
        conn_gen: Arc::new(AtomicU64::new(0)),
    });

    // Spawn the ordered broadcast worker.  All outbound S2S messages flow
    // through this single task, guaranteeing they reach the QUIC writer in
    // the same order their event IDs were assigned.
    let bcast_manager = Arc::clone(&manager);
    tokio::spawn(async move {
        while let Some(msg) = broadcast_rx.recv().await {
            bcast_manager.broadcast_to_peers(msg).await;
        }
    });

    Ok((manager, event_rx))
}

/// Handle an incoming S2S connection (called from iroh accept loop).
pub async fn handle_incoming_s2s(
    conn: iroh::endpoint::Connection,
    state: Arc<SharedState>,
) {
    let manager = state.s2s_manager.lock().unwrap().clone();
    let manager = match manager {
        Some(m) => m,
        None => {
            tracing::warn!("Incoming S2S connection but no S2S manager active");
            return;
        }
    };
    let peer_id = conn.remote_id().to_string();

    // Check allowlist
    let allowed = &state.config.s2s_allowed_peers;
    if !allowed.is_empty() && !allowed.contains(&peer_id) {
        tracing::warn!(
            peer = %peer_id,
            "Rejecting S2S connection: peer not in --s2s-allowed-peers allowlist"
        );
        conn.close(1u32.into(), b"not authorized");
        return;
    }

    tracing::info!(peer = %peer_id, "S2S incoming connection (routed by ALPN)");
    handle_s2s_connection_from_manager(conn, &manager, true).await;
}

/// Connect to a peer server by iroh endpoint ID.
pub async fn connect_peer(
    endpoint: &iroh::Endpoint,
    peer_id: &str,
    manager: &Arc<S2sManager>,
) -> Result<()> {
    let endpoint_id: iroh::EndpointId = peer_id.parse()
        .map_err(|e| anyhow::anyhow!("Invalid peer endpoint ID: {e}"))?;
    let addr = iroh::EndpointAddr::new(endpoint_id);

    tracing::info!(peer = %peer_id, "Connecting to S2S peer");
    let conn = endpoint.connect(addr, S2S_ALPN).await?;

    let manager = Arc::clone(manager);
    tokio::spawn(async move {
        handle_s2s_connection_from_manager(conn, &manager, false).await;
    });

    Ok(())
}

/// Connect to a peer with automatic reconnection on failure.
pub fn connect_peer_with_retry(
    endpoint: iroh::Endpoint,
    peer_id: String,
    manager: Arc<S2sManager>,
) {
    tokio::spawn(async move {
        let mut backoff = std::time::Duration::from_secs(1);
        let max_backoff = std::time::Duration::from_secs(60);

        loop {
            let endpoint_id: iroh::EndpointId = match peer_id.parse() {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!(peer = %peer_id, "Invalid peer endpoint ID (not retrying): {e}");
                    return;
                }
            };
            let addr = iroh::EndpointAddr::new(endpoint_id);

            // Skip reconnect if we already have a live connection (e.g. incoming replaced ours)
            if manager.peers.lock().await.contains_key(&peer_id) {
                tracing::info!(peer = %peer_id, "S2S peer already connected (via incoming), skipping outgoing attempt");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }

            tracing::info!(peer = %peer_id, "Connecting to S2S peer");
            match endpoint.connect(addr, S2S_ALPN).await {
                Ok(conn) => {
                    backoff = std::time::Duration::from_secs(1);
                    tracing::info!(peer = %peer_id, "S2S peer connected, entering link handler");
                    handle_s2s_connection_from_manager(conn, &manager, false).await;
                    tracing::warn!(peer = %peer_id, "S2S link dropped, will reconnect");
                }
                Err(e) => {
                    tracing::warn!(
                        peer = %peer_id,
                        backoff_secs = backoff.as_secs(),
                        "S2S connect failed: {e}"
                    );
                }
            }

            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    });
}

/// Convenience wrapper that extracts fields from the manager.
async fn handle_s2s_connection_from_manager(
    conn: iroh::endpoint::Connection,
    manager: &Arc<S2sManager>,
    incoming: bool,
) {
    let peers = Arc::clone(&manager.peers);
    let peer_names = Arc::clone(&manager.peer_names);
    let event_tx: mpsc::Sender<AuthenticatedS2sEvent> = manager.event_tx.clone();
    let server_id = manager.server_id.clone();
    let server_name = manager.server_name.clone();
    let conn_gen = Arc::clone(&manager.conn_gen);
    let dedup = Arc::clone(&manager.dedup);
    handle_s2s_connection(conn, peers, peer_names, event_tx, server_id, server_name, conn_gen, dedup, incoming).await;
}

/// Handle an S2S connection (both incoming and outgoing).
async fn handle_s2s_connection(
    conn: iroh::endpoint::Connection,
    peers: Arc<tokio::sync::Mutex<HashMap<String, PeerEntry>>>,
    peer_names: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    event_tx: mpsc::Sender<AuthenticatedS2sEvent>,
    server_id: String,
    server_name: String,
    conn_gen: Arc<AtomicU64>,
    dedup: Arc<DedupSet>,
    incoming: bool,
) {
    let peer_id = conn.remote_id().to_string();

    // For incoming: accept_bi, for outgoing: open_bi
    let (send, recv) = if incoming {
        match conn.accept_bi().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(peer = %peer_id, "S2S accept_bi failed: {e}");
                return;
            }
        }
    } else {
        match conn.open_bi().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(peer = %peer_id, "S2S open_bi failed: {e}");
                return;
            }
        }
    };

    // Write channel — with duplicate connection tie-breaking.
    // When both servers have each other in --s2s-peers, we get two QUIC
    // connections (one outgoing, one incoming).  Deterministic rule:
    //   The peer with the LOWER endpoint ID keeps its OUTGOING connection.
    //   The peer with the HIGHER endpoint ID keeps the INCOMING connection.
    // This means: drop if `incoming == (our_id < peer_id)`.
    let (write_tx, mut write_rx) = mpsc::channel::<S2sMessage>(256);
    let my_gen = conn_gen.fetch_add(1, Ordering::Relaxed);
    {
        let mut peers_guard = peers.lock().await;
        if peers_guard.contains_key(&peer_id) {
            tracing::info!(
                peer = %peer_id, incoming, gen = my_gen,
                "S2S duplicate connection — replacing existing (generation-safe cleanup)"
            );
        }
        peers_guard.insert(peer_id.clone(), PeerEntry { tx: write_tx, conn_gen: my_gen });
    }

    tracing::info!(peer = %peer_id, incoming, "S2S link established");

    // Bridge QUIC recv → DuplexStream for BufReader line reading
    let (bridge_side, irc_side) = tokio::io::duplex(16384);
    let (_bridge_read, mut bridge_write) = tokio::io::split(bridge_side);

    let recv_peer = peer_id.clone();
    tokio::spawn(async move {
        let mut recv = recv;
        let mut buf = vec![0u8; 4096];
        let mut bytes_received: u64 = 0;
        loop {
            match recv.read(&mut buf).await {
                Ok(Some(n)) => {
                    bytes_received += n as u64;
                    if bridge_write.write_all(&buf[..n]).await.is_err() {
                        tracing::warn!(peer = %recv_peer, "S2S recv bridge write failed after {bytes_received} bytes");
                        break;
                    }
                }
                Ok(None) => {
                    tracing::info!(peer = %recv_peer, "S2S recv stream finished (EOF) after {bytes_received} bytes");
                    break;
                }
                Err(e) => {
                    tracing::warn!(peer = %recv_peer, "S2S recv error after {bytes_received} bytes: {e}");
                    break;
                }
            }
        }
        let _ = bridge_write.shutdown().await;
    });

    // Read JSON lines from the peer.
    // Each message is wrapped with the transport-authenticated peer ID
    // (from conn.remote_id()) so the event processor can trust the identity
    // without relying on the `origin` field in the JSON payload.
    let read_peer = peer_id.clone();
    let authenticated_peer_id = peer_id.clone(); // from conn.remote_id() — cryptographic
    let read_event_tx = event_tx.clone();
    let read_handle = tokio::spawn(async move {
        let reader = BufReader::new(irc_side);
        let mut lines = reader.lines();
        let mut msg_count: u64 = 0;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    match serde_json::from_str::<S2sMessage>(&line) {
                        Ok(msg) => {
                            msg_count += 1;
                            tracing::debug!(peer = %read_peer, msg_count, "S2S received: {}", line.chars().take(120).collect::<String>());
                            let event = AuthenticatedS2sEvent {
                                authenticated_peer_id: authenticated_peer_id.clone(),
                                msg,
                            };
                            if read_event_tx.send(event).await.is_err() {
                                tracing::warn!(peer = %read_peer, "S2S event_tx closed after {msg_count} messages");
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(peer = %read_peer, "S2S invalid JSON: {e} — line: {}", line.chars().take(200).collect::<String>());
                        }
                    }
                }
                Ok(None) => {
                    tracing::info!(peer = %read_peer, "S2S read EOF after {msg_count} messages");
                    break;
                }
                Err(e) => {
                    tracing::warn!(peer = %read_peer, "S2S read error after {msg_count} messages: {e}");
                    break;
                }
            }
        }
    });

    // Write JSON lines to the peer
    let write_peer = peer_id.clone();
    let write_handle = tokio::spawn(async move {
        let mut send = send;
        let mut msg_count: u64 = 0;
        while let Some(msg) = write_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    msg_count += 1;
                    let line = format!("{json}\n");
                    tracing::debug!(peer = %write_peer, msg_count, "S2S sending: {}", json.chars().take(120).collect::<String>());
                    if let Err(e) = send.write_all(line.as_bytes()).await {
                        tracing::warn!(peer = %write_peer, "S2S write error after {msg_count} messages: {e}");
                        break;
                    }
                    if let Err(e) = send.flush().await {
                        tracing::warn!(peer = %write_peer, "S2S flush error after {msg_count} messages: {e}");
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(peer = %write_peer, "S2S serialize error: {e}");
                }
            }
        }
        tracing::info!(peer = %write_peer, "S2S write channel closed after {msg_count} messages");
        let _ = send.finish();
    });

    // Send Hello handshake — binds transport identity to logical name.
    // The receiver verifies peer_id matches connection's remote_id().
    {
        let hello = S2sMessage::Hello {
            peer_id: server_id.clone(),
            server_name: server_name.clone(),
        };
        if let Some(entry) = peers.lock().await.get(&peer_id) {
            let _ = entry.tx.send(hello).await;
        }
    }

    // Both sides send sync request
    {
        let sync_req = S2sMessage::SyncRequest;
        if let Some(entry) = peers.lock().await.get(&peer_id) {
            let _ = entry.tx.send(sync_req).await;
        }
    }

    // Wait for either direction to end
    let mut read_handle = read_handle;
    let mut write_handle = write_handle;
    let which = tokio::select! {
        _ = &mut read_handle => "read",
        _ = &mut write_handle => "write",
    };
    tracing::warn!(peer = %peer_id, side = which, gen = my_gen, "S2S link ending — {which} task finished first");

    // Only remove peer entry if it's still ours (same generation).
    // A replacement connection may have already inserted a new entry.
    {
        let mut peers_guard = peers.lock().await;
        if let Some(entry) = peers_guard.get(&peer_id) {
            if entry.conn_gen == my_gen {
                peers_guard.remove(&peer_id);
                peer_names.lock().await.remove(&peer_id);
                dedup.remove_peer(&peer_id).await;
                tracing::info!(peer = %peer_id, gen = my_gen, "S2S link closed (entry removed)");

                // Emit PeerDisconnected so the event processor can clean up
                // remote_members for this peer's origin. This prevents ghost
                // users lingering in channel rosters after a link drop.
                let _ = event_tx.send(AuthenticatedS2sEvent {
                    authenticated_peer_id: peer_id.clone(),
                    msg: S2sMessage::PeerDisconnected { peer_id: peer_id.clone() },
                }).await;
            } else {
                tracing::info!(
                    peer = %peer_id, my_gen, current_gen = entry.conn_gen,
                    "S2S link closed (entry kept — newer connection exists)"
                );
            }
        }
    }
}
