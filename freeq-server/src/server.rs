//! Server state and TCP listener.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};
use freeq_sdk::did::DidResolver;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::rustls;
use tokio_rustls::TlsAcceptor;

use crate::config::ServerConfig;
use crate::connection;
use crate::db::Db;
use crate::plugin::PluginManager;
use crate::sasl::ChallengeStore;

/// State for a single channel.
#[derive(Debug, Clone, Default)]
pub struct ChannelState {
    /// Session IDs of local members currently in the channel.
    pub members: HashSet<String>,
    /// Remote members from S2S peers: nick → RemoteMember info.
    pub remote_members: HashMap<String, RemoteMember>,
    /// Session IDs of channel operators (ephemeral, per-session).
    pub ops: HashSet<String>,
    /// Session IDs of voiced users.
    pub voiced: HashSet<String>,

    // ── DID-based persistent authority ──────────────────────────
    /// Channel founder's DID. Set once on channel creation.
    /// Founder always has ops and can't be de-opped.
    /// In S2S: resolved by CRDT (first-write-wins in Automerge causal order),
    /// NOT by timestamps — timestamps can be spoofed by rogue servers.
    pub founder_did: Option<String>,
    /// DIDs with persistent operator status.
    /// Survives reconnects, works across servers.
    /// Granted by founder or other DID-ops.
    pub did_ops: HashSet<String>,
    /// Timestamp (unix secs) when the channel was created (informational only).
    /// NOT used for authority resolution — the CRDT handles that.
    pub created_at: u64,

    /// Ban list: hostmasks (nick!user@host patterns) and/or DIDs.
    pub bans: Vec<BanEntry>,
    /// Invite-only mode (+i).
    pub invite_only: bool,
    /// Invite list (session IDs or DIDs that have been invited).
    pub invites: HashSet<String>,
    /// Recent message history for replay on join.
    pub history: std::collections::VecDeque<HistoryMessage>,
    /// Channel topic, if set.
    pub topic: Option<TopicInfo>,
    /// Channel modes: +t = only ops can set topic.
    pub topic_locked: bool,
    /// Channel mode: +n = no external messages (only members can send).
    pub no_ext_msg: bool,
    /// Channel mode: +m = moderated (only voiced/ops can send).
    pub moderated: bool,
    /// Channel key (+k) — password required to join.
    pub key: Option<String>,
}

/// Pending OAuth authorization: stored between /auth/login and /auth/callback.
#[derive(Debug, Clone)]
pub struct OAuthPending {
    pub handle: String,
    pub did: String,
    pub pds_url: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub token_endpoint: String,
    pub dpop_key_b64: String,
    pub created_at: u64,
}

/// Completed OAuth: stored after /auth/callback, consumed by the web client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OAuthResult {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub pds_url: String,
}

/// Info about a remote user connected via S2S federation.
#[derive(Debug, Clone, Default)]
pub struct RemoteMember {
    /// Iroh endpoint ID of the origin server.
    pub origin: String,
    /// Authenticated DID (if any).
    pub did: Option<String>,
    /// Resolved AT Protocol handle (e.g. "chadfowler.com").
    pub handle: Option<String>,
    /// Whether this user is op on their home server.
    pub is_op: bool,
}

/// A stored message for channel history replay.
#[derive(Debug, Clone)]
pub struct HistoryMessage {
    pub from: String,
    pub text: String,
    pub timestamp: u64,
    /// IRCv3 tags from the original message (for rich media replay).
    pub tags: HashMap<String, String>,
}

/// Maximum number of history messages to keep per channel.
pub const MAX_HISTORY: usize = 100;

/// A ban entry — can be a traditional hostmask or a DID.
#[derive(Debug, Clone)]
pub struct BanEntry {
    pub mask: String,
    pub set_by: String,
    pub set_at: u64,
}

impl BanEntry {
    pub fn new(mask: String, set_by: String) -> Self {
        let set_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self { mask, set_by, set_at }
    }

    /// Check if this ban matches a user.
    ///
    /// Supports:
    /// - DID bans: mask starts with "did:" — matches against authenticated DID
    /// - Hostmask bans: simple wildcard matching against nick!user@host
    pub fn matches(&self, hostmask: &str, did: Option<&str>) -> bool {
        if self.mask.starts_with("did:") {
            // DID-based ban: exact match
            did.is_some_and(|d| d == self.mask)
        } else {
            // Hostmask ban: simple wildcard match
            wildcard_match(&self.mask, hostmask)
        }
    }
}

/// Simple wildcard matching (* and ?).
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let text = text.to_lowercase();
    wildcard_match_inner(pattern.as_bytes(), text.as_bytes())
}

fn wildcard_match_inner(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            // * matches zero or more characters
            wildcard_match_inner(&pattern[1..], text)
                || (!text.is_empty() && wildcard_match_inner(pattern, &text[1..]))
        }
        (Some(b'?'), Some(_)) => wildcard_match_inner(&pattern[1..], &text[1..]),
        (Some(a), Some(b)) if a == b => wildcard_match_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}

impl ChannelState {
    /// Check if a user is banned from this channel.
    pub fn is_banned(&self, hostmask: &str, did: Option<&str>) -> bool {
        self.bans.iter().any(|b| b.matches(hostmask, did))
    }
}

/// Channel topic with metadata.
#[derive(Debug, Clone)]
pub struct TopicInfo {
    pub text: String,
    pub set_by: String,
    pub set_at: u64,
}

impl TopicInfo {
    pub fn new(text: String, set_by: String) -> Self {
        let set_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            text,
            set_by,
            set_at,
        }
    }
}

/// Shared state accessible by all connection handlers.
pub struct SharedState {
    pub server_name: String,
    pub challenge_store: ChallengeStore,
    pub did_resolver: DidResolver,
    /// session_id -> sender for writing lines to that client
    pub connections: Mutex<HashMap<String, mpsc::Sender<String>>>,
    /// nick -> session_id
    pub nick_to_session: Mutex<HashMap<String, String>>,
    /// session_id -> authenticated DID (for WHOIS lookups by other connections)
    pub session_dids: Mutex<HashMap<String, String>>,
    /// DID -> owned nick (persistent identity-nick binding).
    /// When a user authenticates, they claim their nick. No one else can use it.
    pub did_nicks: Mutex<HashMap<String, String>>,
    /// nick -> DID (reverse lookup for nick enforcement).
    pub nick_owners: Mutex<HashMap<String, String>>,
    /// session_id -> resolved Bluesky handle (for WHOIS display).
    pub session_handles: Mutex<HashMap<String, String>>,
    /// channel name -> channel state (keys are always lowercase)
    pub channels: Mutex<HashMap<String, ChannelState>>,
    /// Sessions that have negotiated message-tags capability.
    pub cap_message_tags: Mutex<HashSet<String>>,
    /// Sessions that have negotiated multi-prefix capability.
    pub cap_multi_prefix: Mutex<HashSet<String>>,
    /// Sessions that have negotiated echo-message capability.
    pub cap_echo_message: Mutex<HashSet<String>>,
    /// Sessions that have negotiated server-time capability.
    pub cap_server_time: Mutex<HashSet<String>>,
    /// Sessions that have negotiated batch capability.
    pub cap_batch: Mutex<HashSet<String>>,
    pub cap_account_notify: Mutex<HashSet<String>>,
    pub cap_extended_join: Mutex<HashSet<String>>,
    /// Pending OAuth sessions: state → OAuthPending.
    pub oauth_pending: Mutex<HashMap<String, OAuthPending>>,
    /// Completed OAuth sessions: state → OAuthResult.
    pub oauth_complete: Mutex<HashMap<String, OAuthResult>>,
    /// session_id -> iroh endpoint ID (for connections via iroh transport).
    pub session_iroh_ids: Mutex<HashMap<String, String>>,
    /// session_id -> away message (None = not away).
    pub session_away: Mutex<HashMap<String, String>>,
    /// This server's own iroh endpoint ID (advertised in CAP LS).
    pub server_iroh_id: Mutex<Option<String>>,
    /// Iroh endpoint handle (kept alive for the server's lifetime).
    pub iroh_endpoint: Mutex<Option<iroh::Endpoint>>,
    /// S2S manager (if clustering is active).
    pub s2s_manager: Mutex<Option<Arc<crate::s2s::S2sManager>>>,
    /// CRDT document for cluster state convergence.
    pub cluster_doc: crate::crdt::ClusterDoc,
    /// Database handle for persistence (None = in-memory only).
    pub db: Option<Mutex<Db>>,
    /// Server configuration (for MOTD, max messages, etc.).
    pub config: ServerConfig,
    /// Plugin manager for server extensions.
    pub plugin_manager: PluginManager,
}

impl SharedState {
    /// Run a closure with the database, if persistence is enabled.
    /// Logs errors but does not propagate them — persistence failures
    /// should not break the IRC server.
    pub fn with_db<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Db) -> rusqlite::Result<R>,
    {
        self.db.as_ref().and_then(|db| {
            let db = db.lock().unwrap();
            match f(&db) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::error!("Database error: {e}");
                    None
                }
            }
        })
    }

    // ── CRDT operations ────────────────────────────────────────────
    //
    // NOTE: Presence (join/part) is NOT in CRDT. It's handled by S2S events
    // with periodic resync. This avoids ghost users when servers crash
    // without emitting PART/QUIT.
    //
    // All CRDT methods are async because ClusterDoc uses tokio::sync::Mutex.

    /// Get our iroh endpoint ID (used as CRDT peer identity).
    fn crdt_origin_peer(&self) -> String {
        self.server_iroh_id.lock().unwrap().clone()
            .unwrap_or_else(|| self.server_name.clone())
    }

    /// Record a topic change in the CRDT with provenance.
    pub async fn crdt_set_topic(&self, channel: &str, topic: &str, set_by: &str, set_by_did: Option<&str>) {
        let origin = self.crdt_origin_peer();
        self.cluster_doc.set_topic(channel, topic, set_by, set_by_did, &origin).await;
    }

    /// Record a nick-DID binding in the CRDT.
    pub async fn crdt_set_nick_owner(&self, nick: &str, did: &str) {
        self.cluster_doc.set_nick_owner(nick, did).await;
    }

    /// Record a channel founder in the CRDT.
    pub async fn crdt_set_founder(&self, channel: &str, did: &str) {
        self.cluster_doc.set_founder(channel, did).await;
    }

    /// Record a DID op grant in the CRDT with provenance.
    pub async fn crdt_grant_op(&self, channel: &str, did: &str, granted_by_did: Option<&str>) {
        let origin = self.crdt_origin_peer();
        self.cluster_doc.grant_op(channel, did, granted_by_did, &origin).await;
    }

    /// Record a DID op revoke in the CRDT.
    pub async fn crdt_revoke_op(&self, channel: &str, did: &str) {
        self.cluster_doc.revoke_op(channel, did).await;
    }

    /// Record a ban in the CRDT with provenance.
    pub async fn crdt_add_ban(&self, channel: &str, mask: &str, set_by: &str, set_by_did: Option<&str>) {
        let origin = self.crdt_origin_peer();
        self.cluster_doc.add_ban(channel, mask, set_by, set_by_did, &origin).await;
    }

    /// Record a ban removal in the CRDT.
    pub async fn crdt_remove_ban(&self, channel: &str, mask: &str) {
        self.cluster_doc.remove_ban(channel, mask).await;
    }

    /// Generate CRDT sync messages for all peers and broadcast them.
    /// Sync state is keyed by **iroh endpoint ID** (cryptographic identity).
    pub async fn crdt_broadcast_sync(&self) {
        let manager = self.s2s_manager.lock().unwrap().clone();
        let manager = match manager {
            Some(m) => m,
            None => return,
        };

        // Use iroh endpoint ID as our origin in CRDT sync messages
        let our_peer_id = manager.server_id.clone();

        let peers: Vec<String> = manager.peers.lock().await.keys().cloned().collect();
        for peer_id in &peers {
            // peer_id here is already the iroh endpoint ID (from connection's remote_id)
            if let Some(msg_bytes) = self.cluster_doc.generate_sync_message(peer_id).await {
                let sync_msg = crate::s2s::S2sMessage::CrdtSync {
                    data: {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.encode(&msg_bytes)
                    },
                    // Use iroh endpoint ID as origin (not server_name)
                    origin: our_peer_id.clone(),
                };
                if let Some(entry) = manager.peers.lock().await.get(peer_id) {
                    let _ = entry.tx.send(sync_msg).await;
                }
            }
        }
    }

    /// Receive a CRDT sync message from a peer.
    /// `peer_id` MUST be the iroh endpoint ID (not server_name).
    pub async fn crdt_receive_sync(&self, peer_id: &str, data: &[u8]) -> Result<(), String> {
        self.cluster_doc.receive_sync_message(peer_id, data).await
    }

    /// Send the next CRDT sync message to a specific peer only.
    ///
    /// This is the correct response after receiving a sync message from a peer:
    /// generate the next Automerge sync message for that peer and send it back.
    /// This avoids broadcast amplification storms where receiving from one peer
    /// triggers messages to all peers, which all respond, etc.
    pub async fn crdt_sync_with_peer(&self, peer_id: &str) {
        let manager = self.s2s_manager.lock().unwrap().clone();
        let manager = match manager {
            Some(m) => m,
            None => return,
        };

        let our_peer_id = manager.server_id.clone();

        if let Some(msg_bytes) = self.cluster_doc.generate_sync_message(peer_id).await {
            let sync_msg = crate::s2s::S2sMessage::CrdtSync {
                data: {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(&msg_bytes)
                },
                origin: our_peer_id,
            };
            if let Some(entry) = manager.peers.lock().await.get(peer_id) {
                let _ = entry.tx.send(sync_msg).await;
            }
        }
    }
}

pub struct Server {
    config: ServerConfig,
    resolver: DidResolver,
}

impl Server {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            resolver: DidResolver::http(),
            config,
        }
    }

    /// Create a server with a custom DID resolver (for testing).
    pub fn with_resolver(config: ServerConfig, resolver: DidResolver) -> Self {
        Self { config, resolver }
    }

    /// Build SharedState, opening the database and loading persisted data.
    fn build_state(&self) -> Result<Arc<SharedState>> {
        let db = match &self.config.db_path {
            Some(path) => {
                tracing::info!("Opening database: {path}");
                Some(Db::open(path).map_err(|e| anyhow::anyhow!("Failed to open database: {e}"))?)
            }
            None => None,
        };

        // Load persisted state from DB
        let mut channels = HashMap::new();
        let mut did_nicks = HashMap::new();
        let mut nick_owners = HashMap::new();

        if let Some(ref db) = db {
            // Load channels (metadata + bans)
            channels = db.load_channels()
                .map_err(|e| anyhow::anyhow!("Failed to load channels: {e}"))?;
            tracing::info!("Loaded {} channels from database", channels.len());

            // Load message history into each channel
            for (name, ch) in channels.iter_mut() {
                let messages = db.get_messages(name, crate::server::MAX_HISTORY, None)
                    .map_err(|e| anyhow::anyhow!("Failed to load messages for {name}: {e}"))?;
                for msg in messages {
                    ch.history.push_back(HistoryMessage {
                        from: msg.sender,
                        text: msg.text,
                        timestamp: msg.timestamp,
                        tags: msg.tags,
                    });
                }
            }

            // Load DID-nick bindings
            let identities = db.load_identities()
                .map_err(|e| anyhow::anyhow!("Failed to load identities: {e}"))?;
            tracing::info!("Loaded {} identity bindings from database", identities.len());
            for id in identities {
                nick_owners.insert(id.nick.clone(), id.did.clone());
                did_nicks.insert(id.did, id.nick);
            }
        }

        let plugin_manager = PluginManager::load(
            &self.config.plugins,
            self.config.plugin_dir.as_deref(),
        );

        Ok(Arc::new(SharedState {
            server_name: self.config.server_name.clone(),
            challenge_store: ChallengeStore::new(self.config.challenge_timeout_secs),
            did_resolver: self.resolver.clone(),
            connections: Mutex::new(HashMap::new()),
            nick_to_session: Mutex::new(HashMap::new()),
            session_dids: Mutex::new(HashMap::new()),
            channels: Mutex::new(channels),
            did_nicks: Mutex::new(did_nicks),
            nick_owners: Mutex::new(nick_owners),
            session_handles: Mutex::new(HashMap::new()),
            cap_message_tags: Mutex::new(HashSet::new()),
            cap_multi_prefix: Mutex::new(HashSet::new()),
            cap_echo_message: Mutex::new(HashSet::new()),
            cap_server_time: Mutex::new(HashSet::new()),
            cap_batch: Mutex::new(HashSet::new()),
            cap_account_notify: Mutex::new(HashSet::new()),
            cap_extended_join: Mutex::new(HashSet::new()),
            oauth_pending: Mutex::new(HashMap::new()),
            oauth_complete: Mutex::new(HashMap::new()),
            session_iroh_ids: Mutex::new(HashMap::new()),
            session_away: Mutex::new(HashMap::new()),
            server_iroh_id: Mutex::new(None),
            iroh_endpoint: Mutex::new(None),
            s2s_manager: Mutex::new(None),
            cluster_doc: crate::crdt::ClusterDoc::new(&self.config.server_name),
            db: db.map(Mutex::new),
            config: self.config.clone(),
            plugin_manager,
        }))
    }

    /// Run the server, blocking forever.
    pub async fn run(self) -> Result<()> {
        let tls_acceptor = self.build_tls_acceptor()?;
        let web_addr = self.config.web_addr.clone();
        let state = self.build_state()?;

        // Start plain listener
        let plain_listener = TcpListener::bind(&self.config.listen_addr).await?;
        tracing::info!("Plain listener on {}", self.config.listen_addr);

        // Start TLS listener if configured
        if let Some(ref acceptor) = tls_acceptor {
            let tls_listener = TcpListener::bind(&self.config.tls_listen_addr).await?;
            tracing::info!("TLS listener on {}", self.config.tls_listen_addr);

            let tls_state = Arc::clone(&state);
            let tls_acc = acceptor.clone();
            tokio::spawn(async move {
                loop {
                    match tls_listener.accept().await {
                        Ok((stream, _)) => {
                            let state = Arc::clone(&tls_state);
                            let acceptor = tls_acc.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        if let Err(e) =
                                            connection::handle_generic(tls_stream, state).await
                                        {
                                            tracing::error!("TLS connection error: {e}");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("TLS handshake failed: {e}");
                                    }
                                }
                            });
                        }
                        Err(e) => tracing::error!("TLS accept error: {e}"),
                    }
                }
            });
        }

        // Start iroh transport if configured
        let iroh_endpoint = if self.config.iroh || !self.config.s2s_peers.is_empty() {
            let iroh_state = Arc::clone(&state);
            let iroh_port = self.config.iroh_port;
            match crate::iroh::start(iroh_state, iroh_port).await {
                Ok(endpoint) => {
                    // Wait for the endpoint to be online and print connection info
                    endpoint.online().await;
                    let id = endpoint.id();
                    tracing::info!("Iroh ready. Connect with: --iroh-addr {id}");
                    *state.server_iroh_id.lock().unwrap() = Some(id.to_string());

                    // Re-key the CRDT actor to the iroh endpoint ID.
                    // This MUST happen before any S2S connections, so founder
                    // resolution (min-actor-wins) uses the cryptographic identity.
                    state.cluster_doc.rekey_actor(&id.to_string()).await;

                    Some(endpoint)
                }
                Err(e) => {
                    tracing::error!("Failed to start iroh endpoint: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Start S2S manager whenever iroh is enabled (not just when peers are configured).
        // This allows the server to accept incoming S2S connections from other servers.
        if let Some(ref endpoint) = iroh_endpoint {
            let s2s_state = Arc::clone(&state);
            match crate::s2s::start(s2s_state, endpoint.clone()).await {
                Ok((manager, mut s2s_rx)) => {
                    // Store manager in shared state so iroh accept loop can route S2S
                    *state.s2s_manager.lock().unwrap() = Some(Arc::clone(&manager));

                    // Connect to configured peers with auto-reconnection
                    for peer_id in &self.config.s2s_peers {
                        crate::s2s::connect_peer_with_retry(
                            endpoint.clone(),
                            peer_id.clone(),
                            Arc::clone(&manager),
                        );
                    }

                    // Spawn S2S event processor
                    let s2s_state = Arc::clone(&state);
                    let s2s_manager = Arc::clone(&manager);
                    tokio::spawn(async move {
                        while let Some(event) = s2s_rx.recv().await {
                            process_s2s_message(
                                &s2s_state,
                                &s2s_manager,
                                &event.authenticated_peer_id,
                                event.msg,
                            ).await;
                        }
                    });

                    if self.config.s2s_peers.is_empty() {
                        tracing::info!("S2S ready (accepting incoming peer connections)");
                    } else {
                        tracing::info!(
                            "S2S clustering active with {} peer(s)",
                            self.config.s2s_peers.len()
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to start S2S: {e}");
                }
            }
        } else if !self.config.s2s_peers.is_empty() {
            tracing::error!("S2S requires iroh transport (--iroh)");
        }

        // Store iroh endpoint in shared state to keep it alive
        if let Some(endpoint) = iroh_endpoint {
            *state.iroh_endpoint.lock().unwrap() = Some(endpoint);
        }

        // Start periodic CRDT maintenance tasks:
        // 1. Compaction (every 30 min) — bounds doc growth
        // 2. CRDT→local reconciliation (every 60s) — ensures CRDT is source of truth
        {
            let compact_state = Arc::clone(&state);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
                interval.tick().await; // skip first immediate tick
                loop {
                    interval.tick().await;
                    let metrics = compact_state.cluster_doc.get_metrics().await;
                    tracing::info!(
                        "CRDT metrics: {} changes, {} sync msgs sent, {} recv, last save {}B",
                        metrics.change_count,
                        metrics.sync_messages_sent,
                        metrics.sync_messages_received,
                        metrics.last_save_size,
                    );
                    if let Err(e) = compact_state.cluster_doc.compact().await {
                        tracing::error!("CRDT compaction failed: {e}");
                    } else {
                        tracing::info!("CRDT compacted successfully");
                    }
                }
            });
        }

        // CRDT→local reconciliation: periodically apply CRDT state to local
        // channel state. This ensures the CRDT is the single source of truth
        // for topics, founder, and DID ops — even if S2S events and CRDT
        // diverge due to timing/partitions.
        {
            let reconcile_state = Arc::clone(&state);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                interval.tick().await; // skip first tick
                loop {
                    interval.tick().await;
                    reconcile_crdt_to_local(&reconcile_state).await;
                }
            });
        }

        // Start HTTP/WebSocket listener if configured
        if let Some(ref addr) = web_addr {
            let web_state = Arc::clone(&state);
            let router = crate::web::router(web_state);
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!("HTTP/WebSocket listener on {addr}");
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, router).await {
                    tracing::error!("HTTP server error: {e}");
                }
            });
        }

        // Accept plain connections
        loop {
            let (stream, _addr) = plain_listener.accept().await?;
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                if let Err(e) = connection::handle(stream, state).await {
                    tracing::error!("Connection error: {e}");
                }
            });
        }
    }

    /// Start the server and return the bound address + task handle (for testing).
    pub async fn start(self) -> Result<(SocketAddr, JoinHandle<Result<()>>)> {
        let listener = TcpListener::bind(&self.config.listen_addr).await?;
        let addr = listener.local_addr()?;
        tracing::info!("Listening on {addr}");

        let state = self.build_state()?;

        let handle = tokio::spawn(async move {
            loop {
                let (stream, _addr) = listener.accept().await?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = connection::handle(stream, state).await {
                        tracing::error!("Connection error: {e}");
                    }
                });
            }
        });

        Ok((addr, handle))
    }

    /// Start the server with both plain and TLS listeners for testing.
    /// Returns (plain_addr, tls_addr, handle).
    pub async fn start_tls(self) -> Result<(SocketAddr, SocketAddr, JoinHandle<Result<()>>)> {
        let tls_acceptor = self.build_tls_acceptor()?
            .expect("TLS must be configured for start_tls()");

        let plain_listener = TcpListener::bind(&self.config.listen_addr).await?;
        let plain_addr = plain_listener.local_addr()?;

        let tls_listener = TcpListener::bind(&self.config.tls_listen_addr).await?;
        let tls_addr = tls_listener.local_addr()?;

        tracing::info!("Plain on {plain_addr}, TLS on {tls_addr}");

        let state = self.build_state()?;

        let handle = tokio::spawn(async move {
            let tls_state = Arc::clone(&state);
            let tls_acc = tls_acceptor.clone();
            tokio::spawn(async move {
                loop {
                    match tls_listener.accept().await {
                        Ok((stream, _)) => {
                            let state = Arc::clone(&tls_state);
                            let acceptor = tls_acc.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        if let Err(e) = connection::handle_generic(tls_stream, state).await {
                                            tracing::error!("TLS connection error: {e}");
                                        }
                                    }
                                    Err(e) => tracing::warn!("TLS handshake failed: {e}"),
                                }
                            });
                        }
                        Err(e) => tracing::error!("TLS accept error: {e}"),
                    }
                }
            });

            loop {
                let (stream, _) = plain_listener.accept().await?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = connection::handle(stream, state).await {
                        tracing::error!("Connection error: {e}");
                    }
                });
            }
        });

        Ok((plain_addr, tls_addr, handle))
    }

    fn build_tls_acceptor(&self) -> Result<Option<TlsAcceptor>> {
        if !self.config.tls_enabled() {
            return Ok(None);
        }

        let cert_path = self.config.tls_cert.as_deref().unwrap();
        let key_path = self.config.tls_key.as_deref().unwrap();

        let cert_pem = std::fs::read(cert_path)
            .with_context(|| format!("Failed to read TLS cert: {cert_path}"))?;
        let key_pem = std::fs::read(key_path)
            .with_context(|| format!("Failed to read TLS key: {key_path}"))?;

        let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse TLS certificates")?;
        let key = rustls_pemfile::private_key(&mut &key_pem[..])
            .context("Failed to parse TLS private key")?
            .context("No private key found in PEM file")?;

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("Invalid TLS configuration")?;

        Ok(Some(TlsAcceptor::from(Arc::new(config))))
    }
}

/// Process an S2S message received from a peer server.
///
/// Delivers relayed messages to local clients. Currently handles
/// PRIVMSG, JOIN, PART, QUIT, NICK, TOPIC, and sync.
///
/// Remote users are identified by nick (not session ID). We deliver
/// to local sessions that are members of the target channel.
async fn process_s2s_message(
    state: &Arc<SharedState>,
    manager: &Arc<crate::s2s::S2sManager>,
    authenticated_peer_id: &str,
    msg: crate::s2s::S2sMessage,
) {
    use crate::s2s::S2sMessage;

    /// Deliver a raw IRC line to all local members of a channel.
    fn deliver_to_channel(state: &SharedState, channel: &str, line: &str) {
        let channel_key = channel.to_lowercase();
        let channels = state.channels.lock().unwrap();
        if let Some(ch) = channels.get(&channel_key) {
            let conns = state.connections.lock().unwrap();
            for session_id in &ch.members {
                if let Some(tx) = conns.get(session_id) {
                    let _ = tx.try_send(line.to_string());
                }
            }
        }
    }

    /// Send NAMES update to all local members of a channel (for nick list refresh).
    fn send_names_update(state: &SharedState, channel: &str) {
        let channels = state.channels.lock().unwrap();
        let ch = match channels.get(channel) {
            Some(ch) => ch,
            None => return,
        };

        // Build nick list (local + remote)
        let n2s = state.nick_to_session.lock().unwrap();
        let reverse: HashMap<&String, &String> = n2s.iter().map(|(n, s)| (s, n)).collect();
        let mut nick_list: Vec<String> = ch.members.iter()
            .filter_map(|s| {
                reverse.get(s).map(|n| {
                    let prefix = if ch.ops.contains(s) { "@" }
                        else if ch.voiced.contains(s) { "+" }
                        else { "" };
                    format!("{prefix}{n}")
                })
            })
            .collect();
        for (nick, rm) in &ch.remote_members {
            let is_op = rm.is_op || rm.did.as_ref().is_some_and(|d| {
                ch.founder_did.as_deref() == Some(d) || ch.did_ops.contains(d)
            });
            let prefix = if is_op { "@" } else { "" };
            nick_list.push(format!("{prefix}{nick}"));
        }
        let nick_str = nick_list.join(" ");

        // Send to each local member
        let local_members: Vec<String> = ch.members.iter().cloned().collect();
        drop(channels);

        let conns = state.connections.lock().unwrap();
        for session_id in &local_members {
            // Look up this member's nick for the reply prefix
            let member_nick = reverse.get(session_id).map(|n| n.as_str()).unwrap_or("*");
            let names_line = format!(
                ":{} 353 {} = {} :{}\r\n:{} 366 {} {} :End of /NAMES list\r\n",
                state.server_name, member_nick, channel, nick_str,
                state.server_name, member_nick, channel,
            );
            if let Some(tx) = conns.get(session_id) {
                let _ = tx.try_send(names_line);
            }
        }
    }

    // ── Event dedup ──────────────────────────────────────────────
    // Extract event_id and origin from message for dedup check.
    // Messages with empty event_id (legacy peers) skip dedup.
    let (event_id, origin) = match &msg {
        S2sMessage::Privmsg { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::Join { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::Part { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::Quit { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::NickChange { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::Topic { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::Mode { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::ChannelCreated { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::Kick { event_id, origin, .. } => (event_id.clone(), origin.clone()),
        S2sMessage::CrdtSync { origin, .. } => (String::new(), origin.clone()),
        S2sMessage::PeerDisconnected { .. } => (String::new(), String::new()),
        S2sMessage::Hello { .. } | S2sMessage::SyncRequest | S2sMessage::SyncResponse { .. } => {
            (String::new(), String::new())
        }
    };

    // Skip our own messages
    if !origin.is_empty() && origin == manager.server_id {
        return;
    }

    // Dedup: reject duplicate event_ids
    if !event_id.is_empty() && !manager.dedup.check_and_insert(&origin, &event_id).await {
        tracing::debug!(event_id = %event_id, "S2S event deduplicated (already seen)");
        return;
    }

    match msg {
        S2sMessage::Hello { peer_id, server_name } => {
            // Verify the claimed peer_id matches the transport-authenticated identity.
            // The iroh QUIC connection provides cryptographic identity via remote_id().
            if peer_id != authenticated_peer_id {
                tracing::warn!(
                    authenticated = %authenticated_peer_id,
                    claimed = %peer_id,
                    server_name = %server_name,
                    "S2S Hello: claimed peer_id doesn't match transport identity — using authenticated ID"
                );
            }
            tracing::info!(
                peer = %authenticated_peer_id,
                server_name = %server_name,
                "S2S Hello received — binding transport identity to server name"
            );
            // Always key by the authenticated peer ID, not the claimed one.
            manager.peer_names.lock().await.insert(authenticated_peer_id.to_string(), server_name);
        }

        S2sMessage::Privmsg { from, target, text, origin: _, .. } => {
            let line = format!(":{from} PRIVMSG {target} :{text}\r\n");

            if target.starts_with('#') || target.starts_with('&') {
                // Enforce +n and +m on incoming S2S messages
                let channel_key = target.to_lowercase();
                let channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get(&channel_key) {
                    if ch.no_ext_msg {
                        let nick = from.split('!').next().unwrap_or(&from);
                        let is_member = ch.remote_members.contains_key(nick)
                            || ch.members.iter().any(|sid| {
                                state.nick_to_session.lock().unwrap()
                                    .iter().any(|(n, s)| n == nick && s == sid)
                            });
                        if !is_member {
                            tracing::debug!(channel = %target, from = %from, "S2S PRIVMSG blocked by +n");
                            return;
                        }
                    }
                    if ch.moderated {
                        let nick = from.split('!').next().unwrap_or(&from);
                        let is_privileged = ch.remote_members.get(nick)
                            .is_some_and(|rm| rm.is_op);
                        if !is_privileged {
                            tracing::debug!(channel = %target, from = %from, "S2S PRIVMSG blocked by +m");
                            return;
                        }
                    }
                }
                drop(channels);
                deliver_to_channel(state, &target, &line);
            } else {
                let n2s = state.nick_to_session.lock().unwrap();
                if let Some(sid) = n2s.get(&target) {
                    let conns = state.connections.lock().unwrap();
                    if let Some(tx) = conns.get(sid) {
                        let _ = tx.try_send(line);
                    }
                }
            }
        }

        S2sMessage::Join { nick, channel, did, handle, is_op, origin, .. } => {
            // Presence is S2S-event-only (NOT in CRDT — avoids ghost users)
            // Idempotent: set-based, don't assume not present
            {
                let mut channels = state.channels.lock().unwrap();
                let ch = channels.entry(channel.clone()).or_default();
                ch.remote_members.insert(nick.clone(), RemoteMember {
                    origin: origin.clone(),
                    did: did.clone(),
                    handle: handle.clone(),
                    is_op,
                });
            }

            let line = format!(":{nick}!{nick}@s2s JOIN {channel}\r\n");
            deliver_to_channel(state, &channel, &line);
            send_names_update(state, &channel);
        }

        S2sMessage::Part { nick, channel, .. } => {
            // Presence is S2S-event-only. Idempotent: remove if present.
            {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(&channel) {
                    ch.remote_members.remove(&nick);
                }
            }

            let line = format!(":{nick}!{nick}@s2s PART {channel}\r\n");
            deliver_to_channel(state, &channel, &line);
            send_names_update(state, &channel);
        }

        S2sMessage::Quit { nick, reason, .. } => {
            // Remove remote member from all channels (idempotent)
            let mut affected_channels = Vec::new();
            {
                let mut channels = state.channels.lock().unwrap();
                for (name, ch) in channels.iter_mut() {
                    if ch.remote_members.remove(&nick).is_some() {
                        affected_channels.push(name.clone());
                    }
                }
            }

            let line = format!(":{nick}!{nick}@s2s QUIT :{reason}\r\n");
            for ch_name in &affected_channels {
                deliver_to_channel(state, ch_name, &line);
                send_names_update(state, ch_name);
            }
        }

        S2sMessage::Topic { channel, topic, set_by, .. } => {
            // CRDT is the single source of truth for topic convergence.
            // The S2S Topic event is a notification for immediate display —
            // we apply it locally for UX responsiveness, then write to CRDT
            // for convergent persistence. On any divergence, CRDT wins.

            // Enforce +t locally
            {
                let channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get(&channel) {
                    if ch.topic_locked {
                        let is_authorized = ch.remote_members.get(&set_by)
                            .is_some_and(|rm| rm.is_op || rm.did.as_ref().is_some_and(|d| {
                                ch.founder_did.as_deref() == Some(d) || ch.did_ops.contains(d)
                            }));
                        if !is_authorized {
                            tracing::info!(
                                channel = %channel, set_by = %set_by,
                                "Rejecting S2S topic change: channel is +t and setter is not authorized"
                            );
                            return;
                        }
                    }
                }
            }

            // Write to CRDT (source of truth)
            let setter_did = {
                let channels = state.channels.lock().unwrap();
                channels.get(&channel).and_then(|ch| {
                    ch.remote_members.get(&set_by).and_then(|rm| rm.did.clone())
                })
            };
            state.crdt_set_topic(&channel, &topic, &set_by, setter_did.as_deref()).await;

            // Apply locally for immediate UX (CRDT is authoritative if they diverge)
            {
                let mut channels = state.channels.lock().unwrap();
                let ch = channels.entry(channel.clone()).or_default();
                ch.topic = Some(TopicInfo::new(topic.clone(), set_by.clone()));
            }

            let line = format!(":{set_by}!remote@s2s TOPIC {channel} :{topic}\r\n");
            deliver_to_channel(state, &channel, &line);
        }

        S2sMessage::ChannelCreated { channel, founder_did, did_ops, origin, .. } => {
            let has_local_members;
            {
                let mut channels = state.channels.lock().unwrap();
                let ch = channels.entry(channel.clone()).or_default();

                // ── Authority gating ───────────────────────────────────
                // Founder: only adopt if we have no local founder.
                // If we already have one, reject the remote claim — CRDT
                // convergence will resolve via min-actor-wins.
                if ch.founder_did.is_none() {
                    if let Some(ref did) = founder_did {
                        // Validate: the DID must look plausible (starts with "did:")
                        if did.starts_with("did:") {
                            tracing::info!(
                                channel = %channel, origin = %origin,
                                "Adopting remote founder {did} (no local founder)"
                            );
                            ch.founder_did = Some(did.clone());
                        } else {
                            tracing::warn!(
                                channel = %channel, origin = %origin,
                                "Rejecting invalid founder claim: {did}"
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        channel = %channel,
                        "Keeping local founder {:?} (ignoring remote {:?} from {origin})",
                        ch.founder_did, founder_did
                    );
                }

                // DID ops: validate format + authority before accepting.
                let require_did = state.config.require_did_for_ops;
                for did in &did_ops {
                    if !did.starts_with("did:") {
                        tracing::warn!(
                            channel = %channel, origin = %origin,
                            "Rejecting invalid DID op: {did}"
                        );
                        continue;
                    }
                    // Authority check: ops should be granted by founder or existing op
                    let granter = founder_did.as_deref();
                    let has_authority = granter.is_some()
                        || ch.founder_did.is_some()
                        || !ch.did_ops.is_empty();
                    if !has_authority {
                        if require_did {
                            tracing::warn!(
                                channel = %channel, origin = %origin,
                                "Rejecting DID op {did}: no authority and --require-did-for-ops is set"
                            );
                            continue;
                        }
                        tracing::warn!(
                            channel = %channel, origin = %origin,
                            "DID op {did} granted without known authority (accepting, use --require-did-for-ops to reject)"
                        );
                    }
                    ch.did_ops.insert(did.clone());
                }

                // Re-op local members
                has_local_members = !ch.members.is_empty();
                let members: Vec<String> = ch.members.iter().cloned().collect();
                let dids = state.session_dids.lock().unwrap();
                for session_id in &members {
                    if let Some(did) = dids.get(session_id) {
                        if ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did) {
                            ch.ops.insert(session_id.clone());
                        }
                    }
                }
            } // All MutexGuards dropped

            // Update CRDT with provenance
            if let Some(ref did) = founder_did {
                if did.starts_with("did:") {
                    state.crdt_set_founder(&channel, did).await;
                }
            }
            for did in &did_ops {
                if did.starts_with("did:") {
                    state.crdt_grant_op(&channel, did, founder_did.as_deref()).await;
                }
            }

            if has_local_members {
                send_names_update(state, &channel);
            }
        }

        S2sMessage::SyncRequest => {
            let response = {
                let channels = state.channels.lock().unwrap();
                let n2s = state.nick_to_session.lock().unwrap();
                let s2n: HashMap<&String, &String> = n2s.iter().map(|(n, s)| (s, n)).collect();

                let dids = state.session_dids.lock().unwrap();
                let channel_info: Vec<crate::s2s::ChannelInfo> = channels.iter().map(|(name, ch)| {
                    let nicks: Vec<String> = ch.members.iter()
                        .filter_map(|sid| s2n.get(sid).map(|n| (*n).clone()))
                        .collect();
                    let nick_info: Vec<crate::s2s::SyncNick> = ch.members.iter()
                        .filter_map(|sid| {
                            s2n.get(sid).map(|n| crate::s2s::SyncNick {
                                nick: (*n).clone(),
                                is_op: ch.ops.contains(sid),
                                did: dids.get(sid).cloned(),
                            })
                        })
                        .collect();
                    crate::s2s::ChannelInfo {
                        name: name.clone(),
                        topic: ch.topic.as_ref().map(|t| t.text.clone()),
                        nicks,
                        nick_info,
                        founder_did: ch.founder_did.clone(),
                        did_ops: ch.did_ops.iter().cloned().collect(),
                        created_at: ch.created_at,
                        topic_locked: ch.topic_locked,
                        invite_only: ch.invite_only,
                        no_ext_msg: ch.no_ext_msg,
                        moderated: ch.moderated,
                        key: ch.key.clone(),
                    }
                }).collect();

                S2sMessage::SyncResponse {
                    server_id: manager.server_id.clone(),
                    channels: channel_info,
                }
            };
            manager.broadcast(response);
            state.crdt_broadcast_sync().await;
        }

        S2sMessage::SyncResponse { server_id: peer_id, channels: remote_channels } => {
            tracing::info!(
                "Received sync: {} channel(s) from peer {peer_id}",
                remote_channels.len()
            );
            let mut updated_channels = Vec::new();
            {
                let mut channels = state.channels.lock().unwrap();

                // Clear stale remote members from this peer before merging.
                // SyncResponse is a full state snapshot — any remote members
                // from this peer that aren't in the response are gone.
                // This prevents ghost users after a peer restarts with fewer members.
                let synced_channel_names: std::collections::HashSet<String> =
                    remote_channels.iter().map(|i| i.name.clone()).collect();
                for (name, ch) in channels.iter_mut() {
                    if synced_channel_names.contains(name) {
                        // Will be replaced below per-channel
                        ch.remote_members.retain(|_nick, rm| rm.origin != peer_id);
                    } else {
                        // Peer didn't mention this channel — remove their members from it
                        ch.remote_members.retain(|_nick, rm| rm.origin != peer_id);
                    }
                }

                for info in remote_channels {
                    let ch = channels.entry(info.name.clone()).or_default();

                    // ── Authority gating on sync ──────────────────────
                    // Merge founder: only adopt if we don't have one AND it's a valid DID
                    if ch.founder_did.is_none() {
                        if let Some(ref did) = info.founder_did {
                            if did.starts_with("did:") {
                                ch.founder_did = Some(did.clone());
                            } else {
                                tracing::warn!(
                                    channel = %info.name, peer = %peer_id,
                                    "Rejecting invalid founder DID in sync: {did}"
                                );
                            }
                        }
                    }

                    // DID ops: validate format before accepting.
                    // If --require-did-for-ops and no founder context, reject.
                    let require_did = state.config.require_did_for_ops;
                    for did in &info.did_ops {
                        if !did.starts_with("did:") {
                            tracing::warn!(
                                channel = %info.name, peer = %peer_id,
                                "Rejecting invalid DID op in sync: {did}"
                            );
                            continue;
                        }
                        let has_authority = info.founder_did.is_some()
                            || ch.founder_did.is_some()
                            || !ch.did_ops.is_empty();
                        if !has_authority && require_did {
                            tracing::warn!(
                                channel = %info.name, peer = %peer_id,
                                "Rejecting DID op {did} in sync: no authority (--require-did-for-ops)"
                            );
                            continue;
                        }
                        ch.did_ops.insert(did.clone());
                    }

                    // Presence: S2S-event-based (idempotent set-based merge)
                    if !info.nick_info.is_empty() {
                        for ni in &info.nick_info {
                            ch.remote_members.insert(ni.nick.clone(), RemoteMember {
                                origin: peer_id.clone(),
                                did: ni.did.clone(),
                                handle: None,
                                is_op: ni.is_op,
                            });
                        }
                    } else {
                        for nick in &info.nicks {
                            ch.remote_members.insert(nick.clone(), RemoteMember {
                                origin: peer_id.clone(),
                                did: None,
                                handle: None,
                                is_op: false,
                            });
                        }
                    }

                    if ch.topic.is_none() {
                        if let Some(ref topic) = info.topic {
                            ch.topic = Some(TopicInfo::new(
                                topic.clone(),
                                info.founder_did.as_deref().unwrap_or("unknown").to_string(),
                            ));
                        }
                    }

                    ch.topic_locked = info.topic_locked;
                    ch.invite_only = info.invite_only;
                    ch.no_ext_msg = info.no_ext_msg;
                    ch.moderated = info.moderated;
                    if info.key.is_some() {
                        ch.key = info.key.clone();
                    }

                    let dids = state.session_dids.lock().unwrap();
                    let members: Vec<String> = ch.members.iter().cloned().collect();

                    // First pass: grant ops to DID-backed users with authority
                    let mut did_ops_granted = false;
                    for session_id in &members {
                        if let Some(did) = dids.get(session_id) {
                            if ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did) {
                                ch.ops.insert(session_id.clone());
                                did_ops_granted = true;
                            }
                        }
                    }

                    // Second pass: revoke guest/non-authority auto-ops, but ONLY if
                    // someone with real authority now has ops (locally or remotely).
                    // Don't orphan the channel by revoking everyone's ops.
                    let has_authority_ops = did_ops_granted
                        || ch.remote_members.values().any(|rm| rm.is_op);
                    if has_authority_ops {
                        for session_id in &members {
                            let has_did_auth = dids.get(session_id).is_some_and(|did| {
                                ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did)
                            });
                            if !has_did_auth {
                                ch.ops.remove(session_id);
                            }
                        }
                    }

                    if !ch.members.is_empty() {
                        updated_channels.push(info.name.clone());
                    }

                    tracing::info!(
                        "  Channel {}: {} remote user(s), founder: {:?}, {} DID ops, topic: {:?}",
                        info.name, ch.remote_members.len(), ch.founder_did, ch.did_ops.len(),
                        ch.topic.as_ref().map(|t| &t.text),
                    );
                }
            }

            for channel in &updated_channels {
                send_names_update(state, channel);
                let topic_info = state.channels.lock().unwrap()
                    .get(channel)
                    .and_then(|ch| ch.topic.as_ref().map(|t| (t.text.clone(), t.set_by.clone())));
                if let Some((topic, _set_by)) = topic_info {
                    let line = format!(
                        ":{} 332 * {} :{}\r\n",
                        state.server_name, channel, topic,
                    );
                    let members: Vec<String> = state.channels.lock().unwrap()
                        .get(channel)
                        .map(|ch| ch.members.iter().cloned().collect())
                        .unwrap_or_default();
                    let conns = state.connections.lock().unwrap();
                    for session_id in &members {
                        if let Some(tx) = conns.get(session_id) {
                            let _ = tx.try_send(line.clone());
                        }
                    }
                }
            }
        }

        S2sMessage::Mode { channel, mode, arg, set_by, .. } => {
            {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(&channel) {
                    let adding = mode.starts_with('+');
                    let mode_char = mode.chars().last().unwrap_or(' ');
                    match mode_char {
                        't' => ch.topic_locked = adding,
                        'i' => ch.invite_only = adding,
                        'n' => ch.no_ext_msg = adding,
                        'm' => ch.moderated = adding,
                        'k' => {
                            if adding {
                                ch.key = arg.clone();
                            } else {
                                ch.key = None;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let mode_line = if let Some(ref a) = arg {
                format!(":{set_by}!remote@s2s MODE {channel} {mode} {a}\r\n")
            } else {
                format!(":{set_by}!remote@s2s MODE {channel} {mode}\r\n")
            };
            deliver_to_channel(state, &channel, &mode_line);
        }

        S2sMessage::Kick { nick, channel, by, reason, .. } => {
            // A remote op kicked a user — if the user is local, remove them
            // from the channel and notify them. If the user is a remote member
            // from yet another server, remove from remote_members.
            let kick_line = format!(":{by}!remote@s2s KICK {channel} {nick} :{reason}\r\n");

            let target_session = state.nick_to_session.lock().unwrap().get(&nick).cloned();
            if let Some(ref sid) = target_session {
                // Target is local — broadcast KICK to channel, remove member
                deliver_to_channel(state, &channel, &kick_line);
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(&channel) {
                    ch.members.remove(sid);
                    ch.ops.remove(sid);
                    ch.voiced.remove(sid);
                }
            } else {
                // Target is a remote member from another peer — remove and notify locals
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(&channel) {
                    if ch.remote_members.remove(&nick).is_some() {
                        drop(channels);
                        deliver_to_channel(state, &channel, &kick_line);
                    }
                }
            }
        }

        S2sMessage::NickChange { old, new, .. } => {
            let line = format!(":{old}!remote@s2s NICK :{new}\r\n");

            let mut channels = state.channels.lock().unwrap();
            let mut affected_sessions = std::collections::HashSet::new();
            for ch in channels.values_mut() {
                if let Some(rm) = ch.remote_members.remove(&old) {
                    ch.remote_members.insert(new.clone(), rm);
                    for s in &ch.members {
                        affected_sessions.insert(s.clone());
                    }
                }
            }
            drop(channels);

            let conns = state.connections.lock().unwrap();
            for session_id in &affected_sessions {
                if let Some(tx) = conns.get(session_id) {
                    let _ = tx.try_send(line.clone());
                }
            }
        }

        S2sMessage::CrdtSync { data, origin, .. } => {
            // SECURITY: Use authenticated_peer_id (from QUIC transport) to key
            // the Automerge sync state, NOT the `origin` field from the JSON
            // payload.  The payload origin is untrusted — a bug or malicious
            // peer could set it to anything.  The authenticated_peer_id comes
            // from conn.remote_id() which is cryptographically verified.
            if origin != authenticated_peer_id {
                tracing::warn!(
                    authenticated = %authenticated_peer_id,
                    claimed = %origin,
                    "CRDT sync origin mismatch — using authenticated peer ID"
                );
            }
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(&data) {
                Ok(bytes) => {
                    if let Err(e) = state.crdt_receive_sync(authenticated_peer_id, &bytes).await {
                        tracing::warn!(peer = %authenticated_peer_id, "CRDT sync receive error: {e}");
                    } else {
                        tracing::debug!(peer = %authenticated_peer_id, "CRDT sync message applied");
                        // Respond only to the sender — not all peers.
                        // Broadcasting to all peers on every receive creates
                        // amplification storms (A→B triggers A→all, they all
                        // respond, etc.).  The correct Automerge sync pattern
                        // is: receive from P → generate next message for P.
                        // Periodic full-mesh sync is handled by a timer.
                        state.crdt_sync_with_peer(authenticated_peer_id).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(peer = %authenticated_peer_id, "CRDT sync base64 decode error: {e}");
                }
            }
        }

        S2sMessage::PeerDisconnected { peer_id } => {
            // Clean up all remote_members whose origin matches this peer.
            // Without this, users from a disconnected server linger as ghosts
            // in channel rosters until they individually Part/Quit.
            let mut channels = state.channels.lock().unwrap();
            let mut cleaned = 0usize;
            let mut affected_channels = Vec::new();
            for (name, ch) in channels.iter_mut() {
                let before = ch.remote_members.len();
                ch.remote_members.retain(|_nick, rm| rm.origin != peer_id);
                let removed = before - ch.remote_members.len();
                if removed > 0 {
                    cleaned += removed;
                    affected_channels.push(name.clone());
                }
            }
            drop(channels);

            if cleaned > 0 {
                tracing::info!(
                    peer = %peer_id,
                    "Cleaned {cleaned} ghost remote member(s) from {} channel(s)",
                    affected_channels.len()
                );
                // Update NAMES for affected channels so local users see the change
                for channel in &affected_channels {
                    send_names_update(state, channel);
                }
            }
        }
    }
}

/// Periodic CRDT→local reconciliation.
///
/// Reads CRDT state (topics, founder, DID ops) and applies to local channel
/// state if divergent. This ensures the CRDT is the authoritative source of
/// truth — even when S2S events and CRDT diverge due to timing or partitions.
async fn reconcile_crdt_to_local(state: &Arc<SharedState>) {
    // Get list of channels
    let channel_names: Vec<String> = {
        state.channels.lock().unwrap().keys().cloned().collect()
    };

    let mut reconciled = 0u32;

    for channel_name in &channel_names {
        // Reconcile topic: if CRDT has a topic and it differs from local, adopt CRDT's
        if let Some((crdt_topic, crdt_setter)) = state.cluster_doc.channel_topic(channel_name).await {
            let needs_update = {
                let channels = state.channels.lock().unwrap();
                channels.get(channel_name)
                    .map(|ch| {
                        ch.topic.as_ref()
                            .map(|t| t.text != crdt_topic)
                            .unwrap_or(true) // no local topic, CRDT has one → adopt
                    })
                    .unwrap_or(false)
            };
            if needs_update {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel_name) {
                    ch.topic = Some(TopicInfo::new(crdt_topic, crdt_setter));
                    reconciled += 1;
                }
            }
        }

        // Reconcile founder
        if let Some(crdt_founder) = state.cluster_doc.founder(channel_name).await {
            let needs_update = {
                let channels = state.channels.lock().unwrap();
                channels.get(channel_name)
                    .map(|ch| ch.founder_did.as_deref() != Some(&crdt_founder))
                    .unwrap_or(false)
            };
            if needs_update {
                let mut channels = state.channels.lock().unwrap();
                if let Some(ch) = channels.get_mut(channel_name) {
                    tracing::info!(
                        channel = %channel_name,
                        "CRDT reconciliation: updating founder to {crdt_founder}"
                    );
                    ch.founder_did = Some(crdt_founder);
                    reconciled += 1;

                    // Re-evaluate local ops: grant to DID-backed users with authority.
                    // Only revoke guest auto-ops if an authority-backed user is now
                    // opped (locally or remotely) — don't orphan the channel.
                    let dids = state.session_dids.lock().unwrap();
                    let members: Vec<String> = ch.members.iter().cloned().collect();
                    let mut did_ops_granted = false;
                    for session_id in &members {
                        if let Some(did) = dids.get(session_id) {
                            if ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did) {
                                ch.ops.insert(session_id.clone());
                                did_ops_granted = true;
                            }
                        }
                    }
                    let has_authority_ops = did_ops_granted
                        || ch.remote_members.values().any(|rm| rm.is_op);
                    if has_authority_ops {
                        for session_id in &members {
                            let has_did_auth = dids.get(session_id).is_some_and(|did| {
                                ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did)
                            });
                            if !has_did_auth {
                                ch.ops.remove(session_id);
                            }
                        }
                    }
                }
            }
        }

        // Reconcile DID ops: CRDT is additive authority
        let crdt_ops = state.cluster_doc.channel_did_ops(channel_name).await;
        if !crdt_ops.is_empty() {
            let mut channels = state.channels.lock().unwrap();
            if let Some(ch) = channels.get_mut(channel_name) {
                for did in &crdt_ops {
                    if ch.did_ops.insert(did.clone()) {
                        reconciled += 1;
                    }
                }
                // Re-evaluate local ops: grant to DID-backed users with authority.
                // Revoke guest/non-authority auto-ops only if someone with real
                // authority now has ops (don't orphan the channel).
                let dids = state.session_dids.lock().unwrap();
                let members: Vec<String> = ch.members.iter().cloned().collect();
                let mut did_ops_granted = false;
                for session_id in &members {
                    if let Some(did) = dids.get(session_id) {
                        if ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did) {
                            ch.ops.insert(session_id.clone());
                            did_ops_granted = true;
                        }
                    }
                }
                let has_authority_ops = did_ops_granted
                    || ch.remote_members.values().any(|rm| rm.is_op);
                if has_authority_ops {
                    for session_id in &members {
                        let has_did_auth = dids.get(session_id).is_some_and(|did| {
                            ch.founder_did.as_deref() == Some(did) || ch.did_ops.contains(did)
                        });
                        if !has_did_auth {
                            ch.ops.remove(session_id);
                        }
                    }
                }
            }
        }
    }

    if reconciled > 0 {
        tracing::info!("CRDT→local reconciliation: {reconciled} updates applied across {} channels", channel_names.len());
    }
}
