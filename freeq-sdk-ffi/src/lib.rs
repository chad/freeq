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
    pub is_action: bool,
    pub timestamp_ms: i64,
}

pub struct IrcMember {
    pub nick: String,
    pub is_op: bool,
    pub is_voiced: bool,
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
    Message { msg: IrcMessage },
    Names { channel: String, members: Vec<IrcMember> },
    TopicChanged { channel: String, topic: ChannelTopic },
    ModeChanged { channel: String, mode: String, arg: Option<String>, set_by: String },
    Kicked { channel: String, nick: String, by: String, reason: String },
    UserQuit { nick: String, reason: String },
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
    handle: Mutex<Option<freeq_sdk::client::ClientHandle>>,
    connected: Arc<Mutex<bool>>,
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
            handle: Mutex::new(None),
            connected: Arc::new(Mutex::new(false)),
        })
    }

    pub fn connect(&self) -> Result<(), FreeqError> {
        let nick = self.nick.lock().unwrap().clone();
        let config = freeq_sdk::client::ConnectConfig {
            server_addr: self.server.clone(),
            nick: nick.clone(),
            user: nick.clone(),
            realname: "freeq iOS".to_string(),
            tls: self.server.contains(":6697") || self.server.contains(":443"),
            tls_insecure: false,
        };

        let (client_handle, mut event_rx) = freeq_sdk::client::connect(config, None);

        *self.handle.lock().unwrap() = Some(client_handle);
        *self.connected.lock().unwrap() = true;

        let handler = self.handler.clone();
        let nick_state = self.nick.clone();
        let connected_state = self.connected.clone();

        RUNTIME.spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let ffi_event = convert_event(&event);
                if let FreeqEvent::Disconnected { .. } = &ffi_event {
                    *connected_state.lock().unwrap() = false;
                }
                if let FreeqEvent::Registered { ref nick } = &ffi_event {
                    *nick_state.lock().unwrap() = nick.clone();
                }
                handler.on_event(ffi_event);
            }
        });

        Ok(())
    }

    pub fn disconnect(&self) {
        if let Some(handle) = self.handle.lock().unwrap().take() {
            RUNTIME.block_on(async {
                let _ = handle.quit(Some("Goodbye")).await;
            });
        }
        *self.connected.lock().unwrap() = false;
    }

    pub fn join(&self, channel: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        RUNTIME.block_on(async {
            handle.join(&channel).await.map_err(|_| FreeqError::SendFailed)
        })
    }

    pub fn part(&self, channel: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        RUNTIME.block_on(async {
            handle.raw(&format!("PART {channel}")).await.map_err(|_| FreeqError::SendFailed)
        })
    }

    pub fn send_message(&self, target: String, text: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        RUNTIME.block_on(async {
            handle.privmsg(&target, &text).await.map_err(|_| FreeqError::SendFailed)
        })
    }

    pub fn send_raw(&self, line: String) -> Result<(), FreeqError> {
        let handle = self.handle.lock().unwrap().clone().ok_or(FreeqError::NotConnected)?;
        RUNTIME.block_on(async {
            handle.raw(&line).await.map_err(|_| FreeqError::SendFailed)
        })
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
                    is_action,
                    timestamp_ms: ts,
                },
            }
        }
        Event::Names { channel, nicks } => {
            let members = nicks.iter().map(|n| {
                let (is_op, is_voiced, nick) = if n.starts_with('@') {
                    (true, false, n[1..].to_string())
                } else if n.starts_with('+') {
                    (false, true, n[1..].to_string())
                } else {
                    (false, false, n.clone())
                };
                IrcMember { nick, is_op, is_voiced }
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
        Event::Disconnected { reason } => FreeqEvent::Disconnected { reason: reason.clone() },
        Event::Invited { channel, by } => FreeqEvent::Notice { text: format!("{by} invited you to {channel}") },
        _ => FreeqEvent::Notice { text: String::new() },
    }
}
