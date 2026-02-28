# Build Your First freeq Bot in 10 Minutes

This guide walks you through building, running, and extending an IRC bot on freeq using the Rust SDK.

## Prerequisites

- Rust (1.75+)
- A running freeq server (or use `irc.freeq.at:6697`)

## 1. Create the project

```bash
cargo new mybot
cd mybot
cargo add freeq-sdk --path ../freeq-sdk  # or from crates.io
cargo add tokio --features full
cargo add clap --features derive
cargo add tracing-subscriber
cargo add anyhow
```

## 2. Write the bot

```rust
// src/main.rs
use anyhow::Result;
use freeq_sdk::bot::Bot;
use freeq_sdk::client::{ClientHandle, ConnectConfig, ReconnectConfig, run_with_reconnect};
use freeq_sdk::event::Event;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Build the bot
    let mut bot = Bot::new("!", "mybot")
        .rate_limit(5, Duration::from_secs(30));  // 5 commands per 30s per user

    bot.command("ping", "Check if the bot is alive", |ctx| {
        Box::pin(async move {
            ctx.react("🏓").await?;    // react to the message
            ctx.reply_to("pong!").await // reply with mention
        })
    });

    bot.command("echo", "Echo your message", |ctx| {
        Box::pin(async move {
            let text = ctx.args_str();
            if text.is_empty() {
                ctx.reply("Usage: !echo <message>").await
            } else {
                ctx.reply_in_thread(&text).await  // reply in thread
            }
        })
    });

    // Connect with auto-reconnect
    let config = ConnectConfig {
        server_addr: "irc.freeq.at:6697".into(),
        nick: "mybot".into(),
        user: "mybot".into(),
        realname: "My First Bot".into(),
        tls: true,
        ..Default::default()
    };

    let reconnect = ReconnectConfig {
        channels: vec!["#bots".into()],
        ..Default::default()
    };

    let bot = Arc::new(bot);
    run_with_reconnect(config, None, reconnect, move |handle: ClientHandle, event: Event| {
        let bot = bot.clone();
        Box::pin(async move {
            bot.handle_event(&handle, &event).await;
            Ok(())
        })
    }).await
}
```

## 3. Run it

```bash
cargo run
```

That's it. The bot connects to `irc.freeq.at`, joins `#bots`, and responds to `!ping`, `!echo`, and `!help`. It auto-reconnects on disconnect with exponential backoff.

## Core Concepts

### Commands

```rust
// Anyone can use this
bot.command("ping", "description", handler);

// Only DID-authenticated users
bot.auth_command("secret", "description", handler);

// Only admin DIDs
let bot = Bot::new("!", "mybot").admin("did:plc:abc123");
bot.admin_command("kick", "description", handler);
```

### CommandContext

Every handler receives a `CommandContext` with:

| Method | Description |
|---|---|
| `ctx.reply("text")` | Send to channel or PM |
| `ctx.reply_to("text")` | Reply with `nick: text` prefix |
| `ctx.reply_in_thread("text")` | Reply in thread (uses +draft/reply) |
| `ctx.react("🔥")` | React to the triggering message |
| `ctx.typing()` / `ctx.typing_done()` | Send typing indicator |
| `ctx.arg(0)` | Get Nth argument |
| `ctx.args_str()` | Full argument string |
| `ctx.sender` | Who sent the command |
| `ctx.sender_did` | Their DID (if authenticated) |
| `ctx.msgid()` | Message ID from IRCv3 tags |
| `ctx.is_channel` | True if sent in a channel |

### ClientHandle Helpers

The handle (available via `ctx.handle`) has convenience methods:

```rust
// Messaging
handle.privmsg("#chan", "hello").await;
handle.reply("#chan", "msgid123", "threaded reply").await;
handle.edit_message("#chan", "msgid123", "corrected text").await;
handle.delete_message("#chan", "msgid123").await;

// Channels
handle.join_many(&["#a", "#b", "#c"]).await;
handle.mode("#chan", "+o", Some("nick")).await;
handle.topic("#chan", "New topic").await;
handle.pin("#chan", "msgid123").await;

// Typing
handle.typing_start("#chan").await;
handle.typing_stop("#chan").await;

// History
handle.history_latest("#chan", 50).await;
handle.history_before("#chan", "msgid", 20).await;

// Reactions
handle.react("#chan", "🎉", "msgid123").await;
```

### Rate Limiting

Built-in per-user rate limiting:

```rust
let bot = Bot::new("!", "mybot")
    .rate_limit(5, Duration::from_secs(30))  // 5 cmds / 30s
    .max_args(500);  // reject args > 500 chars
```

When a user exceeds the limit, the bot replies "Slow down — too many commands." automatically.

### Reconnection

`run_with_reconnect` handles the full lifecycle:

- Connects to the server
- Rejoins configured channels after each reconnect
- Exponential backoff with jitter (2s → 4s → 8s → ... → 30s cap)
- Calls your handler for every event (including `Disconnected`)

```rust
let reconnect = ReconnectConfig {
    channels: vec!["#bots".into(), "#ops".into()],
    initial_delay: Duration::from_secs(2),
    max_delay: Duration::from_secs(30),
    ..Default::default()
};
```

### Fallback Handler

Catch messages that don't match any command:

```rust
bot.on_message(|ctx| {
    Box::pin(async move {
        if ctx.args_raw.to_lowercase().contains("hello") {
            ctx.react("👋").await?;
        }
        Ok(())
    })
});
```

### Permissions

Three levels, checked before the handler runs:

| Level | Check |
|---|---|
| `Anyone` | No check |
| `Authenticated` | `sender_did.is_some()` |
| `Admin` | DID in bot's admin list |

## Examples

See `freeq-sdk/examples/`:

- **`echo_bot.rs`** — Minimal bot (10 lines of logic)
- **`framework_bot.rs`** — Command routing + permissions
- **`moderation_bot.rs`** — Full-featured: threads, reactions, typing, rate limiting, admin commands, auto-reconnect

## What's Next

- **Media uploads**: Use `freeq_sdk::media` + PDS OAuth to upload images/audio and share via `handle.send_media()`
- **E2EE channels**: Use `freeq_sdk::e2ee` for encrypted channel messages
- **AT Protocol identity**: Authenticate as a DID with `--handle alice.bsky.social` for verified bot identity
- **Custom IRC**: Use `handle.raw()` for any IRC command not covered by helpers
