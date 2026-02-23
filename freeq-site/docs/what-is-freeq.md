# What is freeq?

freeq is an IRC server for the modern internet. It keeps everything that makes IRC great — open protocol, any client works, simple text messages — and adds the identity layer the internet never had.

When you connect to freeq, you can authenticate with your [Bluesky](https://bsky.app) account. Your decentralized identity (DID) becomes your IRC identity. Your nick follows you across servers. Your channel ops are permanent. And none of this requires abandoning IRC.

## Why IRC?

IRC is the internet's original real-time collaboration protocol. It's been running since 1988. It's an open standard. Hundreds of clients exist. It's text-based, scriptable, and lightweight. There are no terms of service, no tracking pixels, no engagement algorithms.

But IRC has a problem: identity. When you disconnect, your nick is up for grabs. When a server restarts, you lose your ops. There's no way to prove you're who you say you are. NickServ was a workaround that became permanent — a bot pretending to be an identity system.

## What freeq adds

freeq solves this with AT Protocol identity. Your DID is cryptographically yours. Nobody can impersonate you. Your nick is bound to your identity and persists across reconnections and server restarts.

### For users

- **Sign in with Bluesky** — OAuth in the web client, SASL on the wire. Your handle is your nick.
- **Permanent identity** — Your ops, bans, and channel roles survive reconnection.
- **Rich profiles** — Avatar, display name, and bio from your AT Protocol profile.
- **Modern clients** — Web, desktop (Tauri), iOS, TUI, or any standard IRC client.
- **Media sharing** — Upload images, see inline previews, cross-post to Bluesky.
- **Message editing and deletion** — Fix typos, remove messages.
- **Reactions** — Emoji reactions on messages.
- **Reply threading** — Reply to specific messages with quoted context.
- **Guest mode** — Connect without authentication. No account required.

### For developers

- **Rust SDK** — `(ClientHandle, Receiver<Event>)` pattern. Build bots in 50 lines.
- **Bot framework** — LLM-powered personas, webhook integrations, custom commands.
- **Plugin system** — Extend the server with custom behavior.
- **Verifiable Credentials** — Pluggable identity verification (GitHub, email, anything).
- **Channel governance** — Cryptographic policies with auditable membership.
- **REST API** — Channel history, user info, media upload, policy management.
- **E2EE** — End-to-end encrypted channels and P2P encrypted DMs.
- **Federation** — Server-to-server with CRDT state convergence.

### For the protocol

- **100% backward compatible** — Standard IRC clients connect as guests. No breakage.
- **IRCv3 compliant** — message-tags, server-time, batch, chathistory, echo-message, away-notify.
- **No walled garden** — Protocol is open. Run your own server. Federate with others.
- **Identity is portable** — Your DID works across all freeq servers and the broader AT Protocol ecosystem.

## How it differs from IRC

| Feature | Traditional IRC | freeq |
|---------|----------------|-------|
| Identity | NickServ registration | AT Protocol DID (Bluesky) |
| Nick ownership | Gone on disconnect | Permanent, DID-bound |
| Channel ops | Lost on restart | Persistent via DID |
| Authentication | Password to NickServ | SASL with cryptographic challenge |
| Message history | None (server-side) | CHATHISTORY with database persistence |
| Editing | Not possible | `+draft/edit` tag |
| Reactions | Not possible | TAGMSG with `+react` |
| Media | Paste a URL | Upload, inline preview, lightbox |
| Encryption | Not built in | AES-256-GCM channels + P2P DMs |
| Federation | Server links | CRDT-converged S2S via iroh QUIC |
| Web client | Third-party bridges | Built-in, Slack-class UX |
| Mobile | Third-party apps | Native iOS with full feature parity |

## How it differs from Slack/Discord

| Feature | Slack/Discord | freeq |
|---------|--------------|-------|
| Protocol | Proprietary | Open (IRC + IRCv3) |
| Client lock-in | Must use their app | Any IRC client works |
| Identity | Email/password per service | Your DID, portable everywhere |
| Data ownership | They own it | You run the server |
| Federation | None | Built-in S2S |
| Cost | Free tier → paid | Free, self-hosted |
| Privacy | Full telemetry | Zero tracking |
| Governance | Platform rules | Cryptographic channel policies |
| Extensibility | Limited APIs | Full SDK + plugins + bot framework |
| Open source | No | MIT license |

## The philosophy

freeq treats IRC as **infrastructure**, not a product. The goal is to modernize identity without centralization, UX regressions, or protocol breakage.

- If something works in a standard IRC client, it must keep working.
- Identity comes from the AT Protocol ecosystem, not from freeq.
- Modern features (reactions, editing, threads) are client-side enhancements over standard IRC protocol extensions (TAGMSG, message tags).
- Channel governance is cryptographic and auditable, not based on trust.
- The server is a reference implementation. The protocol is the product.

## Quick start

**Web client** (no install):
Visit [irc.freeq.at](https://irc.freeq.at) and click "Sign in with Bluesky"

**Any IRC client**:
```
Server: irc.freeq.at
Port: 6667 (plain) / 6697 (TLS)
```

**Run your own server**:
```bash
git clone https://github.com/chad/freeq
cd freeq
cargo run --release --bin freeq-server -- \
  --listen-addr 0.0.0.0:6667 \
  --web-addr 0.0.0.0:8080 \
  --db-path freeq.db
```
