//! FFI wrapper around freeq-sdk for Swift/Kotlin consumption via UniFFI.

use once_cell::sync::Lazy;
use std::sync::{Arc, Mutex};

static RUNTIME: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
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
    pub is_action: bool,
    pub timestamp_ms: i64,
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

    pub fn connect(&self) -> Result<(), FreeqError> {
        let nick = self.nick.lock().unwrap().clone();
        let web_token = self.web_token.lock().unwrap().take();
        tracing::debug!(
            "[FFI] connect: nick={}, web_token={}",
            nick,
            web_token.is_some()
        );
        let config = freeq_sdk::client::ConnectConfig {
            server_addr: self.server.clone(),
            nick: nick.clone(),
            user: nick.clone(),
            realname: self.platform.lock().unwrap().clone(),
            tls: self.server.contains(":6697") || self.server.contains(":443"),
            tls_insecure: false,
            web_token,
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
        let handle = self
            .handle
            .lock()
            .unwrap()
            .clone()
            .ok_or(FreeqError::NotConnected)?;
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle.raw(&line).await.map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
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
        Event::Joined { channel, nick } => FreeqEvent::Joined {
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
                    is_action,
                    timestamp_ms: ts,
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
        Event::NamesEnd { .. } => {
            // NamesEnd is informational; map to empty notice for FFI consumers
            FreeqEvent::Notice {
                text: String::new(),
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
        Event::Disconnected { reason } => FreeqEvent::Disconnected {
            reason: reason.clone(),
        },
        Event::Invited { channel, by } => FreeqEvent::Notice {
            text: format!("{by} invited you to {channel}"),
        },
        _ => FreeqEvent::Notice {
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
