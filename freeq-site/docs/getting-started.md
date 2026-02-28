# Getting Started

## Connect as a user

The fastest way to try freeq:

1. Open **[irc.freeq.at](https://irc.freeq.at)** in your browser
2. Click **Sign in with Bluesky** (or connect as a guest)
3. You'll land in `#freeq` — say hello

That's it. Your messages are cryptographically signed, your identity is verified, and you're chatting on an open protocol.

## Other ways to connect

### iOS app

Download from the App Store (or build from source). Sign in with Bluesky, same as web.

### Any IRC client

freeq is a standard IRC server. Connect with irssi, weechat, HexChat, or any IRC client:

```
Server: irc.freeq.at
Port: 6697 (TLS)
```

Without Bluesky auth, you'll connect as a guest. All standard IRC features work.

### TUI client

```bash
cargo install freeq-tui
freeq-tui
```

Runs in your terminal. Supports Bluesky OAuth, vi/emacs keybindings, inline images.

## Run your own server

```bash
git clone https://github.com/chad/freeq
cd freeq
cargo build --release -p freeq-server
./target/release/freeq-server --bind 0.0.0.0:6667
```

See the [Self-Hosting Guide](/docs/self-hosting/) for TLS, nginx, systemd, and production configuration.

## Build a bot

```bash
cargo new mybot
cd mybot
# Add freeq-sdk dependency
```

See the [Bot Quickstart](/docs/bot-quickstart/) for a 10-minute tutorial.

## Key concepts

| Concept | What it means |
|---|---|
| **DID** | Decentralized Identifier — your cryptographic identity (e.g., `did:plc:abc123`) |
| **Handle** | Your human-readable name (e.g., `alice.bsky.social`) — resolves to a DID |
| **Signed messages** | Every message from an authenticated user carries an ed25519 signature |
| **Policy** | Channel access rules expressed as verifiable credentials |
| **E2EE** | End-to-end encrypted DMs using X3DH + Double Ratchet |
| **Guest** | Unauthenticated user — standard IRC, no signing, no E2EE |
