//! Application state for the TUI.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::editor::{LineEditor, Mode};

/// Maximum number of messages to keep per buffer.
const MAX_MESSAGES: usize = 1000;

/// A single line in a message buffer.
#[derive(Debug, Clone)]
pub struct BufferLine {
    pub timestamp: String,
    pub from: String,
    pub text: String,
    pub is_system: bool,
    /// If this message has an associated image, its URL (key into ImageCache).
    pub image_url: Option<String>,
}

/// State of a cached image.
pub enum ImageState {
    /// Image is being fetched.
    Loading,
    /// Image is ready to render.
    #[allow(dead_code)]
    Ready(image::DynamicImage),
    /// Fetch failed.
    #[allow(dead_code)]
    Failed(String),
}

/// Thread-safe cache of fetched images, keyed by URL.
pub type ImageCache = Arc<Mutex<HashMap<String, ImageState>>>;

/// How many terminal rows an image takes up in the message area.
#[cfg(feature = "inline-images")]
pub const IMAGE_ROWS: u16 = 10;

/// Maximum entries in the image cache (LRU eviction).
const MAX_IMAGE_CACHE: usize = 50;
/// Maximum image download size (10 MB).
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// A named message buffer (channel, PM, or status).
#[derive(Debug)]
pub struct Buffer {
    pub name: String,
    pub messages: VecDeque<BufferLine>,
    pub nicks: Vec<String>,
    /// Channel topic, if set.
    pub topic: Option<String>,
    /// Scroll offset from the bottom (0 = at bottom).
    pub scroll: u16,
    /// Unread message count since the buffer was last active.
    pub unread: usize,
    /// Whether any unread message mentions the user's nick.
    pub has_mention: bool,
    /// Scroll offset for nick list (0 = top).
    pub nick_scroll: usize,
}

/// In-progress BATCH buffer (e.g., CHATHISTORY).
#[derive(Debug, Clone)]
pub struct BatchBuffer {
    pub target: String,
    pub lines: Vec<(i64, BufferLine)>,
}

impl Buffer {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            messages: VecDeque::new(),
            nicks: Vec::new(),
            topic: None,
            scroll: 0,
            unread: 0,
            has_mention: false,
            nick_scroll: 0,
        }
    }

    pub fn push(&mut self, line: BufferLine) {
        self.messages.push_back(line);
        if self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        // Auto-scroll to bottom when new message arrives
        self.scroll = 0;
    }

    pub fn push_system(&mut self, text: &str) {
        self.push(BufferLine {
            timestamp: now_str(),
            from: String::new(),
            text: sanitize_text(text),
            is_system: true,
            image_url: None,
        });
    }
}

/// Top-level application state.
/// Holds PDS credentials for media upload.
#[derive(Clone)]
pub struct MediaUploader {
    pub did: String,
    pub pds_url: String,
    pub access_token: String,
    pub dpop_key: Option<freeq_sdk::oauth::DpopKey>,
    pub dpop_nonce: Option<String>,
}

/// Transport type for the current connection.
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    Tcp,
    Tls,
    WebSocket,
    Iroh,
}

impl Transport {
    pub fn label(&self) -> &'static str {
        match self {
            Transport::Tcp => "TCP",
            Transport::Tls => "TLS",
            Transport::WebSocket => "WS",
            Transport::Iroh => "IROH",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Transport::Tcp => "⚡",       // unencrypted, fast
            Transport::Tls => "🔒",       // encrypted
            Transport::WebSocket => "🌐", // web
            Transport::Iroh => "🕳️",      // hole-punching / p2p
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Transport::Tcp => "Plain TCP (unencrypted)",
            Transport::Tls => "TLS 1.3 (encrypted)",
            Transport::WebSocket => "WebSocket (browser-compatible)",
            Transport::Iroh => "Iroh QUIC (encrypted, NAT-traversing, P2P-capable)",
        }
    }
}

pub struct App {
    /// Per-channel E2EE keys, keyed by lowercase channel name.
    /// Derived from passphrase via HKDF-SHA256.
    pub channel_keys: HashMap<String, [u8; 32]>,
    /// Named buffers, keyed by lowercase name. "status" is always present.
    pub buffers: BTreeMap<String, Buffer>,
    /// In-progress batch buffers (chathistory), keyed by batch ID.
    pub batches: HashMap<String, BatchBuffer>,
    /// Currently active buffer key.
    pub active_buffer: String,
    /// Line editor (handles input, cursor, emacs/vi keybindings).
    pub editor: LineEditor,
    /// Connection state display.
    pub connection_state: String,
    /// Transport type for the current connection.
    pub transport: Transport,
    /// Server address we connected to.
    pub server_addr: String,
    /// Iroh endpoint ID (if connected via iroh).
    pub iroh_endpoint_id: Option<String>,
    /// Time of connection establishment.
    pub connected_at: Option<std::time::Instant>,
    /// Authenticated DID (if any).
    pub authenticated_did: Option<String>,
    /// Whether to show the /net stats popup.
    pub show_net_popup: bool,
    /// Our nick.
    pub nick: String,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Whether a reconnect attempt should be made.
    pub reconnect_pending: bool,
    /// When the next reconnect attempt should happen.
    pub reconnect_at: Option<std::time::Instant>,
    /// Current reconnect backoff delay.
    pub reconnect_delay: Duration,
    /// Input history (most recent last).
    pub history: Vec<String>,
    /// Current position in history (None = not browsing).
    pub history_pos: Option<usize>,
    /// Saved input line when browsing history.
    pub history_saved: String,
    /// Media uploader (present when authenticated with PDS session).
    pub media_uploader: Option<MediaUploader>,
    /// Cache of fetched images for inline rendering.
    pub image_cache: ImageCache,
    /// Image protocol picker (detects terminal capabilities).
    #[cfg(feature = "inline-images")]
    pub picker: Option<ratatui_image::picker::Picker>,
    /// Prepared image protocol states for rendering, keyed by URL.
    #[cfg(feature = "inline-images")]
    pub image_protos: HashMap<String, ratatui_image::protocol::StatefulProtocol>,
    /// Channel for background tasks to send results back to the main loop.
    pub bg_result_tx: tokio::sync::mpsc::Sender<BgResult>,
    /// Receiver end (held by main loop).
    pub bg_result_rx: Option<tokio::sync::mpsc::Receiver<BgResult>>,
    /// Show raw IRC lines in the status buffer (toggled by /debug).
    pub debug_raw: bool,
    /// P2P direct messaging handle (None if P2P not started).
    pub p2p_handle: Option<freeq_sdk::p2p::P2pHandle>,
    /// P2P event receiver (moved to main loop on first use).
    pub p2p_event_rx: Option<tokio::sync::mpsc::Receiver<freeq_sdk::p2p::P2pEvent>>,
    /// Pending URL from a server notice (waiting for user to press Enter to open).
    pub pending_url: Option<String>,
}

/// Results from background tasks that need to update the UI.
pub enum BgResult {
    /// Profile lines to display in a buffer, with optional avatar URL for the last line.
    ProfileLines(String, Vec<String>, Option<String>),
}

impl App {
    pub fn new(nick: &str, vi_mode: bool) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let mut buffers = BTreeMap::new();
        let mut status = Buffer::new("status");
        let mode_name = if vi_mode { "vi" } else { "emacs" };
        status.push_system(&format!("Welcome to freeq ({mode_name} mode). Type /help for commands."));
        buffers.insert("status".to_string(), status);

        let mode = if vi_mode { Mode::Vi } else { Mode::Emacs };

        Self {
            channel_keys: HashMap::new(),
            buffers,
            batches: HashMap::new(),
            active_buffer: "status".to_string(),
            editor: LineEditor::new(mode),
            connection_state: "connecting".to_string(),
            transport: Transport::Tcp,
            server_addr: String::new(),
            iroh_endpoint_id: None,
            connected_at: None,
            authenticated_did: None,
            show_net_popup: false,
            debug_raw: false,
            nick: nick.to_string(),
            should_quit: false,
            reconnect_pending: false,
            reconnect_at: None,
            reconnect_delay: Duration::from_secs(1),
            history: Vec::new(),
            history_pos: None,
            history_saved: String::new(),
            media_uploader: None,
            image_cache: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "inline-images")]
            picker: None,
            #[cfg(feature = "inline-images")]
            image_protos: HashMap::new(),
            bg_result_tx: tx,
            bg_result_rx: Some(rx),
            p2p_handle: None,
            p2p_event_rx: None,
            pending_url: None,
        }
    }

    /// Get or create a buffer.
    pub fn buffer_mut(&mut self, name: &str) -> &mut Buffer {
        let key = name.to_lowercase();
        self.buffers
            .entry(key)
            .or_insert_with(|| Buffer::new(name))
    }

    /// Push a system message to the status buffer.
    pub fn status_msg(&mut self, text: &str) {
        self.buffer_mut("status").push_system(text);
    }

    /// Push a chat message to the appropriate buffer.
    pub fn chat_msg(&mut self, target: &str, from: &str, text: &str) {
        // If it's a PM to us, use the sender's nick as the buffer
        let buffer_name = if !target.starts_with('#') && !target.starts_with('&') {
            if from == self.nick {
                target.to_string()
            } else {
                from.to_string()
            }
        } else {
            target.to_string()
        };

        let clean_text = sanitize_text(text);
        let clean_from = sanitize_text(from);
        let buf_key = buffer_name.to_lowercase();
        let is_active = buf_key == self.active_buffer;

        self.buffer_mut(&buffer_name).push(BufferLine {
            timestamp: now_str(),
            from: clean_from,
            text: clean_text.clone(),
            is_system: false,
            image_url: None,
        });

        // Track unread + mentions for inactive buffers
        if !is_active && from != self.nick {
            if let Some(buf) = self.buffers.get_mut(&buf_key) {
                buf.unread += 1;
                if clean_text.to_lowercase().contains(&self.nick.to_lowercase()) {
                    buf.has_mention = true;
                }
            }
        }
    }

    /// Start a BATCH (e.g., CHATHISTORY).
    pub fn start_batch(&mut self, id: &str, target: &str) {
        self.batches.insert(id.to_string(), BatchBuffer {
            target: target.to_string(),
            lines: Vec::new(),
        });
    }

    /// Add a line to a batch by ID.
    pub fn add_batch_line(&mut self, id: &str, timestamp_ms: i64, line: BufferLine) {
        if let Some(batch) = self.batches.get_mut(id) {
            batch.lines.push((timestamp_ms, line));
        }
    }

    /// Flush a batch into its target buffer (prepended as history).
    pub fn end_batch(&mut self, id: &str) {
        if let Some(mut batch) = self.batches.remove(id) {
            let buf = self.buffer_mut(&batch.target);
            batch.lines.sort_by(|a, b| a.0.cmp(&b.0));
            // Prepend in order (oldest first)
            for (_, line) in batch.lines.into_iter().rev() {
                buf.messages.push_front(line);
                if buf.messages.len() > MAX_MESSAGES {
                    buf.messages.pop_back();
                }
            }
        }
    }

    /// Switch to the next buffer.
    /// Remove a buffer (e.g. after being kicked from a channel).
    /// Switches to the previous buffer if the removed one was active.
    pub fn remove_buffer(&mut self, name: &str) {
        let key = name.to_lowercase();
        self.buffers.remove(&key);
        if self.active_buffer == key {
            // Switch to first available buffer, or "status"
            self.active_buffer = self.buffers.keys().next()
                .cloned()
                .unwrap_or_else(|| "status".to_string());
        }
    }

    pub fn next_buffer(&mut self) {
        let keys: Vec<String> = self.buffers.keys().cloned().collect();
        if let Some(pos) = keys.iter().position(|k| k == &self.active_buffer) {
            let next = (pos + 1) % keys.len();
            self.active_buffer = keys[next].clone();
            self.clear_active_unread();
        }
    }

    /// Switch to the previous buffer.
    pub fn prev_buffer(&mut self) {
        let keys: Vec<String> = self.buffers.keys().cloned().collect();
        if let Some(pos) = keys.iter().position(|k| k == &self.active_buffer) {
            let prev = if pos == 0 { keys.len() - 1 } else { pos - 1 };
            self.active_buffer = keys[prev].clone();
            self.clear_active_unread();
        }
    }

    /// Switch to a named buffer.
    pub fn switch_to(&mut self, name: &str) {
        let key = name.to_lowercase();
        if self.buffers.contains_key(&key) {
            self.active_buffer = key;
            self.clear_active_unread();
        }
    }

    /// Clear unread state for the currently active buffer.
    fn clear_active_unread(&mut self) {
        if let Some(buf) = self.buffers.get_mut(&self.active_buffer) {
            buf.unread = 0;
            buf.has_mention = false;
        }
    }

    /// Get the ordered list of buffer names for the tab bar.
    pub fn buffer_names(&self) -> Vec<String> {
        self.buffers.keys().cloned().collect()
    }

    /// Take and clear the input line, pushing it to history.
    pub fn input_take(&mut self) -> String {
        self.history_pos = None;
        let line = self.editor.take();
        if !line.is_empty() {
            self.history.push(line.clone());
        }
        line
    }

    /// Browse up in input history.
    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_pos {
            None => {
                self.history_saved = self.editor.text.clone();
                self.history_pos = Some(self.history.len() - 1);
            }
            Some(pos) if pos > 0 => {
                self.history_pos = Some(pos - 1);
            }
            _ => return,
        }
        let pos = self.history_pos.unwrap();
        self.editor.set(self.history[pos].clone());
    }

    /// Browse down in input history.
    pub fn history_down(&mut self) {
        if let Some(pos) = self.history_pos {
            if pos + 1 < self.history.len() {
                self.history_pos = Some(pos + 1);
                self.editor.set(self.history[pos + 1].clone());
            } else {
                self.history_pos = None;
                let saved = std::mem::take(&mut self.history_saved);
                self.editor.set(saved);
            }
        }
    }
}

fn now_str() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Strip terminal control characters (ESC sequences, C0/C1 controls) from text.
/// Prevents malicious users from injecting escape sequences that could
/// mess up the terminal display.
pub fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            // Allow normal printable chars + newline/tab
            c == '\n' || c == '\t' || (c >= ' ' && c != '\x7f')
        })
        .collect()
}

/// Evict oldest entries from image cache if over capacity.
pub fn evict_image_cache(cache: &ImageCache) {
    let mut guard = cache.lock().unwrap();
    if guard.len() > MAX_IMAGE_CACHE {
        // Simple eviction: remove Failed entries first, then arbitrary
        let failed_keys: Vec<String> = guard.iter()
            .filter(|(_, v)| matches!(v, ImageState::Failed(_)))
            .map(|(k, _)| k.clone())
            .collect();
        for k in failed_keys {
            guard.remove(&k);
            if guard.len() <= MAX_IMAGE_CACHE { return; }
        }
        // Still over? Remove arbitrary entries until within limit
        let keys: Vec<String> = guard.keys().cloned().collect();
        for k in keys {
            if guard.len() <= MAX_IMAGE_CACHE { break; }
            guard.remove(&k);
        }
    }
}
