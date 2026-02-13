# Future Direction

This document is organized into three sections: **immediate gaps** (things that should be fixed before wider use), **pragmatic next steps** (concrete features that extend the existing architecture), and **long-term ideas** (bigger bets that may reshape the project).

---

## 1. Immediate Gaps & Fixes

### 1.1 IRC Spec Compliance

**Missing commands that real clients expect:**

- **LIST** — Many clients try LIST on connect. Without it, users can't discover channels. Straightforward to implement: iterate `channels` map, return RPL_LIST (322) / RPL_LISTEND (323). Should respect `+s`/`+p` modes once implemented.

- **WHO** — WeeChat, irssi, and others send WHO on channel join to populate user lists. Without WHO, some clients show incomplete member info. Returns RPL_WHOREPLY (352) / RPL_ENDOFWHO (315).

- **AWAY** — Standard feature for any multi-user deployment. RPL_AWAY (301), RPL_NOWAWAY (306), RPL_UNAWAY (305). The away flag should be visible in WHOIS and reflected in NAMES prefixes in clients that support `away-notify`.

- **MOTD** — Not critical but expected. Many bots and clients check for 375/376. A hardcoded or configurable message of the day avoids confusing error messages.

- **Channel mode `+n`** (no external messages) — Currently, anyone can PRIVMSG a channel without joining it. This is a standard mode that most channels expect enabled by default.

- **Channel mode `+m`** (moderated) — Only voiced/op users can speak. Important for moderation.

- **SASL AUTHENTICATE `*`** — The abort mechanism. Some clients send this on timeout. Currently ignored, which means the SASL state machine can get stuck.

### 1.2 IRCv3 Extensions That Practical Clients Need

- **`server-time`** — Critical for history replay. Currently, replayed messages have no timestamps, so they appear as "just sent." Adding `@time=2024-01-15T12:34:56.000Z` to history replay messages is straightforward and makes history usable.

- **`batch`** — Wrapping history replay in a `chathistory` batch prevents clients from treating replay messages as new. Combined with `server-time`, this makes history replay correct.

- **`echo-message`** — Clients that negotiate this expect to see their own messages echoed back. Without it, some clients double-display messages.

- **`multi-prefix`** — Show all prefix characters (`@+`) in NAMES, not just the highest. Trivial to implement, expected by most modern clients.

- **`account-notify` / `account-tag`** — When a user authenticates via SASL, notify other users. This is natural for Freeq since DID authentication is a core feature.

### 1.3 Iroh Usage Issues

- **`std::mem::forget(endpoint)`** — In `server.rs`, the iroh endpoint is kept alive via `mem::forget()`. This prevents clean shutdown and leaks resources. The endpoint should be held in `SharedState` or a dedicated field with proper lifetime management.

- **Secret key file location** — `iroh-key.secret` is created in the current working directory. This should respect `--db-path` or a dedicated `--data-dir` option. The current behavior means the key might end up in unexpected places.

- **No reconnection for S2S peers** — If an S2S link drops, it's gone. There's no reconnection logic. At minimum, a periodic reconnection attempt with exponential backoff is needed for production S2S.

- **DuplexStream bridging** — Both iroh.rs and p2p.rs use `tokio::io::duplex()` to bridge QUIC streams to `AsyncRead + AsyncWrite`. This works but adds an extra copy. Consider implementing `AsyncRead + AsyncWrite` directly on the QUIC stream pair, or use the `WsBridge` pattern consistently (it's already used for iroh connections via `web::WsBridge`).

### 1.4 AT Protocol Integration Issues

- **No OAuth token refresh** — OAuth access tokens expire (typically 1 hour for AT Protocol). When the cached token expires, the user must re-authenticate via browser. Implementing the refresh token flow would make long-running sessions work. The `refresh_jwt` is already stored in `PdsSession` but never used.

- **`urlencod()` only handles ASCII** — The URL encoding function in `oauth.rs` and `pds.rs` casts chars to `u8`, which silently truncates any non-ASCII character. This will corrupt URLs containing Unicode. Should use proper percent-encoding (the `percent-encoding` crate, or a correct manual implementation for multi-byte UTF-8).

- **DPoP nonce staleness** — The DPoP nonce is captured once (during token exchange or probe) and reused. PDS servers rotate nonces. The media upload code has retry logic for this, but the SASL verification path (`verify_pds_oauth`) doesn't. If the nonce has rotated since the client obtained it, the server-side verification will fail.

- **PDS session verification is trust-the-PDS** — The `pds-session` method trusts the PDS to honestly report the DID. A compromised or malicious PDS could claim any DID. This is documented as a design tradeoff, but the crypto verification path should be preferred when possible.

- **Only tested against Bluesky PDS** — The AT Protocol has other implementations (self-hosted PDS, alternative networks). DID resolution, OAuth endpoints, and PDS APIs may behave differently.

### 1.5 Concurrency & Safety

- **Lock ordering** — `SharedState` uses many `Mutex<T>` fields, and handlers frequently lock multiple mutexes. There's no documented lock ordering, creating deadlock risk. Some handlers acquire `channels` then `connections`, others acquire `nick_to_session` then `channels`. A consistent ordering (or restructuring to reduce lock scope) would prevent subtle deadlocks under load.

- **Channels are retained only while they have local members** — When all local users leave a channel, it's removed from the map (`channels.retain(|_, ch| !ch.members.is_empty())`). This loses DID-based ops, founder, bans, and other state. If the channel has remote members (S2S) or should persist (registered channels), this is wrong. Channels should persist while they have any members (local or remote) or have persistent state (DB-backed).

- **No channel name normalization** — Channel names are sometimes lowercased (for ban matching) and sometimes not (for storage keys). This can cause `#Test` and `#test` to be different channels. IRC RFC says channel names are case-insensitive. A normalize-on-input approach would fix this.

---

## 2. Pragmatic Next Steps

### 2.1 Message History Done Right

**Use `server-time` + `batch` for history replay:**

```
@time=2025-01-15T12:00:00Z :server BATCH +hist chathistory #channel
@time=2025-01-15T12:00:01Z;batch=hist :alice!a@host PRIVMSG #channel :hello
@time=2025-01-15T12:00:02Z;batch=hist :bob!b@host PRIVMSG #channel :hi
:server BATCH -hist
```

This single change makes history dramatically more useful. Clients that understand `batch` treat these as historical, not real-time. Clients that don't still see the messages.

**Add `CHATHISTORY` command** (IRCv3 draft): Allow clients to request history on demand, not just on join. The persistence layer already supports pagination (`before` parameter).

### 2.2 Database Improvements

- **Message pruning** — Add a `--max-messages-per-channel` or `--message-retention-days` option. Without this, the database grows unbounded in production. A periodic vacuum task (every N minutes) can trim old messages.

- **Persist DID ops and founder** — Currently only in-memory and CRDT. The CRDT handles S2S convergence, but a server restart loses founder/ops until the next S2S sync. Save `founder_did` and `did_ops` in the `channels` table.

- **Full-text search** — SQLite FTS5 extension for message search. Enables `/search` in the TUI. The schema change is small; the query interface maps to a REST endpoint.

### 2.3 S2S Improvements

- **S2S authentication** — Currently any server can join the mesh by knowing an endpoint ID. Add a shared secret or mutual DID-based authentication for S2S links. The iroh endpoint already provides cryptographic identity (public key); verify it against an allowlist.

- **Auto-reconnection** — When an S2S link drops, attempt to reconnect with exponential backoff. Track link health and emit metrics.

- **Ban state sync** — Bans are currently local-only despite the CRDT having ban support. Wire up S2S messages for ban add/remove, backed by the CRDT.

- **Moderation event log** — As described in `architecture-decisions.md`: replace flat ban entries with a CRDT-backed moderation log (ULID-keyed events with attribution). This enables auditability, retroactive authority validation, and proper conflict resolution for concurrent ban/unban.

### 2.4 Security Hardening

- **Hostname cloaking** — Currently all users show `host` as their hostname. Implement IP-based cloaking or configurable virtual hosts. This is expected for any public deployment.

- **Connection limits** — Per-IP connection limits to prevent abuse. The rate limiter handles command floods but doesn't prevent thousands of idle connections.

- **TLS client certificates** — An alternative authentication path that could complement SASL, particularly for bots and services.

- **SASL mechanism negotiation** — Advertise supported mechanisms in 908 (RPL_SASLMECHS) so clients know what's available before trying.

### 2.5 Operational Features

- **OPER command** — Server operator status with configurable credentials. Needed for remote administration (kill, kline, rehash).

- **Server-level bans (K-line / G-line)** — Ban by IP, DID, or pattern at the server level (not just per-channel). Stored in DB, enforced on connect.

- **Metrics / monitoring** — Expose Prometheus metrics (connection count, message rate, auth success/failure, S2S link health) via a `/metrics` endpoint alongside the REST API.

- **Logging improvements** — Structured logging with request IDs that correlate across the SASL flow. Currently tracing is ad-hoc.

- **Graceful shutdown** — Send QUIT to all connected clients and close S2S links cleanly. Currently, killing the process drops all connections without notice. The `mem::forget(endpoint)` for iroh makes this harder.

### 2.6 TUI Client Improvements

- **Auto-join on invite** — When invited to a channel, prompt or auto-join (configurable).

- **P2P auto-discovery** — Use the iroh endpoint ID from WHOIS (numeric 672) to auto-connect for P2P DMs instead of requiring manual endpoint ID exchange.

- **Image inline rendering** — For terminals that support Kitty or Sixel graphics protocols, render image thumbnails inline. Fall back to the current text representation.

- **URL opening** — Click or keybinding to open URLs from messages in the default browser.

- **Reconnection** — Auto-reconnect with backoff on disconnection. Rejoin channels, re-authenticate. The SDK's `establish_connection()` separation makes this architecturally feasible.

- **Multi-server** — Connect to multiple servers simultaneously, each in their own buffer group. The SDK already supports independent client instances.

---

## 3. Long-Term Ideas

### 3.1 DID-Based Key Exchange for E2EE

Replace passphrase-based E2EE with DID-based key exchange:

1. Each authenticated user has a signing key (from their DID document)
2. Derive per-channel group keys using authenticated key exchange
3. Rotate keys when members join/leave
4. Use the CRDT to sync key rotation events

This would give forward secrecy and verified encryption — you'd know that only the authenticated identities in the channel can read messages, not just anyone who knows a passphrase.

**Challenges:** Key agreement protocol design, ratcheting, member join/leave key rotation, offline members missing rotations.

### 3.2 AT Protocol Record-Backed Channels

Store channel metadata (topic, rules, membership policy) as AT Protocol records:

```json
{
  "$type": "blue.irc.channel",
  "name": "#bluesky-dev",
  "description": "Bluesky development discussion",
  "founder": "did:plc:...",
  "createdAt": "2025-01-15T00:00:00Z"
}
```

This makes channels discoverable via the AT Protocol, enables channel metadata to follow the social graph, and creates a bridge between IRC's ephemeral model and AT Protocol's record-oriented model.

**Extension:** Membership lists as AT Protocol records could enable "follow a channel" semantics where your PDS stores your channel subscriptions.

### 3.3 Moderation via AT Protocol Labels

AT Protocol has a labeling system for content moderation. Integrate it:

- Server can apply labels to messages or users
- Labels are AT Protocol records, so other services can consume them
- Users can subscribe to label services for filtering
- Channel moderators issue labels that propagate via AT Protocol

This extends IRC moderation beyond the single-server model and makes moderation actions portable and auditable across the federation.

### 3.4 Serverless Mode (Pure P2P)  (DEFER NOT IMPORTANT FOR NOW)

Use iroh + CRDTs to create channels without any server:

1. Channel is a topic on a gossip group
2. Each participant maintains a local Automerge document
3. Messages are relayed via iroh gossip (not a dedicated server)
4. CRDTs handle membership, moderation, and message ordering

The server becomes optional infrastructure for discovery and persistence, not a required relay. Users who are both online can communicate directly.

**Challenges:** Message ordering guarantees, offline message delivery, discovery without a server, Sybil resistance.

### 3.5 Bridge to AT Protocol Conversations

AT Protocol is building direct messaging. Bridge IRC channels to AT Protocol conversations:

- Messages sent in IRC appear in the AT Protocol conversation
- Messages sent via AT Protocol appear in the IRC channel
- Identity is unified (same DID in both)
- Media shared in either context is available in both

This positions IRC as the real-time transport and AT Protocol as the persistent record layer.

### 3.6 Web Client

As stated in `proposal-web-infra.md`: the web client is a separate repo. But some directions:

- **Progressive Web App** using the WebSocket transport
- **AT Protocol OAuth** for authentication (same flow as the TUI client, but in-browser)
- **IndexedDB** for local message history
- **Service Worker** for push notifications

The REST API provides the read-only data; the WebSocket provides the real-time stream. No new server-side protocol needed.

### 3.7 Bot Framework (HIGH PRIORITY AFTER TABLE STAKES)

The SDK's `(ClientHandle, Receiver<Event>)` pattern is already bot-friendly. Formalize this:

- **Bot SDK** with command parsing, permission checks, persistence
- **Standard bot commands** (seen, quote, factoid, karma, RSS)
- **AT Protocol integration** bots: cross-post to Bluesky, fetch profiles, relay notifications
- **Webhook bridge** for GitHub/GitLab/CI notifications
- **LLM integration** with DID-gated access control

### 3.8 Reputation & Trust (HIGH PRIORITY AFTER TABLE STAKES)

Use the AT Protocol social graph as a trust signal:

- Weight moderation actions by follower count or social distance
- Auto-voice users who are followed by channel ops
- Rate limits that relax for verified/trusted identities
- Anti-spam scoring based on account age and social connections

This is the "Option C" from `architecture-decisions.md` — reputation-weighted moderation.

### 3.9 Custom Lexicon Ecosystem

The `blue.irc.media` lexicon is a start. Expand:

- `blue.irc.channel` — Channel metadata records
- `blue.irc.membership` — Channel subscription records
- `blue.irc.reaction` — Persistent reaction records
- `blue.irc.moderation` — Moderation event records
- `blue.irc.identity` — IRC identity binding records (handle → nick preferences)

Each lexicon becomes a building block that other AT Protocol applications can consume, extending IRC's reach beyond the chat context.

### 3.10 IRCv3 Working Group Proposal

The `ATPROTO-CHALLENGE` SASL mechanism could be proposed as an IRCv3 specification:

1. Write a formal specification document (RFC-style)
2. Generalize from AT Protocol to any DID-based identity system
3. Define the mechanism name, challenge format, and verification flow
4. Address backward compatibility, security considerations, and deployment
5. Submit to the IRCv3 working group for review

This would position Freeq as a reference implementation for a standards-track specification, increasing its relevance and adoption potential.

---

## Priority Matrix

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| P0 | Fix `mem::forget(endpoint)` | Small | Correctness |
| P0 | Fix `urlencod()` Unicode bug | Small | Correctness |
| P0 | Add channel name normalization | Small | Correctness |
| P0 | Fix channel retention (preserve remote members + persistent state) | Small | Correctness |
| P1 | Add `server-time` + `batch` to history replay | Medium | High — makes history usable |
| P1 | Add LIST, WHO, AWAY commands | Medium | High — client compatibility |
| P1 | Add MOTD | Small | Medium — expected by clients |
| P1 | Add `+n` and `+m` channel modes | Small | Medium — moderation |
| P1 | S2S auto-reconnection | Medium | High — production reliability |
| P1 | OAuth token refresh | Medium | High — session longevity |
| P1 | Message pruning | Small | Medium — operational |
| P2 | S2S authentication | Medium | High — security |
| P2 | Hostname cloaking | Medium | Medium — privacy |
| P2 | Persist DID ops + founder | Small | Medium |
| P2 | SASL abort (`*`) handling | Small | Low |
| P2 | Multi-prefix, echo-message | Small | Medium — client compat |
| P3 | DID-based key exchange for E2EE | Large | High — security |
| P3 | AT Protocol record-backed channels | Large | Medium |
| P3 | Serverless P2P mode | Very Large | High — architectural |
| P3 | IRCv3 WG proposal | Medium | High — ecosystem |
