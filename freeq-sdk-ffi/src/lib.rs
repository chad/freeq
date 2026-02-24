//! FFI wrapper around freeq-sdk for Swift/Kotlin consumption via UniFFI.

use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;

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
    Registered { nick: String },
    Authenticated { did: String },
    AuthFailed { reason: String },
    Joined { channel: String, nick: String },
    Parted { channel: String, nick: String },
    NickChanged { old_nick: String, new_nick: String },
    AwayChanged { nick: String, away_msg: Option<String> },
    Message { msg: IrcMessage },
    TagMsg { msg: TagMessage },
    Names { channel: String, members: Vec<IrcMember> },
    TopicChanged { channel: String, topic: ChannelTopic },
    ModeChanged { channel: String, mode: String, arg: Option<String>, set_by: String },
    Kicked { channel: String, nick: String, by: String, reason: String },
    UserQuit { nick: String, reason: String },
    BatchStart { id: String, batch_type: String, target: String },
    BatchEnd { id: String },
    Notice { text: String },
    Disconnected { reason: String },
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
        })
    }

    pub fn set_web_token(&self, token: String) -> Result<(), FreeqError> {
        tracing::debug!("[FFI] set_web_token called, token len={}", token.len());
        *self.web_token.lock().unwrap() = Some(token);
        Ok(())
    }

    pub fn connect(&self) -> Result<(), FreeqError> {
        let nick = self.nick.lock().unwrap().clone();
        let web_token = self.web_token.lock().unwrap().take();
        tracing::debug!("[FFI] connect: nick={}, web_token={}", nick, web_token.is_some());
        let config = freeq_sdk::client::ConnectConfig {
            server_addr: self.server.clone(),
            nick: nick.clone(),
            user: nick.clone(),
            realname: "freeq".to_string(),
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
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        // Use spawn + oneshot to avoid block_on deadlock
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle.join(&channel).await.map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
    }

    pub fn part(&self, channel: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle.raw(&format!("PART {channel}")).await.map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
    }

    pub fn send_message(&self, target: String, text: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        let (tx, rx) = std::sync::mpsc::channel();
        RUNTIME.spawn(async move {
            let result = handle.privmsg(&target, &text).await.map_err(|_| FreeqError::SendFailed);
            let _ = tx.send(result);
        });
        rx.recv().map_err(|_| FreeqError::SendFailed)?
    }

    pub fn send_raw(&self, line: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
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
        Event::AuthFailed { reason } => FreeqEvent::AuthFailed { reason: reason.clone() },
        Event::Joined { channel, nick } => FreeqEvent::Joined { channel: channel.clone(), nick: nick.clone() },
        Event::Parted { channel, nick } => FreeqEvent::Parted { channel: channel.clone(), nick: nick.clone() },
        Event::Message { from, target, text, tags } => {
            let msgid = tags.get("msgid").cloned();
            let reply_to = tags.get("+reply").cloned();
            let replaces_msgid = tags.get("+draft/edit").cloned();
            let edit_of = tags.get("+draft/edit").cloned();
            let batch_id = tags.get("batch").cloned();
            let is_action = text.starts_with("\x01ACTION ") && text.ends_with('\x01');
            let clean_text = if is_action {
                text.trim_start_matches("\x01ACTION ").trim_end_matches('\x01').to_string()
            } else {
                text.clone()
            };
            let ts = tags.get("time")
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
            let tag_entries = tags.iter().map(|(k, v)| TagEntry {
                key: k.clone(),
                value: v.clone(),
            }).collect::<Vec<_>>();
            FreeqEvent::TagMsg {
                msg: TagMessage {
                    from: from.clone(),
                    target: target.clone(),
                    tags: tag_entries,
                },
            }
        }
        Event::Names { channel, nicks } => {
            let members = nicks.iter().map(|n| {
                let (is_op, is_halfop, is_voiced, nick) = if n.starts_with('@') {
                    (true, false, false, n[1..].to_string())
                } else if n.starts_with('%') {
                    (false, true, false, n[1..].to_string())
                } else if n.starts_with('+') {
                    (false, false, true, n[1..].to_string())
                } else {
                    (false, false, false, n.clone())
                };
                IrcMember { nick, is_op, is_halfop, is_voiced, away_msg: None }
            }).collect();
            FreeqEvent::Names { channel: channel.clone(), members }
        }
        Event::ModeChanged { channel, mode, arg, set_by } => FreeqEvent::ModeChanged {
            channel: channel.clone(), mode: mode.clone(), arg: arg.clone(), set_by: set_by.clone(),
        },
        Event::Kicked { channel, nick, by, reason } => FreeqEvent::Kicked {
            channel: channel.clone(), nick: nick.clone(), by: by.clone(), reason: reason.clone(),
        },
        Event::TopicChanged { channel, topic, set_by } => FreeqEvent::TopicChanged {
            channel: channel.clone(),
            topic: ChannelTopic { text: topic.clone(), set_by: set_by.clone() },
        },
        Event::ServerNotice { text } => FreeqEvent::Notice { text: text.clone() },
        Event::UserQuit { nick, reason } => FreeqEvent::UserQuit { nick: nick.clone(), reason: reason.clone() },
        Event::NickChanged { old_nick, new_nick } => FreeqEvent::NickChanged {
            old_nick: old_nick.clone(),
            new_nick: new_nick.clone(),
        },
        Event::AwayChanged { nick, away_msg } => FreeqEvent::AwayChanged {
            nick: nick.clone(),
            away_msg: away_msg.clone(),
        },
        Event::BatchStart { id, batch_type, target } => FreeqEvent::BatchStart {
            id: id.clone(),
            batch_type: batch_type.clone(),
            target: target.clone(),
        },
        Event::BatchEnd { id } => FreeqEvent::BatchEnd { id: id.clone() },
        Event::Disconnected { reason } => FreeqEvent::Disconnected { reason: reason.clone() },
        Event::Invited { channel, by } => FreeqEvent::Notice { text: format!("{by} invited you to {channel}") },
        _ => FreeqEvent::Notice { text: String::new() },
    }
}
