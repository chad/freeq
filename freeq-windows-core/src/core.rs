//! AppCore — per-client state managed by the global handle table.

use std::sync::atomic::AtomicBool;

use parking_lot::Mutex;

use crate::bridge::callback::CallbackSink;

/// Per-client state. One instance per `freeq_win_create_client` call.
///
/// Stored in the global `HANDLES` table (see `bridge::abi`) behind an `Arc`.
pub struct AppCore {
    /// Unique handle ID (key in the HANDLES table).
    pub id: u64,
    /// Active SDK client handle (set after connect, cleared on disconnect).
    pub sdk_handle: Mutex<Option<freeq_sdk::client::ClientHandle>>,
    /// Whether the client is currently connected.
    pub connected: AtomicBool,
    /// Current nick (updated on Registered events).
    pub nick: Mutex<String>,
    /// Registered event callback (set via subscribe_events).
    pub callback: Mutex<Option<CallbackSink>>,
    /// Server address (host:port).
    pub server_addr: String,
    /// Initial nick from config.
    pub initial_nick: String,
    /// Whether to use TLS.
    pub tls: bool,
    /// Web token for SASL authentication (consumed on connect).
    pub web_token: Mutex<Option<String>>,
    /// Channels the client has joined (for reconnect re-join).
    pub channels: Mutex<Vec<String>>,
}
