# Getting Started

## Connect as a user

### Web client (easiest)

Visit [irc.freeq.at](https://irc.freeq.at). Click **Sign in with Bluesky**, complete the OAuth flow, and you're in. Your Bluesky handle becomes your IRC nick.

### Any IRC client (guest mode)

```
Server: irc.freeq.at
Port:   6667 (plain) or 6697 (TLS)
```

Connect with irssi, WeeChat, Hexchat, mIRC, Textual, or any IRC client. You'll join as a guest — no authentication needed. You can chat in any open channel.

### TUI client

```bash
git clone https://github.com/chad/freeq && cd freeq
cargo run --release --bin freeq-tui
```

The TUI supports AT Protocol authentication, inline images, reactions, and message editing — all in your terminal.

### iOS

The native iOS app is in `freeq-ios/`. Build with:

```bash
./freeq-ios/build-rust.sh
cd freeq-ios && xcodegen generate
open freeq.xcodeproj
```

Sign in with Bluesky from the login screen.

## Run your own server

```bash
git clone https://github.com/chad/freeq && cd freeq
cargo build --release --bin freeq-server

./target/release/freeq-server \
  --listen-addr 0.0.0.0:6667 \
  --web-addr 0.0.0.0:8080 \
  --db-path freeq.db \
  --web-static-dir freeq-app/dist
```

### With the web client

```bash
cd freeq-app && npm install && npm run build && cd ..
```

Then add `--web-static-dir freeq-app/dist` to serve the web client from the same server.

### With TLS

```bash
./target/release/freeq-server \
  --listen-addr 0.0.0.0:6667 \
  --tls-listen-addr 0.0.0.0:6697 \
  --tls-cert /path/to/cert.pem \
  --tls-key /path/to/key.pem \
  --web-addr 0.0.0.0:8080 \
  --db-path freeq.db
```

### With federation

Start two servers and link them via iroh QUIC:

```bash
# Server A
./target/release/freeq-server --iroh --listen-addr 0.0.0.0:6667

# Note server A's iroh endpoint ID from the log output, then:

# Server B
./target/release/freeq-server --iroh --listen-addr 0.0.0.0:6668 \
  --s2s-peers <server-a-endpoint-id>
```

Channels, messages, topics, and ops sync automatically via CRDT convergence.

## Build a bot

```rust
use freeq_sdk::client::{connect, ConnectConfig};
use freeq_sdk::event::Event;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (handle, mut events) = connect(&ConnectConfig {
        server_addr: "irc.freeq.at:6667".into(),
        nick: "mybot".into(),
        user: "mybot".into(),
        realname: "My Bot".into(),
        ..Default::default()
    }).await?;

    handle.join("#general").await?;

    while let Some(event) = events.recv().await {
        match event {
            Event::Message { from, target, text, .. } => {
                if text.starts_with("!hello") {
                    handle.privmsg(&target, &format!("Hello, {from}!")).await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
```

See the [SDK documentation](/sdk/) for the full API and the [Bot Framework](/docs/bots/) guide for LLM-powered personas.
