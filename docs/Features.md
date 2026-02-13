# Freeq Feature List

This document catalogs every feature implemented in Freeq, organized by category. Features unique to Freeq (not present in classic IRC) are marked with **🆕**. Features that extend or modify standard IRC behavior are marked with **🔧**. Standard IRC features are unmarked.

---

## 1. IRC Protocol — Core

### Connection & Registration

| Feature | Status | Notes |
|---------|--------|-------|
| NICK / USER registration | ✅ | Standard IRC registration flow |
| PING / PONG keepalive | ✅ | Both client→server and server→client |
| QUIT with reason broadcast | ✅ | Broadcasts to all shared channels |
| Connection timeout detection | ✅ | 90s ping interval, 180s timeout |
| Rate limiting (token bucket) | ✅ | 10 cmd/sec; exempt during registration |
| ERR_UNKNOWNCOMMAND (421) | ✅ | For unrecognized commands |

### Channels

| Feature | Status | Notes |
|---------|--------|-------|
| JOIN (single and multi-channel) | ✅ | `JOIN #a,#b` with per-channel keys |
| PART (single and multi-channel) | ✅ | |
| PRIVMSG to channels | ✅ | |
| PRIVMSG to users (PM) | ✅ | |
| NOTICE to channels and users | ✅ | |
| CTCP ACTION (`/me`) | ✅ | Via `\x01ACTION ...\x01` |
| TOPIC query and set | ✅ | RPL_TOPIC (332), RPL_TOPICWHOTIME (333), RPL_NOTOPIC (331) |
| NAMES (353/366) | ✅ | With `@` and `+` prefixes for ops/voiced |
| KICK | ✅ | With reason, proper numeric errors |
| INVITE | ✅ | RPL_INVITING (341), notifies target |

### Channel Modes

| Mode | Status | Notes |
|------|--------|-------|
| `+o` / `-o` (channel operator) | ✅ | |
| `+v` / `-v` (voice) | ✅ | |
| `+b` / `-b` (ban) | ✅ | Hostmask wildcard matching |
| `+i` / `-i` (invite-only) | ✅ | |
| `+t` / `-t` (topic lock) | ✅ | Only ops can set topic when enabled |
| `+k` / `-k` (channel key) | ✅ | Password required to join |
| MODE query (324) | ✅ | Lists current channel modes |
| Ban list query (`+b` no arg) | ✅ | RPL_BANLIST (367), RPL_ENDOFBANLIST (368) |

### User Modes

| Feature | Status | Notes |
|---------|--------|-------|
| User mode query (221) | ✅ | Returns `+` (no user modes implemented) |

### WHOIS

| Feature | Status | Notes |
|---------|--------|-------|
| RPL_WHOISUSER (311) | ✅ | |
| RPL_WHOISSERVER (312) | ✅ | |
| RPL_ENDOFWHOIS (318) | ✅ | |
| RPL_WHOISACCOUNT (330) | 🆕 | Shows authenticated DID |
| Custom 671: AT Protocol handle | 🆕 | Shows resolved Bluesky handle |
| Custom 672: iroh endpoint | 🆕 | Shows P2P iroh endpoint ID |
| RPL_WHOISCHANNELS (319) | ✅ | For remote S2S users |

### Missing Standard IRC Commands

| Feature | Status | Notes |
|---------|--------|-------|
| LIST | ❌ | Not implemented |
| WHO | ❌ | Not implemented |
| AWAY | ❌ | Not implemented |
| MOTD (375/372/376) | ❌ | Not implemented |
| OPER (server operator) | ❌ | Not implemented |
| WALLOPS | ❌ | Not implemented |
| LUSERS | ❌ | Not implemented |
| USERHOST | ❌ | Not implemented |
| ISON | ❌ | Not implemented |
| ADMIN | ❌ | Not implemented |
| INFO | ❌ | Not implemented |
| LINKS | ❌ | Not implemented |
| STATS | ❌ | Not implemented |
| TIME | ❌ | Not implemented |
| VERSION | ❌ | Not implemented |
| Channel modes: `+m` (moderated) | ❌ | Not implemented |
| Channel modes: `+n` (no external) | ❌ | Not implemented (external msgs always allowed) |
| Channel modes: `+s` / `+p` (secret/private) | ❌ | Not implemented |
| Channel modes: `+l` (user limit) | ❌ | Not implemented |
| SASL AUTHENTICATE `*` (abort) | ❌ | Not handled |
| Hostname cloaking | ❌ | |
| Reverse DNS lookup | ❌ | |
| K-line / G-line (server bans) | ❌ | |
| Nick change broadcast | Partial | Local clients see NICK change; S2S broadcasts NICK but remote_members map isn't fully updated |
| NICK change after registration | Partial | Nick→session mapping updates but JOIN/channel membership doesn't re-broadcast |

---

## 2. IRCv3 Capabilities

| Feature | Status | Notes |
|---------|--------|-------|
| CAP LS / REQ / ACK / NAK / END | ✅ | IRCv3 capability negotiation |
| `sasl` capability | ✅ | With ATPROTO-CHALLENGE mechanism |
| `message-tags` capability | ✅ | Tag-aware routing per client |
| TAGMSG (tags-only messages) | ✅ | With fallback for plain clients |
| `iroh=<id>` CAP advertisement | 🆕 | Transport discovery via CAP LS |

### Missing IRCv3 Extensions

| Feature | Status | Notes |
|---------|--------|-------|
| `multi-prefix` | ❌ | |
| `away-notify` | ❌ | |
| `account-notify` | ❌ | |
| `account-tag` | ❌ | |
| `batch` | ❌ | History replay doesn't use batch |
| `echo-message` | ❌ | |
| `server-time` | ❌ | History replay has no timestamps |
| `labeled-response` | ❌ | |
| `invite-notify` | ❌ | |
| `chghost` | ❌ | |
| `cap-notify` | ❌ | |
| `extended-join` | ❌ | |
| `setname` | ❌ | |
| `standard-replies` | ❌ | |
| `msgid` (message IDs) | ❌ | |

---

## 3. Authentication — SASL ATPROTO-CHALLENGE 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| Challenge-response SASL flow | ✅ | Custom `ATPROTO-CHALLENGE` mechanism |
| Cryptographically random nonce (32 bytes) | ✅ | Per challenge |
| Challenge single-use enforcement | ✅ | Consumed on take, replay blocked |
| Configurable challenge timeout | ✅ | Default 60s, `--challenge-timeout-secs` |
| JSON-encoded challenges | ✅ | Deviation from binary: for debuggability |
| RPL_LOGGEDIN (900) | ✅ | |
| RPL_SASLSUCCESS (903) | ✅ | |
| ERR_SASLFAIL (904) | ✅ | |
| Guest fallback (no SASL) | ✅ | Standard IRC clients work unmodified |

### Verification Methods

| Method | Status | Notes |
|--------|--------|-------|
| `crypto` (DID document key signature) | ✅ | Signs raw challenge bytes |
| `pds-session` (app password Bearer JWT) | ✅ | Verifies via PDS `getSession` |
| `pds-oauth` (DPoP-bound access token) | ✅ | DPoP proof forwarded to PDS |

### Key Types

| Key Type | Status | Notes |
|----------|--------|-------|
| secp256k1 | ✅ | MUST per spec — compressed SEC1 encoding |
| ed25519 | ✅ | SHOULD per spec |
| Multibase/multicodec parsing | ✅ | `z` prefix (base58btc), proper varint codecs |

### DID Resolution

| Feature | Status | Notes |
|---------|--------|-------|
| `did:plc` resolution (plc.directory) | ✅ | |
| `did:web` resolution | ✅ | Including path-based DIDs |
| Handle resolution (`.well-known/atproto-did`) | ✅ | |
| PDS endpoint extraction from DID doc | ✅ | `AtprotoPersonalDataServer` service type |
| PDS URL verification (claimed vs doc) | ✅ | Prevents spoofing |
| Authentication key extraction | ✅ | From `authentication` + `assertionMethod` |
| Static resolver (testing) | ✅ | In-memory DID document map |

---

## 4. DID-Aware IRC Features 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| DID-based bans (`MODE +b did:plc:xyz`) | ✅ | Identity-based, survives nick changes |
| DID-based invites | ✅ | Stored by DID, survive reconnect |
| Nick ownership (DID binding) | ✅ | Persisted across restarts |
| Nick enforcement at registration | ✅ | Non-owners renamed to `GuestXXXX` |
| Persistent DID-based channel ops | ✅ | Auto-op on rejoin by DID |
| Channel founder (first authenticated user) | ✅ | Can't be de-opped |
| DID in WHOIS output | ✅ | Numeric 330 |
| AT handle in WHOIS output | ✅ | Resolved asynchronously from DID doc |

---

## 5. Transport Stack

### TCP / TLS (Standard)

| Feature | Status | Notes |
|---------|--------|-------|
| Plain TCP (port 6667) | ✅ | |
| TLS (port 6697) | ✅ | rustls with configurable cert/key |
| Auto-detect TLS by port (client) | ✅ | Port 6697 → TLS |
| Self-signed cert support (client) | ✅ | `--tls-insecure` flag |

### WebSocket 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| WebSocket IRC transport (`/irc`) | ✅ | IRC-over-WS, not a new protocol |
| Text frame ↔ IRC line bridging | ✅ | One line per frame, `\r\n` handling |
| `--web-addr` opt-in | ✅ | Zero-cost when disabled |
| HTML test client | ✅ | `test-client.html` |

### Iroh QUIC Transport 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| Iroh endpoint for IRC connections | ✅ | ALPN: `freeq/iroh/1` |
| Persistent secret key (`iroh-key.secret`) | ✅ | Stable endpoint ID across restarts |
| NAT hole-punching + relay fallback | ✅ | Via iroh's infrastructure |
| Transport-agnostic handler | ✅ | All transports → `handle_generic()` |
| Iroh ID in CAP LS for auto-discovery | ✅ | `iroh=<endpoint-id>` |
| Client auto-upgrade to iroh | ✅ | Probes CAP LS, reconnects via iroh |
| Configurable iroh UDP port | ✅ | `--iroh-port` |

---

## 6. End-to-End Encryption (E2EE) 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| AES-256-GCM channel encryption | ✅ | Per-channel passphrase |
| HKDF-SHA256 key derivation | ✅ | Channel-name-salted |
| Wire format: `ENC1:<nonce>:<ciphertext>` | ✅ | Version-tagged, base64 encoded |
| Server-transparent relay | ✅ | Server sees ciphertext only |
| `/encrypt` and `/decrypt` commands | ✅ | TUI commands |
| Unicode passphrase support | ✅ | |
| Tamper detection (GCM auth tag) | ✅ | |

---

## 7. Peer-to-Peer Encrypted DMs 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| Client-side iroh endpoint for DMs | ✅ | ALPN: `freeq/p2p-dm/1` |
| Direct encrypted QUIC connections | ✅ | Server-free |
| `/p2p start/id/connect/msg` commands | ✅ | TUI commands |
| Newline-delimited JSON wire format | ✅ | Not IRC protocol |
| Dedicated `p2p:<id>` TUI buffers | ✅ | |
| Iroh endpoint ID in WHOIS (672) | ✅ | For peer discovery |

---

## 8. Server-to-Server Federation (S2S) 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| Iroh QUIC-based S2S links | ✅ | ALPN: `freeq/s2s/1` |
| `--s2s-peers` CLI option | ✅ | Connect to peers on startup |
| Incoming S2S acceptance (when iroh enabled) | ✅ | |
| ALPN-based routing (client vs S2S) | ✅ | |
| Origin tracking (loop prevention) | ✅ | `origin` field in S2S messages |
| Newline-delimited JSON S2S protocol | ✅ | |

### What Syncs

| Feature | Status | Notes |
|---------|--------|-------|
| PRIVMSG relay | ✅ | Channel messages |
| JOIN / PART / QUIT propagation | ✅ | Membership tracking per origin server |
| NICK change propagation | ✅ | |
| TOPIC sync | ✅ | |
| Remote member tracking | ✅ | `remote_members` with DID + handle |
| SyncRequest / SyncResponse | ✅ | Initial state exchange |
| NAMES includes remote members | ✅ | With DID-based op prefixes |
| WHOIS for remote users | ✅ | Shows DID, handle, origin |
| DID-based ops sync | ✅ | Union merge |
| Founder sync (first-write-wins) | ✅ | No timestamp dependency |

### CRDT State Layer (Automerge)

| Feature | Status | Notes |
|---------|--------|-------|
| Flat-key Automerge document | ✅ | Avoids nested-map conflicts |
| Channel membership CRDT | ✅ | `member:{channel}:{nick}` |
| Topic CRDT (LWW) | ✅ | |
| Ban CRDT (add/remove) | ✅ | |
| Nick ownership CRDT | ✅ | |
| Founder CRDT (first-write-wins) | ✅ | Conditional put, deterministic convergence |
| DID ops CRDT (grant/revoke) | ✅ | |
| Sync message generation/receipt | ✅ | Automerge sync protocol |
| Save/load from bytes | ✅ | |

### S2S Limitations

| Limitation | Notes |
|------------|-------|
| No S2S authentication | Any server can join the mesh |
| No partition healing | Lost peers don't auto-reconnect |
| No conflict notification to users | Silent merge |
| Rogue server can add `did_ops` | Authorization-on-write not implemented |
| S2S doesn't sync ban state | Only local bans enforced |

---

## 9. Persistence (SQLite)

| Feature | Status | Notes |
|---------|--------|-------|
| `--db-path` opt-in | ✅ | In-memory by default |
| WAL mode | ✅ | Good concurrent read performance |
| Message history storage | ✅ | All channel messages |
| Channel state persistence | ✅ | Topics, modes (+t/+i/+k), keys |
| Ban persistence | ✅ | Hostmask and DID bans |
| DID-nick identity bindings | ✅ | Survive restarts |
| History replay on JOIN | ✅ | Last 100 messages, with tag support |
| Graceful persistence failures | ✅ | Logged, don't crash server |
| Load persisted state on startup | ✅ | Channels, bans, messages, identities |

### Persistence Gaps

| Gap | Notes |
|-----|-------|
| No message pruning/rotation | Database grows unbounded |
| No DID-based ops persistence | Only in-memory + CRDT |
| No founder persistence in DB | Only in-memory + CRDT |
| History replay lacks timestamps | Messages appear as "just sent" |

---

## 10. REST API 🆕

| Endpoint | Status | Notes |
|----------|--------|-------|
| `GET /api/v1/health` | ✅ | Server stats |
| `GET /api/v1/channels` | ✅ | List all channels |
| `GET /api/v1/channels/{name}/history` | ✅ | Paginated, `?limit=N&before=T` |
| `GET /api/v1/channels/{name}/topic` | ✅ | |
| `GET /api/v1/users/{nick}` | ✅ | Online status, DID, handle |
| `GET /api/v1/users/{nick}/whois` | ✅ | + channels |
| CORS support | ✅ | Permissive by default |
| Read-only (no write endpoints) | ✅ | By design |

---

## 11. Rich Media (IRCv3 Tags) 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| Media attachment tags | ✅ | `content-type`, `media-url`, `media-alt`, etc. |
| Multipart/alternative semantics | ✅ | Tags for rich clients, body for plain |
| Link preview tags | ✅ | `text/x-link-preview` content type |
| Reaction tags (`+react`) | ✅ | With TAGMSG, fallback ACTION for plain clients |
| Media upload to AT Protocol PDS | ✅ | Blob upload + record pinning |
| `blue.irc.media` custom lexicon | ✅ | Prevents blob GC, doesn't pollute feed |
| Optional cross-post to Bluesky feed | ✅ | |
| OpenGraph link preview fetching | ✅ | HTML parsing, 64KB limit |
| CDN URL generation (bsky.app) | ✅ | |
| DPoP nonce retry for PDS uploads | ✅ | Up to 3 attempts |
| Tag escaping/unescaping (IRCv3 spec) | ✅ | `\:`, `\s`, `\\`, `\r`, `\n` |

---

## 12. OAuth 2.0 (AT Protocol) 🆕

| Feature | Status | Notes |
|---------|--------|-------|
| Browser-based OAuth login | ✅ | Opens system browser |
| Authorization server discovery | ✅ | Protected resource metadata → AS metadata |
| Pushed Authorization Request (PAR) | ✅ | Required by Bluesky |
| PKCE (S256) | ✅ | |
| DPoP key generation (P-256 / ES256) | ✅ | |
| DPoP proof creation (RFC 9449) | ✅ | With `ath` claim |
| DPoP nonce discovery and retry | ✅ | |
| Token exchange | ✅ | |
| Session caching to disk | ✅ | `~/.config/freeq-tui/<handle>.session.json` |
| Cached session validation | ✅ | Probes PDS on reuse |
| Restrictive file permissions (0600) | ✅ | |
| Handle → DID → PDS resolution | ✅ | |

### OAuth Gaps

| Gap | Notes |
|-----|-------|
| No token refresh | Re-auth via browser when expired |
| No PKCE state persistence across crashes | |

---

## 13. TUI Client

### Buffers & Navigation

| Feature | Status | Notes |
|---------|--------|-------|
| Multi-buffer UI (status + channels + PMs) | ✅ | |
| Buffer switching (Ctrl-N/P, Alt-N/P, Shift-Tab) | ✅ | |
| Auto-buffer creation on JOIN/PM | ✅ | |
| P2P DM dedicated buffers | ✅ | `p2p:<short-id>` |
| Unread indicator (●) | ✅ | |
| PageUp/PageDown scroll | ✅ | |
| Channel member list in buffer | ✅ | |

### Input Editing

| Feature | Status | Notes |
|---------|--------|-------|
| Emacs keybindings (default) | ✅ | Full readline-style |
| Vi mode (`--vi`) | ✅ | Normal + Insert modes |
| Kill ring (Ctrl-K/U/W/Y) | ✅ | |
| Word motion (Alt-F/B/D) | ✅ | |
| Case transforms (Alt-U/L/C) | ✅ | |
| Transpose (Ctrl-T) | ✅ | |
| Tab nick completion | ✅ | |
| Input history (Up/Down) | ✅ | |

### Display

| Feature | Status | Notes |
|---------|--------|-------|
| Status bar (transport, nick, auth, uptime) | ✅ | |
| Transport badge (color-coded) | ✅ | Red=TCP, Green=TLS, Cyan=WS, Magenta=Iroh |
| Network info popup (`/net`) | ✅ | |
| Debug mode (`/debug`) | ✅ | Raw IRC lines |
| Rich media display (🖼 badge) | ✅ | Image/video/audio formatting |
| E2EE status display | ✅ | 🔒 prefix on encrypted channels |

### Commands (37 total)

`/join`, `/part`, `/msg`, `/me`, `/topic`, `/mode`, `/op`, `/deop`, `/voice`, `/kick`, `/ban`, `/unban`, `/invite`, `/whois`, `/names`, `/raw`, `/encrypt`, `/decrypt`, `/p2p start`, `/p2p id`, `/p2p connect`, `/p2p msg`, `/net`, `/debug`, `/quit`, `/help`, plus MODE variants (+o/-o, +v/-v, +b/-b, +i/-i, +t/-t, +k/-k).

---

## 14. SDK

| Feature | Status | Notes |
|---------|--------|-------|
| `(ClientHandle, Receiver<Event>)` pattern | ✅ | Any UI/bot can consume |
| Pluggable `ChallengeSigner` trait | ✅ | KeySigner, PdsSessionSigner, StubSigner |
| `establish_connection()` pre-TUI | ✅ | Connection errors before UI starts |
| Iroh auto-discovery (`discover_iroh_id`) | ✅ | Probe CAP LS for iroh upgrade |
| Tagged message sending | ✅ | `send_tagged`, `send_media`, `send_reaction` |
| P2P DM subsystem | ✅ | Full lifecycle management |
| E2EE encrypt/decrypt | ✅ | Library functions |
| DID resolution | ✅ | HTTP and static resolvers |
| Crypto key generation and signing | ✅ | secp256k1 + ed25519 |
| PDS client (create session, verify) | ✅ | |
| Bluesky profile fetching | ✅ | Public API, no auth needed |
| Media upload to PDS | ✅ | With DPoP retry |
| Link preview fetching | ✅ | OpenGraph parsing |
| IRC message parser with tag support | ✅ | |

---

## 15. Testing

| Category | Count | Notes |
|----------|-------|-------|
| SDK unit tests | 35 | IRC parsing, crypto, DID, media, auth |
| Server unit tests | 33 | Parsing, SASL, channel state, DB, CRDT |
| Integration tests | 27 | End-to-end auth flows, channel ops, persistence |
| S2S acceptance tests | 9 | Requires two live servers |
| **Total** | **104** | |

---

## 16. Configuration

| Option | Default | Notes |
|--------|---------|-------|
| `--listen-addr` | `127.0.0.1:6667` | Plain TCP |
| `--tls-listen-addr` | `127.0.0.1:6697` | TLS |
| `--tls-cert` / `--tls-key` | None | Enables TLS |
| `--server-name` | `freeq` | |
| `--challenge-timeout-secs` | `60` | |
| `--db-path` | None (in-memory) | |
| `--web-addr` | None | Enables HTTP/WS |
| `--iroh` | false | Enables iroh |
| `--iroh-port` | random | |
| `--s2s-peers` | empty | Comma-separated |
