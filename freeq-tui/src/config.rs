//! Persistent configuration for freeq-tui.
//!
//! Config file lives at `~/.config/freeq/tui.toml`.
//! Session state (last server, channels) at `~/.config/freeq/session.toml`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default IRC server.
pub const DEFAULT_SERVER: &str = "irc.freeq.at:6697";
/// Default channel to join on first run.
pub const DEFAULT_CHANNEL: &str = "#freeq";

/// User configuration (persisted in tui.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Server address (host:port). Default: irc.freeq.at:6697
    pub server: Option<String>,
    /// IRC nickname.
    pub nick: Option<String>,
    /// Bluesky handle for OAuth login.
    pub handle: Option<String>,
    /// Use TLS (auto-detected from :6697, but can force).
    pub tls: Option<bool>,
    /// Skip TLS certificate verification.
    pub tls_insecure: Option<bool>,
    /// Use vi keybindings.
    pub vi: Option<bool>,
    /// Channels to auto-join (overrides session state).
    pub channels: Option<Vec<String>>,
    /// Iroh endpoint address (P2P transport).
    pub iroh_addr: Option<String>,
}

/// Session state saved on quit, restored on start.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    /// Last server connected to.
    pub server: Option<String>,
    /// Last nick used.
    pub nick: Option<String>,
    /// Last handle used for auth.
    pub handle: Option<String>,
    /// Channels that were open on quit.
    pub channels: Vec<String>,
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("freeq")
}

fn config_path() -> PathBuf {
    config_dir().join("tui.toml")
}

fn session_path() -> PathBuf {
    config_dir().join("session.toml")
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(s) => match toml::from_str(&s) {
                    Ok(c) => return c,
                    Err(e) => eprintln!("Warning: bad config file {}: {e}", path.display()),
                },
                Err(e) => eprintln!("Warning: can't read {}: {e}", path.display()),
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    eprintln!("Warning: can't save config: {e}");
                }
            }
            Err(e) => eprintln!("Warning: can't serialize config: {e}"),
        }
    }
}

impl Session {
    pub fn load() -> Self {
        let path = session_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(s) => match toml::from_str(&s) {
                    Ok(c) => return c,
                    Err(e) => eprintln!("Warning: bad session file {}: {e}", path.display()),
                },
                Err(e) => eprintln!("Warning: can't read {}: {e}", path.display()),
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = session_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    eprintln!("Warning: can't save session: {e}");
                }
            }
            Err(e) => eprintln!("Warning: can't serialize session: {e}"),
        }
    }
}

/// Resolve the effective values by merging CLI args > config file > session state > defaults.
pub struct Resolved {
    pub server: String,
    pub nick: String,
    pub handle: Option<String>,
    pub tls: bool,
    pub tls_insecure: bool,
    pub vi: bool,
    pub channels: Vec<String>,
    pub iroh_addr: Option<String>,
}

impl Resolved {
    /// Merge: CLI overrides > config file > session state > defaults.
    pub fn merge(cli: &super::Cli, config: &Config, session: &Session) -> Self {
        let server = cli.server.clone()
            .or_else(|| config.server.clone())
            .or_else(|| session.server.clone())
            .unwrap_or_else(|| DEFAULT_SERVER.to_string());

        let nick = cli.nick.clone()
            .or_else(|| config.nick.clone())
            .or_else(|| session.nick.clone())
            .unwrap_or_else(|| {
                // Derive from handle or system username
                cli.handle.as_ref()
                    .or(config.handle.as_ref())
                    .or(session.handle.as_ref())
                    .map(|h| h.split('.').next().unwrap_or("guest").to_string())
                    .unwrap_or_else(|| whoami::fallible::username().unwrap_or_else(|_| "guest".to_string()))
            });

        let handle = cli.handle.clone()
            .or_else(|| config.handle.clone())
            .or_else(|| session.handle.clone());

        let tls_explicit = cli.tls || config.tls.unwrap_or(false);
        let tls = tls_explicit || server.ends_with(":6697");

        let tls_insecure = cli.tls_insecure || config.tls_insecure.unwrap_or(false);
        let vi = cli.vi || config.vi.unwrap_or(false);

        // Channels: CLI > config > session > default
        let channels = if let Some(ref ch) = cli.channels {
            ch.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
        } else if let Some(ref ch) = config.channels {
            ch.clone()
        } else if !session.channels.is_empty() {
            session.channels.clone()
        } else {
            vec![DEFAULT_CHANNEL.to_string()]
        };

        let iroh_addr = cli.iroh_addr.clone()
            .or_else(|| config.iroh_addr.clone());

        Self { server, nick, handle, tls, tls_insecure, vi, channels, iroh_addr }
    }
}
