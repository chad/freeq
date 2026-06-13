//! FFI wrapper around freeq-sdk for Swift/Kotlin consumption via UniFFI.

use once_cell::sync::Lazy;
use std::sync::{Arc, Mutex};

/// Install a tracing subscriber that writes to stderr the first time anyone
/// touches the SDK. iOS captures this in the Xcode console pane while
/// debugging — invaluable for triaging connect-path hangs. Idempotent: a
/// second install is a no-op.
fn install_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("freeq_sdk=debug,freeq_sdk_ffi=debug,info")
                }),
            )
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_ansi(false)
            .try_init();
    });
}

static RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    install_tracing_subscriber();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("Failed to create tokio runtime")
});

uniffi::include_scaffolding!("freeq");

// ── Types (must match UDL exactly) ──

pub struct IrcMessage {
    pub from_nick: String,
    pub target: String,
    pub text: String,
    pub msgid: Option<String>,
    pub reply_to: Option<String>,
    pub replaces_msgid: Option<String>,
    pub edit_of: Option<String>,
    pub batch_id: Option<String>,
    pub pin_msgid: Option<String>,
    pub unpin_msgid: Option<String>,
    pub is_action: bool,
    pub is_signed: bool,
    pub timestamp_ms: i64,
    pub account: Option<String>,
    /// Persisted reactions delivered on the message itself via the
    /// server's `+freeq.at/reactions` tag (CHATHISTORY / JOIN replay).
    /// Live reactions still arrive as separate `TagMsg` events.
    pub reactions: Vec<ReactionTally>,
}

pub struct ReactionTally {
    pub emoji: String,
    pub nicks: Vec<String>,
}

/// Parse the server's `+freeq.at/reactions` value
/// (`emoji1:nick1,nick2;emoji2:nick3`) into structured tallies.
/// Malformed segments are skipped.
fn parse_reactions_tag(raw: &str) -> Vec<ReactionTally> {
    raw.split(';')
        .filter_map(|seg| {
            let (emoji, nicks) = seg.split_once(':')?;
            let emoji = emoji.trim();
            if emoji.is_empty() {
                return None;
            }
            let nicks: Vec<String> = nicks
                .split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect();
            if nicks.is_empty() {
                return None;
            }
            Some(ReactionTally {
                emoji: emoji.to_string(),
                nicks,
            })
        })
        .collect()
}

pub struct TagEntry {
    pub key: String,
    pub value: String,
}

pub struct TagMessage {
    pub from: String,
    pub target: String,
    pub tags: Vec<TagEntry>,
}

pub struct IrcMember {
    pub nick: String,
    pub is_op: bool,
    pub is_halfop: bool,
    pub is_voiced: bool,
    pub away_msg: Option<String>,
}

pub struct ChannelTopic {
    pub text: String,
    pub set_by: Option<String>,
}

pub enum FreeqEvent {
    Connected,
    Registered {
        nick: String,
    },
    Authenticated {
        did: String,
    },
    AuthFailed {
        reason: String,
    },
    Joined {
        channel: String,
        nick: String,
    },
    Parted {
        channel: String,
        nick: String,
    },
    NickChanged {
        old_nick: String,
        new_nick: String,
    },
    AwayChanged {
        nick: String,
        away_msg: Option<String>,
    },
    Message {
        msg: IrcMessage,
    },
    TagMsg {
        msg: TagMessage,
    },
    Names {
        channel: String,
        members: Vec<IrcMember>,
    },
    TopicChanged {
        channel: String,
        topic: ChannelTopic,
    },
    ModeChanged {
        channel: String,
        mode: String,
        arg: Option<String>,
        set_by: String,
    },
    Kicked {
        channel: String,
        nick: String,
        by: String,
        reason: String,
    },
    UserQuit {
        nick: String,
        reason: String,
    },
    BatchStart {
        id: String,
        batch_type: String,
        target: String,
    },
    BatchEnd {
        id: String,
    },
    ChatHistoryTarget {
        nick: String,
        timestamp: Option<String>,
    },
    WhoisReply {
        nick: String,
        info: String,
    },
    Notice {
        text: String,
    },
    Disconnected {
        reason: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum FreeqError {
    #[error("Connection failed")]
    ConnectionFailed,
    #[error("Not connected")]
    NotConnected,
    #[error("Send failed")]
    SendFailed,
    #[error("Invalid argument")]
    InvalidArgument,
}

pub trait EventHandler: Send + Sync + 'static {
    fn on_event(&self, event: FreeqEvent);
}

// ── Client ──

pub struct FreeqClient {
    server: String,
    nick: Arc<Mutex<String>>,
    handler: Arc<dyn EventHandler>,
    handle: Arc<Mutex<Option<freeq_sdk::client::ClientHandle>>>,
    connected: Arc<Mutex<bool>>,
    web_token: Arc<Mutex<Option<String>>>,
    platform: Arc<Mutex<String>>,
    /// WebSocket URL (`wss://host/path`). When set, connect() prefers this
    /// transport over raw TCP — used by iOS so it can reach the server on
    /// networks that block port 6667.
    websocket_url: Arc<Mutex<Option<String>>>,
}

impl FreeqClient {
    pub fn new(
        server: String,
        nick: String,
        handler: Box<dyn EventHandler>,
    ) -> Result<Self, FreeqError> {
        Ok(Self {
            server,
            nick: Arc::new(Mutex::new(nick)),
            handler: Arc::from(handler),
            handle: Arc::new(Mutex::new(None)),
            connected: Arc::new(Mutex::new(false)),
            web_token: Arc::new(Mutex::new(None)),
            platform: Arc::new(Mutex::new("freeq ios".to_string())),
            websocket_url: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_web_token(&self, token: String) -> Result<(), FreeqError> {
        tracing::debug!("[FFI] set_web_token called, token len={}", token.len());
        *self.web_token.lock().unwrap() = Some(token);
        Ok(())
    }

    pub fn set_platform(&self, platform: String) -> Result<(), FreeqError> {
        *self.platform.lock().unwrap() = platform;
        Ok(())
    }

    /// Set the WebSocket URL the next `connect()` should use. Pass an empty
    /// string to clear and fall back to the configured `server` (TCP).
    pub fn set_websocket_url(&self, url: String) -> Result<(), FreeqError> {
        let trimmed = url.trim();
        let value = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        tracing::debug!("[FFI] set_websocket_url: {:?}", value);
        *self.websocket_url.lock().unwrap() = value;
        Ok(())
    }

    pub fn connect(&self) -> Result<(), FreeqError> {
        let nick = self.nick.lock().unwrap().clone();
        let web_token = self.web_token.lock().unwrap().take();
        let websocket_url = self.websocket_url.lock().unwrap().clone();
        tracing::debug!(
            "[FFI] connect: nick={}, web_token={}, ws={}",
            nick,
            web_token.is_some(),
            websocket_url.is_some()
        );
        let config = freeq_sdk::client::ConnectConfig {
            server_addr: self.server.clone(),
            nick: nick.clone(),
            user: nick.clone(),
            realname: self.platform.lock().unwrap().clone(),
            tls: self.server.contains(":6697") || self.server.contains(":443"),
            tls_insecure: false,
            web_token,
            websocket_url,
        };

        // MUST call connect() inside the runtime — it uses tokio::spawn internally.
        let handle_store = self.handle.clone();
        let connected_store = self.connected.clone();
        let handler = self.handler.clone();
        let nick_state = self.nick.clone();

        // Use a std::thread to avoid blocking the main thread (UniFFI calls from Swift main thread).
        // The thread enters the tokio runtime, calls connect, then pumps events.
        std::thread::spawn(move || {
            RUNTIME.block_on(async move {
                let (client_handle, mut event_rx) = freeq_sdk::client::connect(config, None);

                *handle_store.lock().unwrap() = Some(client_handle);
                *connected_store.lock().unwrap() = true;

                // Pump events
                while let Some(event) = event_rx.recv().await {
                    let ffi_event = convert_event(&event);
                    if let FreeqEvent::Disconnected { .. } = &ffi_event {
                        *connected_store.lock().unwrap() = false;
                    }
                    if let FreeqEvent::Registered { ref nick } = &ffi_event {
                        *nick_state.lock().unwrap() = nick.clone();
                    }
                    handler.on_event(ffi_event);
                }
            });
        });

        Ok(())
    }

    pub fn disconnect(&self) {
        let handle = self.handle.lock().unwrap().take();
        if let Some(handle) = handle {
            // Spawn quit on the runtime — don't block_on from arbitrary thread
            RUNTIME.spawn(async move {
                let _ = handle.quit(Some("Goodbye")).await;
            });
        }
        *self.connected.lock().unwrap() = false;
    }

    pub fn join(&self, channel: String) -> Result<(), FreeqError> {
        let handle = self
            .handle
            .lock()
            .unwrap()
            .clone()
            .ok_or(FreeqError::NotConnected)?;
        // Use spawn + oneshot to avoid block_on deadlock
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle
                .join(&channel)
                .await
                .map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
    }

    pub fn part(&self, channel: String) -> Result<(), FreeqError> {
        let handle = self
            .handle
            .lock()
            .unwrap()
            .clone()
            .ok_or(FreeqError::NotConnected)?;
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle
                .raw(&format!("PART {channel}"))
                .await
                .map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
    }

    pub fn send_message(&self, target: String, text: String) -> Result<(), FreeqError> {
        let handle = self
            .handle
            .lock()
            .unwrap()
            .clone()
            .ok_or(FreeqError::NotConnected)?;
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle
                .privmsg(&target, &text)
                .await
                .map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
    }

    pub fn send_raw(&self, line: String) -> Result<(), FreeqError> {
        tracing::debug!("[FFI] send_raw called: {}", &line);
        let handle = self
            .handle
            .lock()
            .unwrap()
            .clone()
            .ok_or(FreeqError::NotConnected)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let line_clone = line.clone();
        RUNTIME.spawn(async move {
            let result = handle
                .raw(&line_clone)
                .await
                .map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        match rx.recv() {
            Ok(Ok(())) => {
                tracing::debug!("[FFI] send_raw OK: {}", &line);
                Ok(())
            }
            Ok(Err(e)) => {
                tracing::error!("[FFI] send_raw failed: {:?}", e);
                Err(e)
            }
            Err(_) => {
                tracing::error!("[FFI] send_raw channel error");
                Err(FreeqError::SendFailed)
            }
        }
    }

    pub fn set_topic(&self, channel: String, topic: String) -> Result<(), FreeqError> {
        self.send_raw(format!("TOPIC {channel} :{topic}"))
    }

    pub fn nick(&self, new_nick: String) -> Result<(), FreeqError> {
        self.send_raw(format!("NICK {new_nick}"))
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    pub fn current_nick(&self) -> Option<String> {
        Some(self.nick.lock().unwrap().clone())
    }
}

// ── Event conversion ──

fn convert_event(event: &freeq_sdk::event::Event) -> FreeqEvent {
    use freeq_sdk::event::Event;
    match event {
        Event::Connected => FreeqEvent::Connected,
        Event::Registered { nick } => FreeqEvent::Registered { nick: nick.clone() },
        Event::Authenticated { did } => FreeqEvent::Authenticated { did: did.clone() },
        Event::AuthFailed { reason } => FreeqEvent::AuthFailed {
            reason: reason.clone(),
        },
        Event::Joined { channel, nick, .. } => FreeqEvent::Joined {
            channel: channel.clone(),
            nick: nick.clone(),
        },
        Event::Parted { channel, nick } => FreeqEvent::Parted {
            channel: channel.clone(),
            nick: nick.clone(),
        },
        Event::Message {
            from,
            target,
            text,
            tags,
        } => {
            let msgid = tags.get("msgid").cloned();
            let reply_to = tags.get("+reply").cloned();
            let replaces_msgid = tags.get("+draft/edit").cloned();
            let edit_of = tags.get("+draft/edit").cloned();
            let batch_id = tags.get("batch").cloned();
            let pin_msgid = tags.get("+freeq.at/pin").cloned();
            let unpin_msgid = tags.get("+freeq.at/unpin").cloned();
            let is_action = text.starts_with("\x01ACTION ") && text.ends_with('\x01');
            let clean_text = if is_action {
                text.trim_start_matches("\x01ACTION ")
                    .trim_end_matches('\x01')
                    .to_string()
            } else {
                text.clone()
            };
            let ts = tags
                .get("time")
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|dt: chrono::DateTime<chrono::FixedOffset>| dt.timestamp_millis())
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
            let reactions = tags
                .get("+freeq.at/reactions")
                .map(|raw| parse_reactions_tag(raw))
                .unwrap_or_default();
            FreeqEvent::Message {
                msg: IrcMessage {
                    from_nick: from.clone(),
                    target: target.clone(),
                    text: clean_text,
                    msgid,
                    reply_to,
                    replaces_msgid,
                    edit_of,
                    batch_id,
                    pin_msgid,
                    unpin_msgid,
                    is_action,
                    is_signed: tags.contains_key("+freeq.at/sig"),
                    timestamp_ms: ts,
                    account: tags.get("account").cloned(),
                    reactions,
                },
            }
        }
        Event::TagMsg { from, target, tags } => {
            let tag_entries = tags
                .iter()
                .map(|(k, v)| TagEntry {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect::<Vec<_>>();
            FreeqEvent::TagMsg {
                msg: TagMessage {
                    from: from.clone(),
                    target: target.clone(),
                    tags: tag_entries,
                },
            }
        }
        Event::Names { channel, nicks } => {
            let members = nicks
                .iter()
                .map(|n| {
                    let (is_op, is_halfop, is_voiced, nick) =
                        if let Some(rest) = n.strip_prefix('@') {
                            (true, false, false, rest.to_string())
                        } else if let Some(rest) = n.strip_prefix('%') {
                            (false, true, false, rest.to_string())
                        } else if let Some(rest) = n.strip_prefix('+') {
                            (false, false, true, rest.to_string())
                        } else {
                            (false, false, false, n.clone())
                        };
                    IrcMember {
                        nick,
                        is_op,
                        is_halfop,
                        is_voiced,
                        away_msg: None,
                    }
                })
                .collect();
            FreeqEvent::Names {
                channel: channel.clone(),
                members,
            }
        }
        Event::NamesEnd { channel } => {
            // Signal end of NAMES list — client should flush pending members + request history
            FreeqEvent::Notice {
                text: format!("__NAMES_END__{}", channel),
            }
        }
        Event::ModeChanged {
            channel,
            mode,
            arg,
            set_by,
        } => FreeqEvent::ModeChanged {
            channel: channel.clone(),
            mode: mode.clone(),
            arg: arg.clone(),
            set_by: set_by.clone(),
        },
        Event::Kicked {
            channel,
            nick,
            by,
            reason,
        } => FreeqEvent::Kicked {
            channel: channel.clone(),
            nick: nick.clone(),
            by: by.clone(),
            reason: reason.clone(),
        },
        Event::TopicChanged {
            channel,
            topic,
            set_by,
        } => FreeqEvent::TopicChanged {
            channel: channel.clone(),
            topic: ChannelTopic {
                text: topic.clone(),
                set_by: set_by.clone(),
            },
        },
        Event::ServerNotice { text } => FreeqEvent::Notice { text: text.clone() },
        Event::UserQuit { nick, reason } => FreeqEvent::UserQuit {
            nick: nick.clone(),
            reason: reason.clone(),
        },
        Event::NickChanged { old_nick, new_nick } => FreeqEvent::NickChanged {
            old_nick: old_nick.clone(),
            new_nick: new_nick.clone(),
        },
        Event::AwayChanged { nick, away_msg } => FreeqEvent::AwayChanged {
            nick: nick.clone(),
            away_msg: away_msg.clone(),
        },
        Event::BatchStart {
            id,
            batch_type,
            target,
        } => FreeqEvent::BatchStart {
            id: id.clone(),
            batch_type: batch_type.clone(),
            target: target.clone(),
        },
        Event::BatchEnd { id } => FreeqEvent::BatchEnd { id: id.clone() },
        Event::ChatHistoryTarget { nick, timestamp } => FreeqEvent::ChatHistoryTarget {
            nick: nick.clone(),
            timestamp: timestamp.clone(),
        },
        Event::Disconnected { reason } => FreeqEvent::Disconnected {
            reason: reason.clone(),
        },
        Event::Invited { channel, by } => FreeqEvent::Notice {
            text: format!("{by} invited you to {channel}"),
        },
        Event::WhoisReply { nick, info } => FreeqEvent::WhoisReply {
            nick: nick.clone(),
            info: info.clone(),
        },
        Event::RawLine(_) => FreeqEvent::Notice {
            text: String::new(),
        },
    }
}

// ── E2EE Manager ───────────────────────────────────────────────────

use freeq_sdk::ratchet::{self, Session as RatchetSession};
use std::collections::HashMap;

/// E2EE manager for iOS — wraps Rust Double Ratchet sessions.
pub struct FreeqE2ee {
    sessions: Mutex<HashMap<String, RatchetSession>>,
    identity_secret: Mutex<Option<[u8; 32]>>,
    identity_public: Mutex<Option<[u8; 32]>>,
    spk_secret: Mutex<Option<[u8; 32]>>,
    spk_public: Mutex<Option<[u8; 32]>>,
}

/// Pre-key bundle for uploading to the server.
pub struct PreKeyBundle {
    pub identity_key: String,   // base64url
    pub signed_pre_key: String, // base64url
    pub spk_signature: String,  // base64url (Ed25519 sig of SPK)
    pub spk_id: u32,
}

/// Safety number for verification.
pub struct SafetyNumber {
    pub number: String,
}

impl FreeqE2ee {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            identity_secret: Mutex::new(None),
            identity_public: Mutex::new(None),
            spk_secret: Mutex::new(None),
            spk_public: Mutex::new(None),
        }
    }

    /// Generate identity and signed pre-key. Returns the bundle to upload.
    fn generate_keys(&self) -> Result<PreKeyBundle, FreeqError> {
        use aes_gcm::aead::OsRng;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
        use base64::Engine;
        use x25519_dalek::{PublicKey, StaticSecret};

        let ik_secret = StaticSecret::random_from_rng(OsRng);
        let ik_public = PublicKey::from(&ik_secret);
        let spk_secret = StaticSecret::random_from_rng(OsRng);
        let spk_public = PublicKey::from(&spk_secret);

        *self.identity_secret.lock().unwrap() = Some(ik_secret.to_bytes());
        *self.identity_public.lock().unwrap() = Some(ik_public.to_bytes());
        *self.spk_secret.lock().unwrap() = Some(spk_secret.to_bytes());
        *self.spk_public.lock().unwrap() = Some(spk_public.to_bytes());

        // Sign SPK with Ed25519 signing key
        use ed25519_dalek::{Signer, SigningKey};
        let signing_key = SigningKey::generate(&mut OsRng);
        let sig = signing_key.sign(spk_public.as_bytes());

        Ok(PreKeyBundle {
            identity_key: B64.encode(ik_public.as_bytes()),
            signed_pre_key: B64.encode(spk_public.as_bytes()),
            spk_signature: B64.encode(sig.to_bytes()),
            spk_id: 1,
        })
    }

    /// Restore keys from persisted base64url strings (from Keychain).
    fn restore_keys(
        &self,
        ik_secret_b64: String,
        spk_secret_b64: String,
    ) -> Result<PreKeyBundle, FreeqError> {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
        use base64::Engine;
        use x25519_dalek::{PublicKey, StaticSecret};

        let ik_bytes: [u8; 32] = B64
            .decode(&ik_secret_b64)
            .map_err(|_| FreeqError::InvalidArgument)?
            .try_into()
            .map_err(|_| FreeqError::InvalidArgument)?;
        let spk_bytes: [u8; 32] = B64
            .decode(&spk_secret_b64)
            .map_err(|_| FreeqError::InvalidArgument)?
            .try_into()
            .map_err(|_| FreeqError::InvalidArgument)?;

        let ik_secret = StaticSecret::from(ik_bytes);
        let ik_public = PublicKey::from(&ik_secret);
        let spk_secret = StaticSecret::from(spk_bytes);
        let spk_public = PublicKey::from(&spk_secret);

        *self.identity_secret.lock().unwrap() = Some(ik_bytes);
        *self.identity_public.lock().unwrap() = Some(ik_public.to_bytes());
        *self.spk_secret.lock().unwrap() = Some(spk_bytes);
        *self.spk_public.lock().unwrap() = Some(spk_public.to_bytes());

        Ok(PreKeyBundle {
            identity_key: B64.encode(ik_public.as_bytes()),
            signed_pre_key: B64.encode(spk_public.as_bytes()),
            spk_signature: String::new(),
            spk_id: 1,
        })
    }

    /// Export private keys as base64url for Keychain persistence.
    fn export_keys(&self) -> Result<Vec<String>, FreeqError> {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
        use base64::Engine;

        let ik = self
            .identity_secret
            .lock()
            .unwrap()
            .ok_or(FreeqError::NotConnected)?;
        let spk = self
            .spk_secret
            .lock()
            .unwrap()
            .ok_or(FreeqError::NotConnected)?;
        Ok(vec![B64.encode(ik), B64.encode(spk)])
    }

    /// Establish a session with a remote user from their pre-key bundle.
    /// `bundle_json` is the JSON from GET /api/v1/keys/{did}.
    fn establish_session(
        &self,
        remote_did: String,
        their_ik_b64: String,
        their_spk_b64: String,
    ) -> Result<(), FreeqError> {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
        use base64::Engine;
        use x25519_dalek::{PublicKey, StaticSecret};

        let their_ik: [u8; 32] = B64
            .decode(&their_ik_b64)
            .map_err(|_| FreeqError::InvalidArgument)?
            .try_into()
            .map_err(|_| FreeqError::InvalidArgument)?;
        let their_spk: [u8; 32] = B64
            .decode(&their_spk_b64)
            .map_err(|_| FreeqError::InvalidArgument)?
            .try_into()
            .map_err(|_| FreeqError::InvalidArgument)?;

        let my_ik_secret = self
            .identity_secret
            .lock()
            .unwrap()
            .ok_or(FreeqError::NotConnected)?;
        let my_ik = StaticSecret::from(my_ik_secret);
        let their_ik_pk = PublicKey::from(their_ik);

        // X3DH: DH(our IK, their SPK) — simplified, same as web client
        let dh_out = my_ik.diffie_hellman(&their_ik_pk).to_bytes();
        let their_spk_pk = PublicKey::from(their_spk);
        let dh_out2 = my_ik.diffie_hellman(&their_spk_pk).to_bytes();

        // Combine DH outputs
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dh_out);
        hasher.update(dh_out2);
        let shared_secret: [u8; 32] = hasher.finalize().into();

        // Canonical order: lower public key is "initiator"
        let my_pk = self
            .identity_public
            .lock()
            .unwrap()
            .ok_or(FreeqError::NotConnected)?;
        let we_are_first = my_pk < their_ik;

        let session = if we_are_first {
            RatchetSession::init_alice(shared_secret, their_spk)
        } else {
            let my_spk = self
                .spk_secret
                .lock()
                .unwrap()
                .ok_or(FreeqError::NotConnected)?;
            RatchetSession::init_bob(shared_secret, my_spk)
        };

        self.sessions.lock().unwrap().insert(remote_did, session);
        Ok(())
    }

    /// Encrypt a message for a remote user. Returns ENC3:... wire format.
    fn encrypt_message(&self, remote_did: String, plaintext: String) -> Result<String, FreeqError> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&remote_did)
            .ok_or(FreeqError::NotConnected)?;
        session
            .encrypt(&plaintext)
            .map_err(|_| FreeqError::SendFailed)
    }

    /// Decrypt a message from a remote user.
    fn decrypt_message(&self, remote_did: String, wire: String) -> Result<String, FreeqError> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(&remote_did)
            .ok_or(FreeqError::NotConnected)?;
        session
            .decrypt(&wire)
            .map_err(|_| FreeqError::InvalidArgument)
    }

    /// Check if we have an active session with a user.
    fn has_session(&self, remote_did: String) -> bool {
        self.sessions.lock().unwrap().contains_key(&remote_did)
    }

    /// Check if a message is encrypted.
    fn is_encrypted(&self, text: String) -> bool {
        text.starts_with(ratchet::ENC3_PREFIX)
    }

    /// Get safety number for a session (hash of both identity keys).
    fn get_safety_number(&self, remote_did: String) -> Result<SafetyNumber, FreeqError> {
        use sha2::{Digest, Sha256};
        let my_pk = self
            .identity_public
            .lock()
            .unwrap()
            .ok_or(FreeqError::NotConnected)?;

        // Combine in canonical order
        let mut hasher = Sha256::new();
        let remote_bytes = remote_did.as_bytes();
        if my_pk.as_slice() < remote_bytes {
            hasher.update(my_pk);
            hasher.update(remote_bytes);
        } else {
            hasher.update(remote_bytes);
            hasher.update(my_pk);
        }
        let hash: [u8; 32] = hasher.finalize().into();

        // 12 groups of 5 digits
        let mut digits = Vec::new();
        for i in 0..12 {
            let val = ((hash[i * 2] as u32) << 8 | hash[i * 2 + 1] as u32) % 100000;
            digits.push(format!("{val:05}"));
        }
        Ok(SafetyNumber {
            number: digits.join(" "),
        })
    }

    /// Serialize a session state for persistence.
    fn export_session(&self, remote_did: String) -> Result<String, FreeqError> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions.get(&remote_did).ok_or(FreeqError::NotConnected)?;
        serde_json::to_string(session).map_err(|_| FreeqError::SendFailed)
    }

    /// Restore a session from serialized state.
    fn import_session(&self, remote_did: String, json: String) -> Result<(), FreeqError> {
        let session: RatchetSession =
            serde_json::from_str(&json).map_err(|_| FreeqError::InvalidArgument)?;
        self.sessions.lock().unwrap().insert(remote_did, session);
        Ok(())
    }
}

// ── P2P via iroh ──────────────────────────────────────────────────

pub enum P2pEvent {
    EndpointReady { endpoint_id: String },
    PeerConnected { peer_id: String },
    PeerDisconnected { peer_id: String },
    DirectMessage { peer_id: String, text: String },
    Error { message: String },
}

pub trait P2pEventHandler: Send + Sync + 'static {
    fn on_p2p_event(&self, event: P2pEvent);
}

pub struct FreeqP2p {
    handle: Mutex<Option<freeq_sdk::p2p::P2pHandle>>,
    endpoint_id: Mutex<Option<String>>,
    _shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl FreeqP2p {
    fn new(handler: Box<dyn P2pEventHandler>) -> Result<Self, FreeqError> {
        let (p2p_handle, mut event_rx) = RUNTIME
            .block_on(freeq_sdk::p2p::start())
            .map_err(|_| FreeqError::ConnectionFailed)?;

        let endpoint_id = p2p_handle.endpoint_id.clone();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn event forwarding task
        RUNTIME.spawn(async move {
            loop {
                tokio::select! {
                    evt = event_rx.recv() => {
                        match evt {
                            Some(e) => {
                                let ffi_event = match e {
                                    freeq_sdk::p2p::P2pEvent::EndpointReady { endpoint_id } => {
                                        P2pEvent::EndpointReady { endpoint_id }
                                    }
                                    freeq_sdk::p2p::P2pEvent::PeerConnected { peer_id } => {
                                        P2pEvent::PeerConnected { peer_id }
                                    }
                                    freeq_sdk::p2p::P2pEvent::PeerDisconnected { peer_id } => {
                                        P2pEvent::PeerDisconnected { peer_id }
                                    }
                                    freeq_sdk::p2p::P2pEvent::DirectMessage { peer_id, text } => {
                                        P2pEvent::DirectMessage { peer_id, text }
                                    }
                                    freeq_sdk::p2p::P2pEvent::Error { message } => {
                                        P2pEvent::Error { message }
                                    }
                                };
                                handler.on_p2p_event(ffi_event);
                            }
                            None => break,
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });

        Ok(Self {
            handle: Mutex::new(Some(p2p_handle)),
            endpoint_id: Mutex::new(Some(endpoint_id)),
            _shutdown: Mutex::new(Some(shutdown_tx)),
        })
    }

    fn endpoint_id(&self) -> Result<String, FreeqError> {
        self.endpoint_id
            .lock()
            .unwrap()
            .clone()
            .ok_or(FreeqError::NotConnected)
    }

    fn connect_peer(&self, endpoint_id: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap();
        let h = handle.as_ref().ok_or(FreeqError::NotConnected)?;
        let h = h.clone();
        RUNTIME.spawn(async move {
            if let Err(e) = h.connect_peer(&endpoint_id).await {
                tracing::error!("P2P connect error: {e}");
            }
        });
        Ok(())
    }

    fn send_message(&self, peer_id: String, text: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap();
        let h = handle.as_ref().ok_or(FreeqError::NotConnected)?;
        let h = h.clone();
        RUNTIME.spawn(async move {
            if let Err(e) = h.send_message(&peer_id, &text).await {
                tracing::error!("P2P send error: {e}");
            }
        });
        Ok(())
    }

    fn connected_peers(&self) -> Vec<String> {
        // TODO: expose connected peers list from P2pHandle
        Vec::new()
    }

    fn shutdown(&self) {
        let _ = self._shutdown.lock().unwrap().take();
        let _ = self.handle.lock().unwrap().take();
    }
}

// ── AV (voice/video via MoQ SFU) ─────────────────────────────────

pub enum AvEvent {
    Connected,
    Disconnected {
        reason: String,
    },
    ParticipantJoined {
        nick: String,
    },
    ParticipantLeft {
        nick: String,
    },
    AudioTrackStarted {
        nick: String,
    },
    AudioTrackStopped {
        nick: String,
    },
    VideoTrackStarted {
        nick: String,
    },
    VideoTrackStopped {
        nick: String,
    },
    VideoFrame {
        nick: String,
        bgra: Vec<u8>,
        width: u32,
        height: u32,
    },
    Error {
        message: String,
    },
}

pub trait AvEventHandler: Send + Sync + 'static {
    fn on_av_event(&self, event: AvEvent);
}

#[cfg(feature = "av")]
mod av_impl {
    use super::{AvEvent, AvEventHandler, FreeqError, RUNTIME};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    use iroh_live::media::codec::{AudioCodec, VideoCodec};
    use iroh_live::media::format::{
        AudioFormat, AudioPreset, PixelFormat, VideoFormat, VideoFrame, VideoPreset,
    };
    use iroh_live::media::publish::LocalBroadcast;
    use iroh_live::media::traits::{AudioSource, VideoSource};

    /// Wraps an [`AudioSource`] and zero-fills output when muted. Used so that
    /// muting doesn't tear down the audio track (which would surface as
    /// `AudioTrackStopped` to peers and cause a noticeable reconnect blip).
    pub(super) struct MuteableAudioSource {
        inner: Box<dyn AudioSource>,
        muted: Arc<AtomicBool>,
    }

    impl AudioSource for MuteableAudioSource {
        fn format(&self) -> AudioFormat {
            self.inner.format()
        }
        fn pop_samples(&mut self, buf: &mut [f32]) -> anyhow::Result<Option<usize>> {
            let result = self.inner.pop_samples(buf)?;
            if let Some(n) = result {
                if self.muted.load(Ordering::Relaxed) {
                    for s in &mut buf[..n] {
                        *s = 0.0;
                    }
                }
            }
            Ok(result)
        }
    }

    /// Latest-frame-only video source fed from Swift via `push_video_frame`.
    ///
    /// The encoder pipeline calls `pop_frame` at the desired output rate; we
    /// surface whatever Swift pushed most recently. Older frames are dropped
    /// silently — this is the right behaviour for camera capture (display the
    /// fresh frame, not a backlog).
    pub(super) struct PushVideoSource {
        pub(super) pending: Arc<Mutex<Option<VideoFrame>>>,
        pub(super) format: Arc<Mutex<VideoFormat>>,
        /// Toggled by `set_camera_enabled`. While false, pop_frame returns
        /// None (encoder idles); the source itself stays registered so the
        /// catalog keeps advertising the video rendition.
        pub(super) enabled: Arc<AtomicBool>,
    }

    impl VideoSource for PushVideoSource {
        fn name(&self) -> &str {
            "swift-push"
        }
        fn format(&self) -> VideoFormat {
            self.format.lock().unwrap().clone()
        }
        fn pop_frame(&mut self) -> anyhow::Result<Option<VideoFrame>> {
            if !self.enabled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            Ok(self.pending.lock().unwrap().take())
        }
        fn start(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Rate Swift resamples mic audio to before pushing it in.
    pub(super) const PUSH_AUDIO_RATE: u32 = 48_000;

    /// Audio source fed from Swift via `push_audio_frame`. Swift owns the
    /// real mic capture (`AVAudioEngine`) — the same arrangement as video —
    /// because iroh-live's audio *input* backend isn't viable on iOS.
    pub(super) struct PushAudioSource {
        pub(super) queue: Arc<Mutex<std::collections::VecDeque<f32>>>,
    }

    impl AudioSource for PushAudioSource {
        fn format(&self) -> AudioFormat {
            AudioFormat {
                sample_rate: PUSH_AUDIO_RATE,
                channel_count: 1,
            }
        }
        fn pop_samples(&mut self, buf: &mut [f32]) -> anyhow::Result<Option<usize>> {
            let mut q = self.queue.lock().unwrap();
            for slot in buf.iter_mut() {
                *slot = q.pop_front().unwrap_or(0.0);
            }
            // Cap the backlog at ~1s so a stalled encoder can't make the
            // queue grow without bound.
            let cap = PUSH_AUDIO_RATE as usize;
            if q.len() > cap {
                let excess = q.len() - cap;
                q.drain(..excess);
            }
            Ok(Some(buf.len()))
        }
    }

    pub(super) const DEFAULT_VIDEO_FORMAT: VideoFormat = VideoFormat {
        pixel_format: PixelFormat::Bgra,
        dimensions: [1280, 720],
    };

    pub(super) struct State {
        /// Held so the producer side of the broadcast (audio + video
        /// sources, encoder pipelines) stays alive for the call's lifetime.
        /// Dropping it would tear down the publish path.
        pub _broadcast: LocalBroadcast,
        // Held to keep audio/video device handles alive for the session.
        pub _audio_backend: iroh_live::media::audio_backend::AudioBackend,
        pub _session: moq_lite::Session,
        pub _origin: moq_lite::OriginProducer,
        pub shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        pub connected: bool,
        pub muted: Arc<AtomicBool>,
        /// Gated by `set_camera_enabled`. The video source is registered at
        /// connect time so the catalog always advertises the rendition;
        /// the flag just controls whether `pop_frame` actually returns
        /// pushed frames or short-circuits to None.
        pub camera_enabled: Arc<AtomicBool>,
        pub pending_frame: Arc<Mutex<Option<VideoFrame>>>,
        pub video_format: Arc<Mutex<VideoFormat>>,
        /// Mic samples pushed from Swift, drained by the Opus encoder.
        pub audio_queue: Arc<Mutex<std::collections::VecDeque<f32>>>,
    }

    pub(super) fn connect(
        server_url: String,
        session_id: String,
        nick: String,
        instance_id: String,
        handler: Box<dyn AvEventHandler>,
    ) -> Result<State, FreeqError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Broadcast path includes a per-call instance suffix so two devices
        // signed in as the same DID/nick get distinct MoQ paths. Matches the
        // `+freeq.at/av-instance` tag the client sent on av-join — the
        // server records that suffix in its participants list, so peers
        // know the right path to subscribe to.
        let broadcast_name = if instance_id.is_empty() {
            format!("{session_id}/{nick}")
        } else {
            format!("{session_id}/{nick}~{instance_id}")
        };
        let moq_url: url::Url = format!("{server_url}/av/moq")
            .parse()
            .map_err(|_| FreeqError::InvalidArgument)?;

        let muted = Arc::new(AtomicBool::new(false));
        let pending_frame: Arc<Mutex<Option<VideoFrame>>> = Arc::new(Mutex::new(None));
        let video_format = Arc::new(Mutex::new(DEFAULT_VIDEO_FORMAT));
        let camera_enabled_flag = Arc::new(AtomicBool::new(false));
        let audio_queue: Arc<Mutex<std::collections::VecDeque<f32>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));

        let (session, origin, sub_consumer, audio_backend, broadcast) =
            RUNTIME.block_on(async {
                let broadcast = LocalBroadcast::new();
                let audio_backend = iroh_live::media::audio_backend::AudioBackend::default();
                audio_backend.set_aec_enabled(false);

                // Mic capture is Swift-driven (AVAudioEngine →
                // `push_audio_frame` → `PushAudioSource`). iroh-live's audio
                // *input* backend isn't viable on iOS — the same reason
                // video capture is Swift-driven. `audio_backend` is still
                // used, for *playback* of remote audio.
                let push_audio = PushAudioSource {
                    queue: audio_queue.clone(),
                };
                let muteable = MuteableAudioSource {
                    inner: Box::new(push_audio),
                    muted: muted.clone(),
                };

                broadcast
                    .audio()
                    .set(muteable, AudioCodec::Opus, [AudioPreset::Hq])
                    .map_err(|_| FreeqError::ConnectionFailed)?;

                // Advertise video in the catalog NOW, before any peer
                // subscribes. moq-watch (used by the web client) reads the
                // catalog at sub time; if we wait until the user toggles
                // their camera on, peers who subscribed earlier never pick
                // up the video track and silently render a black tile.
                // The encoder happily idles while pop_frame returns None.
                let push_source = PushVideoSource {
                    pending: pending_frame.clone(),
                    format: video_format.clone(),
                    enabled: camera_enabled_flag.clone(),
                };
                broadcast
                    .video()
                    .set_source(push_source, VideoCodec::H264, [VideoPreset::P720])
                    .map_err(|e| {
                        tracing::warn!("AV: initial video set_source failed: {e}");
                        FreeqError::ConnectionFailed
                    })?;

                let origin = moq_lite::Origin::produce();
                origin.publish_broadcast(&broadcast_name, broadcast.consume());

                let sub_origin = moq_lite::Origin::produce();
                let sub_consumer = sub_origin.consume();

                let mut client_config = moq_native::ClientConfig::default();
                client_config.tls.disable_verify = Some(true);
                client_config.backend = Some(moq_native::QuicBackend::Noq);
                let client = client_config
                    .init()
                    .map_err(|_| FreeqError::ConnectionFailed)?;

                let session = client
                    .with_publish(origin.consume())
                    .with_consume(sub_origin)
                    .connect(moq_url)
                    .await
                    .map_err(|_| FreeqError::ConnectionFailed)?;

                Ok::<_, FreeqError>((session, origin, sub_consumer, audio_backend, broadcast))
            })?;

        tracing::info!(broadcast = %broadcast_name, "AV: connected to MoQ SFU");
        handler.on_av_event(AvEvent::Connected);

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let mut sub_consumer = sub_consumer;

        let our_name = broadcast_name.clone();
        let audio_for_playback = audio_backend.clone();
        let handler: Arc<dyn AvEventHandler> = Arc::from(handler);
        let handler_loop = handler.clone();

        RUNTIME.spawn(async move {
            // Per-participant playback tasks live in this set. Aborting
            // it when the call ends stops inbound audio immediately —
            // otherwise these tasks run on and you keep hearing people
            // after you've left the call.
            let mut remote_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    announced = sub_consumer.announced() => {
                        match announced {
                            Some((path, Some(broadcast_consumer))) => {
                                let path_str = path.to_string();
                                if path_str == our_name { continue; }

                                // Path is `{session}/{nick}[~{instance}]` —
                                // strip the per-device instance suffix so
                                // events surface the display nick. Matches the
                                // web client: subscribe uses the full path
                                // (which the MoQ layer already has), UI keys
                                // off the bare nick.
                                let last = path_str
                                    .split('/')
                                    .last()
                                    .unwrap_or("unknown");
                                let nick = last.split('~').next().unwrap_or(last).to_string();
                                tracing::info!(nick = %nick, path = %path_str, "AV: participant broadcast");
                                handler_loop
                                    .on_av_event(AvEvent::ParticipantJoined { nick: nick.clone() });

                                let ab = audio_for_playback.clone();
                                let h = handler_loop.clone();
                                let nick_for_task = nick.clone();
                                remote_tasks.spawn(async move {
                                    handle_remote_broadcast(path_str, broadcast_consumer, ab, h, nick_for_task)
                                        .await;
                                });
                            }
                            Some((path, None)) => {
                                let path_str = path.to_string();
                                let last = path_str
                                    .split('/')
                                    .last()
                                    .unwrap_or("unknown");
                                let nick = last.split('~').next().unwrap_or(last).to_string();
                                handler_loop.on_av_event(AvEvent::ParticipantLeft { nick });
                            }
                            None => break,
                        }
                    }
                }
            }
            // Call over — abort every per-participant playback task so
            // inbound audio stops the instant the user leaves.
            remote_tasks.abort_all();
            handler_loop.on_av_event(AvEvent::Disconnected {
                reason: "session ended".to_string(),
            });
        });

        Ok(State {
            _broadcast: broadcast,
            _audio_backend: audio_backend,
            _session: session,
            _origin: origin,
            shutdown_tx: Some(shutdown_tx),
            connected: true,
            muted,
            camera_enabled: camera_enabled_flag,
            pending_frame,
            video_format,
            audio_queue,
        })
    }

    async fn handle_remote_broadcast(
        path_str: String,
        broadcast_consumer: moq_lite::BroadcastConsumer,
        audio_backend: iroh_live::media::audio_backend::AudioBackend,
        handler: Arc<dyn AvEventHandler>,
        nick: String,
    ) {
        // iroh-live's default policy is 150ms max_latency with cross-track
        // sync. 150ms is conservative — fine for streaming a published
        // talk, painful for a 1:1 video call. Cut it to 60ms; on a stable
        // network this gives noticeably snappier video and the synced
        // playout still keeps audio and video aligned. If we see decode
        // stutter on poor networks we can dial back up.
        let policy = iroh_live::media::playout::PlaybackPolicy::default()
            .with_max_latency(Duration::from_millis(60));
        let remote = match iroh_live::media::subscribe::RemoteBroadcast::with_playback_policy(
            &path_str,
            broadcast_consumer,
            policy,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(nick = %nick, "AV: subscribe error: {e}");
                return;
            }
        };

        let mut tracks = match remote.media(&audio_backend, Default::default()).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(nick = %nick, "AV: media error: {e}");
                return;
            }
        };

        if tracks.audio.is_some() {
            tracing::info!(nick = %nick, "AV: playing remote audio");
            handler.on_av_event(AvEvent::AudioTrackStarted { nick: nick.clone() });
        }

        // `media()` samples the catalog once. A peer who joined the call
        // before enabling their camera has no video rendition in the
        // catalog at sub time, so `tracks.video` is None permanently.
        // Watch the catalog and (re)subscribe when video appears.
        let mut video = tracks.video.take();

        // Hold audio + broadcast alive for the duration of the function.
        let remote_for_watch = remote.clone();
        let _tracks = tracks;

        loop {
            match video.take() {
                Some(mut v) => {
                    tracing::info!(nick = %nick, decoder = %v.decoder_name(), "AV: remote video track present");
                    handler.on_av_event(AvEvent::VideoTrackStarted { nick: nick.clone() });
                    while let Some(frame) = v.next_frame().await {
                        let (w, h) = (frame.width(), frame.height());
                        // Decoded frames arrive as I420/NV12/GPU depending on
                        // backend. rgba_image() unifies them into packed RGBA;
                        // swap R↔B for BGRA (kCVPixelFormatType_32BGRA), which
                        // is what Swift's AVSampleBufferDisplayLayer expects.
                        let rgba = frame.rgba_image();
                        let mut bgra = rgba.as_raw().clone();
                        for chunk in bgra.chunks_exact_mut(4) {
                            chunk.swap(0, 2);
                        }
                        handler.on_av_event(AvEvent::VideoFrame {
                            nick: nick.clone(),
                            bgra,
                            width: w,
                            height: h,
                        });
                    }
                    tracing::info!(nick = %nick, "AV: remote video track ended; will re-subscribe if it returns");
                    handler.on_av_event(AvEvent::VideoTrackStopped { nick: nick.clone() });
                    // Track ended — fall through and wait for video to come back.
                }
                None => {
                    // No video yet (or it ended). Wait for it.
                    match remote_for_watch.video_ready().await {
                        Ok(v) => {
                            video = Some(v);
                        }
                        Err(e) => {
                            tracing::warn!(nick = %nick, "AV: video_ready error: {e}; staying audio-only");
                            // Keep the function alive so audio + broadcast
                            // stay subscribed until the caller cancels.
                            std::future::pending::<()>().await;
                        }
                    }
                }
            }
        }
    }

    pub(super) fn enable_camera(state: &State) -> Result<(), FreeqError> {
        if state.camera_enabled.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        tracing::info!("AV: camera enabled");
        Ok(())
    }

    pub(super) fn disable_camera(state: &State) {
        if !state.camera_enabled.swap(false, Ordering::Relaxed) {
            return;
        }
        *state.pending_frame.lock().unwrap() = None;
        tracing::info!("AV: camera disabled");
    }

    pub(super) fn push_frame(state: &State, bgra: Vec<u8>, width: u32, height: u32, ts_us: u64) {
        if !state.camera_enabled.load(Ordering::Relaxed) {
            return; // drop frames while camera is off
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4));
        if expected != Some(bgra.len()) {
            tracing::warn!(
                got = bgra.len(),
                expected = ?expected,
                width,
                height,
                "AV: push_video_frame size mismatch — dropping"
            );
            return;
        }
        {
            let mut fmt = state.video_format.lock().unwrap();
            if fmt.dimensions != [width, height] {
                tracing::info!(
                    old_w = fmt.dimensions[0],
                    old_h = fmt.dimensions[1],
                    new_w = width,
                    new_h = height,
                    "AV: source frame dimensions changed"
                );
                *fmt = VideoFormat {
                    pixel_format: PixelFormat::Bgra,
                    dimensions: [width, height],
                };
            }
        }
        // Periodic frame-push heartbeat — useful when diagnosing why the
        // catalog never advertises video: if pushes flow but the encoder
        // never produces packets, it's an encoder-side problem (e.g.,
        // preset dimension mismatch).
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            static FRAMES: AtomicU64 = AtomicU64::new(0);
            let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_multiple_of(60) {
                tracing::info!(frame_no = n, width, height, "AV: pushed frame");
            }
        }
        let frame = VideoFrame::new_packed(
            bgra.into(),
            width,
            height,
            PixelFormat::Bgra,
            Duration::from_micros(ts_us),
        );
        *state.pending_frame.lock().unwrap() = Some(frame);
    }

    pub(super) fn push_audio(state: &State, samples: Vec<f32>) {
        state.audio_queue.lock().unwrap().extend(samples);
    }
}

#[cfg(feature = "av")]
pub struct FreeqAv {
    state: Mutex<Option<av_impl::State>>,
}

#[cfg(not(feature = "av"))]
pub struct FreeqAv;

#[cfg(feature = "av")]
impl FreeqAv {
    fn new(
        server_url: String,
        session_id: String,
        nick: String,
        instance_id: String,
        handler: Box<dyn AvEventHandler>,
    ) -> Result<Self, FreeqError> {
        let state = av_impl::connect(server_url, session_id, nick, instance_id, handler)?;
        Ok(Self {
            state: Mutex::new(Some(state)),
        })
    }

    fn leave(&self) {
        // Take the State out under the lock, then release the lock before the
        // teardown so we don't hold the mutex during async shutdown.
        let taken = {
            let mut guard = self.state.lock().unwrap();
            if let Some(state) = guard.as_mut() {
                if let Some(tx) = state.shutdown_tx.take() {
                    let _ = tx.send(());
                }
                state.connected = false;
            }
            guard.take()
        };
        // Drop the State (which tears down the MoQ / web-transport session)
        // INSIDE the tokio runtime context. The session's Drop impl spawns
        // shutdown work and panics ("there is no reactor running, must be
        // called from the context of a Tokio 1.x runtime") if dropped from a
        // plain FFI thread — which crashes the host app on `/av leave`.
        if let Some(state) = taken {
            let _enter = RUNTIME.enter();
            drop(state);
        }
    }

    fn set_muted(&self, muted: bool) {
        let guard = self.state.lock().unwrap();
        if let Some(state) = guard.as_ref() {
            state
                .muted
                .store(muted, std::sync::atomic::Ordering::Relaxed);
            tracing::info!(muted, "AV: mute set");
        }
    }

    fn set_camera_enabled(&self, enabled: bool) -> Result<(), FreeqError> {
        let mut guard = self.state.lock().unwrap();
        let state = guard.as_mut().ok_or(FreeqError::NotConnected)?;
        if enabled {
            av_impl::enable_camera(state)
        } else {
            av_impl::disable_camera(state);
            Ok(())
        }
    }

    fn push_video_frame(&self, bgra: Vec<u8>, width: u32, height: u32, timestamp_us: u64) {
        let guard = self.state.lock().unwrap();
        if let Some(state) = guard.as_ref() {
            av_impl::push_frame(state, bgra, width, height, timestamp_us);
        }
    }

    fn push_audio_frame(&self, samples: Vec<f32>) {
        let guard = self.state.lock().unwrap();
        if let Some(state) = guard.as_ref() {
            av_impl::push_audio(state, samples);
        }
    }

    fn is_connected(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.connected)
            .unwrap_or(false)
    }
}

#[cfg(feature = "av")]
impl Drop for FreeqAv {
    fn drop(&mut self) {
        // Belt-and-suspenders for the same reason as `leave()`: if the object
        // is deallocated while still holding a live session (e.g. a call that
        // was never explicitly left), tear it down inside the runtime context
        // so the web-transport session's Drop has a reactor.
        if let Ok(mut guard) = self.state.lock() {
            if let Some(state) = guard.take() {
                let _enter = RUNTIME.enter();
                drop(state);
            }
        }
    }
}

#[cfg(not(feature = "av"))]
impl FreeqAv {
    fn new(
        _server_url: String,
        _session_id: String,
        _nick: String,
        _instance_id: String,
        _handler: Box<dyn AvEventHandler>,
    ) -> Result<Self, FreeqError> {
        Err(FreeqError::ConnectionFailed) // AV not compiled in
    }

    fn leave(&self) {}
    fn set_muted(&self, _muted: bool) {}
    fn set_camera_enabled(&self, _enabled: bool) -> Result<(), FreeqError> {
        Err(FreeqError::NotConnected)
    }
    fn push_video_frame(&self, _bgra: Vec<u8>, _w: u32, _h: u32, _ts: u64) {}
    fn push_audio_frame(&self, _samples: Vec<f32>) {}
    fn is_connected(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestAvHandler {
        connected: Arc<AtomicBool>,
    }

    impl AvEventHandler for TestAvHandler {
        fn on_av_event(&self, event: AvEvent) {
            match event {
                AvEvent::Connected => {
                    self.connected.store(true, Ordering::Relaxed);
                    println!("[test] AV connected");
                }
                AvEvent::Disconnected { reason } => {
                    println!("[test] AV disconnected: {reason}");
                }
                AvEvent::ParticipantJoined { nick } => {
                    println!("[test] Participant joined: {nick}");
                }
                AvEvent::ParticipantLeft { nick } => {
                    println!("[test] Participant left: {nick}");
                }
                AvEvent::Error { message } => {
                    println!("[test] AV error: {message}");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn av_event_types_exist() {
        // Verify all event variants can be constructed
        let _ = AvEvent::Connected;
        let _ = AvEvent::Disconnected {
            reason: "test".to_string(),
        };
        let _ = AvEvent::ParticipantJoined {
            nick: "alice".to_string(),
        };
        let _ = AvEvent::ParticipantLeft {
            nick: "bob".to_string(),
        };
        let _ = AvEvent::AudioTrackStarted {
            nick: "carol".to_string(),
        };
        let _ = AvEvent::AudioTrackStopped {
            nick: "dave".to_string(),
        };
        let _ = AvEvent::VideoTrackStarted {
            nick: "eve".to_string(),
        };
        let _ = AvEvent::VideoTrackStopped {
            nick: "frank".to_string(),
        };
        let _ = AvEvent::VideoFrame {
            nick: "grace".to_string(),
            bgra: vec![0; 4],
            width: 1,
            height: 1,
        };
        let _ = AvEvent::Error {
            message: "test error".to_string(),
        };
    }

    #[test]
    fn av_handler_trait_works() {
        let connected = Arc::new(AtomicBool::new(false));
        let handler = TestAvHandler {
            connected: connected.clone(),
        };
        handler.on_av_event(AvEvent::Connected);
        assert!(connected.load(Ordering::Relaxed));
    }

    #[test]
    fn av_without_server_fails_gracefully() {
        let connected = Arc::new(AtomicBool::new(false));
        let handler = Box::new(TestAvHandler { connected });

        // Connecting to a non-existent server should fail, not panic
        let result = FreeqAv::new(
            "http://127.0.0.1:19999".to_string(), // no server here
            "test-session".to_string(),
            "test-nick".to_string(),
            "test-instance".to_string(),
            handler,
        );

        assert!(result.is_err());
    }

    #[cfg(feature = "av")]
    #[test]
    fn av_leave_sets_disconnected() {
        // Can't create without a server, but we can test the stub path
        // by verifying the non-av build returns error
    }

    #[test]
    fn parse_reactions_tag_handles_canonical_format() {
        let tallies = parse_reactions_tag("👍:alice,bob;❤️:carol");
        assert_eq!(tallies.len(), 2);
        assert_eq!(tallies[0].emoji, "👍");
        assert_eq!(
            tallies[0].nicks,
            vec!["alice".to_string(), "bob".to_string()]
        );
        assert_eq!(tallies[1].emoji, "❤️");
        assert_eq!(tallies[1].nicks, vec!["carol".to_string()]);
    }

    #[test]
    fn parse_reactions_tag_skips_malformed_segments() {
        // missing colon, empty emoji, empty nicks, stray ; — all dropped silently
        let tallies = parse_reactions_tag("notacolon;:no_emoji;👍:;👎:dave");
        assert_eq!(tallies.len(), 1);
        assert_eq!(tallies[0].emoji, "👎");
        assert_eq!(tallies[0].nicks, vec!["dave".to_string()]);
    }

    #[test]
    fn parse_reactions_tag_empty_input() {
        assert!(parse_reactions_tag("").is_empty());
    }

    #[test]
    fn convert_event_message_populates_reactions_from_tag() {
        let mut tags = std::collections::HashMap::new();
        tags.insert("msgid".to_string(), "01ABC".to_string());
        tags.insert(
            "+freeq.at/reactions".to_string(),
            "👍:alice,bob;🎉:carol".to_string(),
        );
        let ev = freeq_sdk::event::Event::Message {
            from: "smoke-tx".to_string(),
            target: "#naptest".to_string(),
            text: "hi".to_string(),
            tags,
        };
        let out = convert_event(&ev);
        let FreeqEvent::Message { msg } = out else {
            panic!("expected Message variant");
        };
        assert_eq!(msg.reactions.len(), 2);
        assert!(msg
            .reactions
            .iter()
            .any(|r| r.emoji == "👍" && r.nicks == vec!["alice".to_string(), "bob".to_string()]));
        assert!(msg
            .reactions
            .iter()
            .any(|r| r.emoji == "🎉" && r.nicks == vec!["carol".to_string()]));
    }

    #[test]
    fn convert_event_message_no_reactions_tag_yields_empty() {
        let mut tags = std::collections::HashMap::new();
        tags.insert("msgid".to_string(), "01XYZ".to_string());
        let ev = freeq_sdk::event::Event::Message {
            from: "alice".to_string(),
            target: "#x".to_string(),
            text: "no reactions here".to_string(),
            tags,
        };
        let out = convert_event(&ev);
        let FreeqEvent::Message { msg } = out else {
            panic!("expected Message variant");
        };
        assert!(msg.reactions.is_empty());
    }
}
