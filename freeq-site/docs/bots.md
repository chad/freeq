# Bot Framework

freeq comes with a Rust SDK and a bot framework for building IRC bots, integrations, and LLM-powered chat personas.

## Quick start

Add `freeq-sdk` to your `Cargo.toml`:

```toml
[dependencies]
freeq-sdk = { path = "../freeq-sdk" }
tokio = { version = "1", features = ["full"] }
```

### Echo bot

```rust
use freeq_sdk::client::{connect, ConnectConfig};
use freeq_sdk::event::Event;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (handle, mut events) = connect(&ConnectConfig {
        server_addr: "irc.freeq.at:6667".into(),
        nick: "echobot".into(),
        user: "echobot".into(),
        realname: "Echo Bot".into(),
        ..Default::default()
    }).await?;

    handle.join("#test").await?;

    while let Some(event) = events.recv().await {
        if let Event::Message { from, target, text, .. } = event {
            if target.starts_with('#') && text.starts_with("!echo ") {
                handle.privmsg(&target, &text[6..]).await?;
            }
        }
    }
    Ok(())
}
```

### SDK pattern

The SDK uses a `(ClientHandle, Receiver<Event>)` pattern:

- **`ClientHandle`** — Send commands: `join()`, `part()`, `privmsg()`, `mode()`, `raw()`
- **`Receiver<Event>`** — Receive events: messages, joins, parts, kicks, mode changes, reactions, edits, deletions

This separation makes it trivial to build multi-threaded bots or share the handle across tasks.

## Event types

```rust
enum Event {
    Message { from, target, text, tags, msgid },
    Join { nick, channel },
    Part { nick, channel, reason },
    Kick { nick, channel, target, reason },
    Mode { channel, changes },
    Topic { channel, topic, setter },
    Nick { old_nick, new_nick },
    Quit { nick, reason },
    Reaction { from, target, msgid, emoji },
    Edit { from, target, msgid, new_text },
    Delete { from, target, msgid },
    Typing { from, target },
    ServerNotice { text },
    Away { nick, message },
    // ...
}
```

## LLM-powered chatroom

freeq includes a chatroom simulator that creates AI personas in channels using Claude:

```bash
ANTHROPIC_API_KEY=sk-... cargo run --release --bin chatroom -- \
  --server irc.freeq.at:6667 \
  --channel '#general' \
  --bots 5
```

This creates 5 LLM-powered bots with distinct personalities that:

- Read channel messages and respond naturally
- React to messages with emoji
- Have conversations with each other
- Follow channel topic context
- Maintain consistent personality traits

### Building custom personas

The chatroom binary at `freeq-bots/src/bin/chatroom.rs` demonstrates the pattern:

1. Define personality profiles (name, traits, speaking style)
2. Connect each bot via the SDK
3. Feed channel messages into an LLM with persona context
4. Post responses with natural timing

The model defaults to `claude-sonnet-4-20250514` and can be changed with `--model`.

## Webhook bot pattern

```rust
use axum::{Router, Json, routing::post};

#[tokio::main]
async fn main() {
    let (handle, _events) = connect(&config).await.unwrap();
    handle.join("#alerts").await.unwrap();

    let app = Router::new()
        .route("/webhook", post(move |Json(payload): Json<WebhookPayload>| {
            let h = handle.clone();
            async move {
                h.privmsg("#alerts", &format!("🔔 {}: {}", payload.source, payload.message)).await.ok();
                "ok"
            }
        }));

    axum::serve(listener, app).await.unwrap();
}
```

## DM bot pattern

```rust
while let Some(event) = events.recv().await {
    if let Event::Message { from, target, text, .. } = event {
        // DMs: target is the bot's own nick
        if target == "mybot" {
            handle.privmsg(&from, &format!("You said: {text}")).await?;
        }
    }
}
```

## Media-aware bot

Bots can upload and share media:

```rust
use freeq_sdk::media;

// Upload an image
let url = media::upload(&handle, "/path/to/image.png").await?;
handle.privmsg("#channel", &url).await?;
```

## Tips

- **Unique nicks**: Use timestamp suffixes if running multiple instances
- **Rate limiting**: The server doesn't rate-limit yet — be a good citizen
- **Reconnection**: The SDK doesn't auto-reconnect — wrap `connect()` in a retry loop
- **CHATHISTORY**: Call `handle.raw("CHATHISTORY LATEST #channel * 50")` to backfill
- **Error handling**: Listen for `Event::ServerNotice` for error numerics
