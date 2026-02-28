//! Events emitted by the IRC client for the UI layer to consume.

/// Events that the SDK emits to the consumer (TUI, GUI, bot, etc.)
#[derive(Debug, Clone)]
pub enum Event {
    /// Successfully connected to the server.
    Connected,

    /// IRC registration complete. `nick` is our confirmed nick.
    Registered {
        nick: String,
    },

    /// SASL authentication result.
    Authenticated {
        did: String,
    },
    AuthFailed {
        reason: String,
    },

    /// Joined a channel.
    Joined {
        channel: String,
        nick: String,
    },

    /// Someone left a channel.
    Parted {
        channel: String,
        nick: String,
    },

    /// A message in a channel or private message.
    Message {
        from: String,
        target: String,
        text: String,
        /// IRCv3 message tags (empty if none).
        tags: std::collections::HashMap<String, String>,
    },

    /// A TAGMSG (tags only, no body) — used for reactions, typing indicators, etc.
    TagMsg {
        from: String,
        target: String,
        tags: std::collections::HashMap<String, String>,
    },

    /// BATCH start (e.g., chathistory)
    BatchStart {
        id: String,
        batch_type: String,
        target: String,
    },

    /// BATCH end
    BatchEnd {
        id: String,
    },

    /// NAMES list for a channel (one 353 reply; may arrive in multiple parts).
    Names {
        channel: String,
        nicks: Vec<String>,
    },

    /// End of NAMES list (366).
    NamesEnd {
        channel: String,
    },

    /// Channel mode changed.
    ModeChanged {
        channel: String,
        mode: String,
        arg: Option<String>,
        set_by: String,
    },

    /// Someone was kicked from a channel.
    Kicked {
        channel: String,
        nick: String,
        by: String,
        reason: String,
    },

    /// A user's AWAY status changed (via away-notify cap).
    /// `away_msg` is Some("reason") when going away, None when coming back.
    AwayChanged {
        nick: String,
        away_msg: Option<String>,
    },

    /// A user changed nick.
    NickChanged {
        old_nick: String,
        new_nick: String,
    },

    /// We were invited to a channel.
    Invited {
        channel: String,
        by: String,
    },

    /// Channel topic changed or received on join.
    TopicChanged {
        channel: String,
        topic: String,
        set_by: Option<String>,
    },

    /// WHOIS response line (numeric code + text).
    WhoisReply {
        nick: String,
        info: String,
    },

    /// Server sent an error or notice.
    ServerNotice {
        text: String,
    },

    /// Someone quit the server.
    UserQuit {
        nick: String,
        reason: String,
    },

    /// Connection was closed.
    Disconnected {
        reason: String,
    },

    /// Raw server line (for debugging).
    RawLine(String),
}
