# Proposal: Web Infrastructure for irc-reboot

## Summary

irc-reboot should add an optional HTTP/WebSocket layer — not to become a
Discord clone, but to make the server accessible from browsers and HTTP-based
tooling without compromising its IRC-first identity.

This document draws on the Concord project (an open-source IRC-compatible
chat platform with a web UI) as a reference implementation, takes what's
useful from its architecture, and leaves behind the feature bloat.

## Why This Matters

Right now irc-reboot speaks one protocol: IRC over TCP. That's correct for
the core, but it limits who can build on top of it:

- **No browser access.** A web client can't open a raw TCP socket. WebSocket
  is the standard bridge. Without it, the only web option is a bouncer or
  proxy — extra infrastructure that shouldn't be necessary.

- **No REST API for tooling.** Bots, dashboards, monitoring, admin panels,
  mobile apps, and integrations all want HTTP. Fetching channel history,
  looking up a user's DID, checking server health — these are HTTP-shaped
  operations. Forcing everything through IRC PRIVMSG commands is awkward.

- **The SDK already abstracts this.** Our `irc-at-sdk` crate provides a
  `(ClientHandle, Receiver<Event>)` pattern. A WebSocket adapter would
  consume the same event types. The conceptual model already supports
  multiple transports — we just haven't built the second one.

## What We'd Take from Concord

Concord's architecture has a clean separation worth studying:

```
IRC clients ──TCP──▸ ┌──────────────────┐ ◂──WS── Browsers
                     │   IRC Adapter    │
                     ├──────────────────┤
                     │   Chat Engine    │  ← protocol-agnostic
                     ├──────────────────┤
                     │   Web Adapter    │
                     └──────────────────┘
```

The dependency direction is `irc → engine ← web`. The engine never imports
protocol-specific code. Both adapters translate between their wire format and
a shared `ChatEvent` enum.

**Specifically, we'd adopt:**

### 1. Protocol-agnostic engine extraction

Concord's `ChatEngine` struct is the right idea. It holds all chat state
(sessions, channels, members) and exposes methods like `connect()`,
`send_message()`, `join_channel()`, `disconnect()`. Protocol adapters call
into it; it calls back via `mpsc` channels with `ChatEvent` values.

Our `server.rs` currently mixes IRC wire parsing, channel state, and
connection management. Extracting a protocol-agnostic engine would:

- Let the IRC adapter and a future web adapter share state naturally
- Make cross-protocol messaging automatic (IRC user sends a message → web
  users in the same channel see it, and vice versa)
- Simplify testing — engine logic can be tested without TCP connections

This is a refactor of existing code, not new functionality. The IRC adapter
would wrap the engine the same way it wraps `Server` today, just with a
cleaner boundary.

### 2. WebSocket transport

Concord uses axum's built-in WebSocket support. The handler:

1. Upgrades an HTTP connection to WebSocket
2. Authenticates (session cookie or token)
3. Calls `engine.connect()` to get a session ID and event receiver
4. Spawns two tasks: one reads JSON messages from the client and translates
   them to engine calls, the other reads `ChatEvent`s from the engine and
   serializes them as JSON to the client

The JSON wire format is simple — tagged unions:

```json
// Client → Server
{"type": "send_message", "channel": "#general", "content": "hello"}
{"type": "join_channel", "channel": "#dev"}

// Server → Client
{"type": "message", "from": "alice", "target": "#general", "content": "hello", "timestamp": "..."}
{"type": "join", "nickname": "bob", "channel": "#general"}
```

This is minimal. No binary protocol, no custom framing. Any language with a
WebSocket library can connect.

### 3. REST API (read-only + auth)

A small HTTP API for operations that don't need a persistent connection:

| Endpoint | Purpose |
|---|---|
| `GET /api/channels` | List channels |
| `GET /api/channels/{name}/history` | Fetch message history |
| `GET /api/users/{nick}` | User info (DID, handle, online status) |
| `GET /api/health` | Server health check |
| `POST /api/auth/token` | Exchange AT Protocol credentials for a session token |

This is **not** a full CRUD API for managing servers, roles, emoji, etc.
It's infrastructure: enough to bootstrap a connection, fetch context, and
integrate with external systems.

### 4. Static file serving

Concord serves its React frontend from a `static/` directory using
`tower-http`'s `ServeDir`. For irc-reboot, this means the binary can
optionally serve a web UI — a single `--web-static ./static` flag. If the
directory doesn't exist, the HTTP routes still work (API + WebSocket only,
no UI). This keeps the web UI fully decoupled: it's just files on disk,
replaceable, optional.

## What We Would NOT Take from Concord

Concord is a Discord clone that happens to speak IRC. Most of its 40K lines
of Rust and 7.4K lines of TypeScript serve features that don't belong in
irc-reboot:

- **Multi-server/guild model.** Discord's server concept adds a layer of
  indirection (server → channels) that IRC doesn't have. Concord's IRC
  adapter has to do `#server-name/channel` translation. We don't need this.
  irc-reboot is one server. If you want multiple communities, run multiple
  instances.

- **Roles and permission bitfields.** Concord has 20+ permission flags,
  role hierarchies, per-channel overrides. Our channel modes (+o, +v, +b,
  +i, +t, +k) already express IRC's permission model. DID-based identity
  makes bans more durable than anything role-based. We don't need to
  reinvent Discord's permission system.

- **Message editing, deletion, reactions, threads, forums, pins, bookmarks,
  search operators.** These are product features for a chat application.
  irc-reboot is infrastructure. If a web frontend wants to add these, it can
  — but the server shouldn't mandate them.

- **OAuth via GitHub/Google.** Our identity story is AT Protocol. Adding
  GitHub and Google OAuth dilutes the philosophical position. Users
  authenticate with their DID.

- **User profiles, presence, custom status, server discovery, scheduled
  events, announcement channels, templates, AutoMod, audit logs.** All of
  this is application-layer polish that belongs in a product built on top of
  irc-reboot, not in irc-reboot itself.

- **SQLite persistence of everything.** Concord persists messages, users,
  servers, roles, emoji, attachments, audit logs — 11 migration files. We
  should add persistence, but scoped to what IRC needs: channel state,
  message history, DID-nick bindings, bans. Not a full relational model of
  a social platform.

## Proposed Architecture

```
irc-server/
  src/
    engine/           Protocol-agnostic core (extracted from current server.rs)
      mod.rs          ChatEngine struct, ChatEvent enum
      channel.rs      Channel state, modes, bans, history
      session.rs      Connection sessions, DID bindings
    irc/              IRC protocol adapter (current connection.rs + irc.rs)
    web/              NEW — HTTP + WebSocket adapter
      mod.rs
      router.rs       axum routes
      ws.rs           WebSocket connection handler
      api.rs          REST endpoints
    sasl.rs           SASL challenge store (unchanged)
    config.rs         Add --web-addr, --web-static flags
    main.rs           Start both listeners

irc-at-sdk/           Unchanged — client SDK
irc-at-tui/           Unchanged — TUI client
irc-web-ui/           NEW — optional, minimal web client (static files)
```

### New Crate: `irc-web-ui` (optional)

A minimal single-page app. Not React, not a framework — probably vanilla
TypeScript or a small Preact app. Features:

- Connect via WebSocket
- Authenticate with AT Protocol (reuse the same SASL flow over WS, or a
  simplified HTTP-based token exchange)
- Join channels, send/receive messages
- Show DID/handle for authenticated users
- Display rich media from IRCv3 tags (images, links)

This is a **demo**, not a product. It proves the WebSocket transport works
and gives people something to point a browser at. The real value is the API
and WebSocket layer that anyone can build on.

### New Dependencies

| Crate | Purpose | Weight |
|---|---|---|
| `axum` | HTTP framework + WebSocket | Already tokio-native, minimal overhead |
| `tower-http` | CORS, static file serving | Lightweight middleware |
| `serde_json` | JSON serialization (already a workspace dep) | Zero new cost |

No database dependencies in this phase. No OAuth libraries. No JWT.

### Authentication for Web Clients

Web clients authenticate the same way IRC clients do — with AT Protocol
identity. Two options:

**Option A: SASL over WebSocket.** The WebSocket handler implements the same
`ATPROTO-CHALLENGE` flow. The client sends the challenge response as a JSON
message. This is philosophically pure — same auth mechanism, different
transport.

**Option B: HTTP token exchange.** `POST /api/auth/token` accepts an AT
Protocol credential (DPoP-bound access token or app password) and returns a
session token. The WebSocket connection then sends this token on connect.
This is more practical for browser-based OAuth flows.

Either way, the identity primitive remains DID. No GitHub login, no Google
login. If you're on the web, you still prove you own a DID.

## Implementation Plan

### Phase 1: Engine extraction (refactor, no new features)

Pull channel state, session management, and message routing out of
`server.rs` into an `engine/` module. The IRC adapter calls into the engine.
All existing tests continue to pass. No new binary, no new protocol.

**Estimated effort:** Medium. This is reorganizing existing code behind a
clean interface.

### Phase 2: WebSocket + REST (new transport)

Add axum, wire up the WebSocket handler and REST endpoints. Cross-protocol
messaging works: IRC users and WebSocket users share channels.

**Estimated effort:** Medium. The engine does the hard work; the web adapter
is a thin translation layer.

### Phase 3: Minimal web UI (demo)

Build a small static web client. Ship it in an `irc-web-ui/` directory.
Serve it with `--web-static`. This is polish, not infrastructure.

**Estimated effort:** Small. It's a single-page app that talks JSON over a
WebSocket.

### Phase 4: Persistence (separate concern)

Add SQLite for message history, channel state, and DID-nick bindings.
This benefits both IRC and web clients. It's listed here because the web
layer makes it more visible (REST history endpoints need a database), but
it's independently valuable.

**Estimated effort:** Medium. Schema design + migration for a focused set of
tables (channels, messages, bans, identities).

## What This Gets Us

- **Browser access** without a bouncer or proxy
- **Bot and integration ecosystem** via HTTP + WebSocket
- **Cross-protocol messaging** — IRC and web users in the same channels
- **A foundation others can build on** — the web UI is a demo; the API is
  the product
- **No philosophical compromise** — identity is still DID, the protocol is
  still IRC, the web layer is an adapter not an owner

## What This Doesn't Do

- Doesn't turn irc-reboot into a Discord/Slack clone
- Doesn't add features IRC doesn't have (no reactions, no threads, no
  message editing at the protocol level)
- Doesn't require a database (phases 1–3 work in-memory, same as today)
- Doesn't change anything about how IRC clients connect
- Doesn't add non-AT-Protocol authentication

## Open Questions

1. **Should `irc-web-ui` live in this repo or a separate one?** Keeping it
   in-repo makes it easy to ship as part of the binary. Separating it keeps
   the Rust workspace clean. Leaning toward in-repo with a `--features web-ui`
   compile flag.

2. **WebSocket message format: JSON lines or structured envelopes?** JSON
   tagged unions (Concord's approach) are simple and debuggable. Could also
   consider MessagePack for efficiency, but JSON is fine for v1.

3. **Should the web adapter be a separate binary or a feature flag?**
   A feature flag (`--features web`) keeps it optional without requiring
   users to build axum if they don't need it. A separate binary
   (`irc-web-gateway`) is cleaner but adds deployment complexity.

4. **How much of the REST API is read-only?** Fetching history and user info
   is clearly read-only. Channel joins, sends, and topic changes could go
   through WebSocket only (keeping REST simple) or through REST too (more
   flexible). Leaning toward WebSocket for real-time actions, REST for
   queries.
