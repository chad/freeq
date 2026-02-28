# Future Direction

This document is organized into three sections: **immediate gaps** (things that should be fixed before wider use), **pragmatic next steps** (concrete features that extend the existing architecture), and **long-term ideas** (bigger bets that may reshape the project).

See also: `docs/s2s-audit.md` for the full S2S sync architectural audit.

---

## 1. Immediate Gaps & Fixes

### ~~1.1 IRC Spec Compliance~~ ✅ DONE
All P0/P1 IRC commands implemented: LIST, WHO, AWAY, MOTD, +n, +m, SASL abort.

### 1.2 IRCv3 Extensions Still Missing

- **`account-notify` / `account-tag`** — When a user authenticates via SASL, notify other users. Natural for Freeq since DID authentication is a core feature.

- **`away-notify`** — Broadcast AWAY state changes to shared channels. The server already tracks AWAY status; this just adds the broadcast.

- **`extended-join`** — Include account name (DID) in JOIN messages. Trivial with existing infrastructure.

- **`CHATHISTORY` command** — Allow clients to request history on demand, not just on join. The persistence layer already supports pagination. This is the missing piece to make history truly useful.

### 1.3 Remaining AT Protocol Issues

- **DPoP nonce staleness** — The DPoP nonce is captured once and reused. PDS servers rotate nonces. The media upload code has retry logic, but the SASL verification path (`verify_pds_oauth`) doesn't. If the nonce rotates, server-side verification fails.

- **PDS session verification is trust-the-PDS** — The `pds-session` method trusts the PDS to honestly report the DID. A compromised PDS could claim any DID. Crypto verification should be preferred.

- **Only tested against Bluesky PDS** — AT Protocol has other implementations (self-hosted PDS, alternative networks). DID resolution, OAuth endpoints, and PDS APIs may behave differently.

### 1.4 Concurrency & Safety

- **Lock ordering** — `SharedState` uses many `Mutex<T>` fields, and handlers frequently lock multiple mutexes. No documented lock ordering creates deadlock risk. Some handlers acquire `channels` then `connections`, others `nick_to_session` then `channels`. A consistent ordering (or restructuring to reduce lock scope) would prevent subtle deadlocks under load.

---

## 2. Pragmatic Next Steps

### 2.1 S2S Hardening (see docs/s2s-audit.md)

**P2 — S2S authentication:**
Currently any server can join the mesh by knowing an endpoint ID. Add mutual authentication for S2S links. The iroh endpoint provides cryptographic identity (public key); verify against an allowlist.

**P2 — Ban state sync:**
Bans are local-only despite the CRDT having ban support. Wire up S2S messages for ban add/remove, enforce bans on S2S Join.

**P2 — S2S Join enforcement:**
Incoming S2S Joins don't check bans or +i. A user banned on Server 1 can still appear from Server 2.

**P3 — Wire CRDT to live S2S:**
The Automerge CRDT exists and is designed for this problem. The flat-key schema (`founder:{channel}`, `mode:{channel}:t`, etc.) provides convergent merge. Currently two separate state systems (CRDT + ad-hoc JSON messages). Wiring the CRDT to live S2S would solve most split-brain issues permanently.

**P3 — Moderation event log:**
Replace flat ban entries with CRDT-backed moderation log (ULID-keyed events with attribution). Enables auditability, retroactive authority validation, proper conflict resolution for concurrent ban/unban. See `architecture-decisions.md`.

### 2.2 Database Improvements

- **Time-based message retention** — `--message-retention-days` in addition to count-based pruning.

- **Full-text search** — SQLite FTS5 for message search. Enables `/search` in the TUI. Small schema change; query maps to REST endpoint.

### 2.3 Security Hardening

- **Hostname cloaking** — Currently all users show `host` as their hostname. Implement IP-based cloaking or configurable virtual hosts. Expected for any public deployment.

- **Connection limits** — Per-IP connection limits. Rate limiter handles command floods but doesn't prevent thousands of idle connections.

- **TLS client certificates** — Alternative auth path for bots and services.

- **SASL mechanism negotiation** — Advertise supported mechanisms in 908 (RPL_SASLMECHS) so clients know what's available.

### 2.4 Operational Features

- **OPER command** — Server operator status with configurable credentials for remote administration (kill, kline, rehash).

- **Server-level bans (K-line / G-line)** — Ban by IP, DID, or pattern at server level. Stored in DB, enforced on connect.

- **Metrics / monitoring** — Prometheus metrics (connection count, message rate, auth success/failure, S2S link health) via `/metrics` endpoint.

- **Structured logging** — Request IDs that correlate across the SASL flow. Currently tracing is ad-hoc.

- **Graceful shutdown** — Send QUIT to all connected clients and close S2S links cleanly.

### 2.5 TUI Client Improvements

- **Auto-reconnection** — Auto-reconnect with backoff on disconnection. Rejoin channels, re-authenticate. The SDK's `establish_connection()` separation makes this architecturally feasible.

- **P2P auto-discovery** — Use iroh endpoint ID from WHOIS (672) to auto-connect for P2P DMs instead of manual endpoint ID exchange.

- **URL opening** — Keybinding to open URLs from messages in the default browser.

- **Multi-server** — Connect to multiple servers simultaneously, each in their own buffer group. The SDK already supports independent client instances.

---

## 3. Long-Term Ideas

### 3.1 DID-Based Key Exchange for E2EE

Replace passphrase-based E2EE with DID-based key exchange:

1. Each authenticated user has a signing key (from their DID document)
2. Derive per-channel group keys using authenticated key exchange
3. Rotate keys when members join/leave
4. Use the CRDT to sync key rotation events

This gives forward secrecy and verified encryption — you'd know only the authenticated identities can read messages, not just anyone who knows a passphrase.

**Challenges:** Key agreement protocol design, ratcheting, member join/leave rotation, offline members missing rotations.

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

Makes channels discoverable via AT Protocol, enables metadata to follow the social graph, bridges IRC's ephemeral model with AT Protocol's record-oriented model.

### 3.3 Moderation via AT Protocol Labels

Integrate AT Protocol's labeling system:

- Server applies labels to messages or users
- Labels are AT Protocol records, consumable by other services
- Users subscribe to label services for filtering
- Channel moderators issue labels that propagate via AT Protocol

### 3.4 Serverless Mode (Pure P2P) — DEFER

Use iroh + CRDTs to create channels without any server. The server becomes optional infrastructure for discovery and persistence.

**Challenges:** Message ordering, offline delivery, discovery, Sybil resistance.

### 3.5 Bridge to AT Protocol Conversations

Bridge IRC channels to AT Protocol DMs/conversations. Messages in either context appear in both. Identity unified via DID.

### 3.6 Web Client

As described in `proposal-web-infra.md`: separate repo. PWA using WebSocket transport, AT Protocol OAuth, IndexedDB for local history, Service Worker for push.

### 3.7 Bot Framework

The SDK's `(ClientHandle, Receiver<Event>)` pattern is already bot-friendly. Formalize:

- Bot SDK with command parsing, permission checks, persistence
- Standard bots (seen, quote, factoid, RSS)
- AT Protocol integration bots: cross-post to Bluesky, fetch profiles, relay notifications
- Webhook bridge for GitHub/CI
- LLM integration with DID-gated access control

### 3.8 Reputation & Trust

Use the AT Protocol social graph as a trust signal:

- Weight moderation actions by follower count or social distance
- Auto-voice users followed by channel ops
- Relaxed rate limits for verified/trusted identities
- Anti-spam scoring based on account age and social connections

### 3.9 Custom Lexicon Ecosystem

Expand beyond `blue.irc.media`:

- `blue.irc.channel` — Channel metadata records
- `blue.irc.membership` — Channel subscription records
- `blue.irc.reaction` — Persistent reaction records
- `blue.irc.moderation` — Moderation event records
- `blue.irc.identity` — IRC identity binding records

### 3.10 IRCv3 Working Group Proposal

The `ATPROTO-CHALLENGE` SASL mechanism could be proposed as an IRCv3 spec:

1. Formal specification document (RFC-style)
2. Generalize from AT Protocol to any DID-based identity
3. Define mechanism name, challenge format, verification flow
4. Address backward compat, security considerations, deployment
5. Submit to IRCv3 working group

---

## Priority Matrix (Updated)

All original P0/P1 items are **done**. The canonical TODO is in `CLAUDE.md`.
Key addition: **message signing by default** is P0 — all messages from DID-authenticated
users should be cryptographically signed (IRCv3 tag, end-to-end verifiable across S2S).

Here's what remains:

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| **P0** | **Message signing by default** | Large | Critical — trust foundation |
| **P1** | **S2S auth on Kick/Mode/Topic** | Medium | High — security |
| **P1** | **S2S rate limiting** | Medium | High — operational |
| **P2** | **S2S authentication** | Medium | High — security |
| **P2** | **S2S ban sync + enforcement** | Medium | High — moderation |
| **P2** | **Hostname cloaking** | Medium | Medium — privacy |
| **P2** | **account-notify / extended-join** | Small | Medium — client compat |
| **P2** | **away-notify** | Small | Medium — UX |
| **P2** | **CHATHISTORY command** | Medium | High — history UX |
| **P2** | **Connection limits** | Small | Medium — operational |
| **P2** | **TUI auto-reconnection** | Medium | High — reliability |
| **P2** | **OPER command** | Medium | Medium — admin |
| **P2** | **DPoP nonce retry for SASL** | Small | Medium — robustness |
| P3 | Wire CRDT to live S2S | Large | High — correctness |
| P3 | DID-based key exchange for E2EE | Large | High — security |
| P3 | Full-text search | Medium | Medium |
| P3 | Bot framework | Medium | High — ecosystem |
| P3 | AT Protocol record-backed channels | Large | Medium |
| P3 | Reputation & trust via social graph | Large | Medium |
| P3 | Serverless P2P mode | Very Large | High — architectural |
| P3 | IRCv3 WG proposal | Medium | High — ecosystem |
| P3 | Web client | Large | High — adoption |
