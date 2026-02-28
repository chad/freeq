# freeq Encryption & Security

> **Goal**: Everything encrypted by default. This document maps every data path in freeq, what's protected today, and what's not yet.

## Overview

freeq has encryption at multiple layers — transport, authentication, federation, and (planned) message-level. Some paths are fully encrypted today. Others have gaps. We're transparent about both.

---

## The Scorecard

| Data Path | Encrypted? | Mechanism | Notes |
|-----------|-----------|-----------|-------|
| **Web client ↔ Server** | ✅ Yes | TLS 1.3 (HTTPS/WSS) | nginx terminates TLS with Let's Encrypt cert |
| **iOS app ↔ Server** | ✅ Yes | TLS 1.3 (WSS) | App Transport Security enforces HTTPS |
| **IRC client ↔ Server (TLS)** | ✅ Yes | TLS 1.3 (port 6697) | rustls with Let's Encrypt cert |
| **IRC client ↔ Server (plain)** | ❌ No | Plaintext TCP (port 6667) | Legacy IRC compat; should use TLS port |
| **Server ↔ Auth Broker** | ✅ Yes | HTTPS + HMAC-SHA256 | All broker API calls over TLS; request bodies signed with shared secret |
| **Auth Broker ↔ Bluesky PDS** | ✅ Yes | HTTPS + OAuth 2.0 + DPoP | Token-bound proof-of-possession; PDS credentials never leave the broker |
| **Server ↔ Server (S2S)** | ✅ Yes | QUIC (iroh) | iroh uses Noise protocol over QUIC; peer identity = Ed25519 public key |
| **Server ↔ SQLite (at rest)** | ✅ Yes | AES-256-GCM per message | Key derived from server signing key via HMAC; backward-compatible with legacy plaintext |
| **Server ↔ Policy DB (at rest)** | ❌ No | Plaintext on disk | Channel policies, credentials |
| **Message content (in transit)** | 🟡 Transport only | TLS protects the pipe, not the payload | Server sees plaintext; E2E DMs available |
| **Message content (at rest)** | ✅ Yes | AES-256-GCM (EAR1: prefix) | New messages encrypted; old messages readable as-is |
| **Message signatures** | ✅ Yes (client + server) | ed25519 via `+freeq.at/sig` IRCv3 tag | Client-side signing with session keys; server fallback for legacy clients |
| **DM content** | 🟡 E2E available | Double Ratchet (X3DH + AES-256-GCM) | E2EE auto-enabled between DID-authenticated users; server sees ciphertext |
| **File uploads (in transit)** | ✅ Yes | HTTPS to server → HTTPS to PDS | Uploaded via TLS to server, proxied via TLS to AT Protocol PDS |
| **File uploads (at rest)** | 🟡 PDS-dependent | Stored on user's PDS (Bluesky infra) | Not under freeq's control; PDS may or may not encrypt at rest |
| **Authentication challenge** | ✅ Yes | Cryptographic challenge-response | Server issues nonce → client signs with DID key → server verifies |
| **OAuth tokens** | ✅ Yes | In-memory only, TLS transport | Never written to disk; lost on server restart |
| **Broker tokens** | ✅ Yes | HMAC-signed, TLS transport | Short-lived; broker refreshes PDS tokens on demand |
| **Verifier signing key** | 🟡 Partial | Persisted to disk as plaintext file | `verifier-signing-key.secret`; filesystem permissions are the only protection |
| **Hostname/IP** | ✅ Cloaked | `freeq/plc/xxxxxxxx` format | Real IP never exposed to other users |

---

## What's Encrypted Today

### Transport Layer (Client ↔ Server)

Every production connection is TLS-encrypted:

- **Web/iOS**: Connect via `wss://irc.freeq.at` — nginx terminates TLS 1.3 with a Let's Encrypt certificate, proxies to the local HTTP server.
- **IRC over TLS**: Port 6697 uses rustls with the same Let's Encrypt cert. Direct TLS termination, no proxy.
- **Plain IRC**: Port 6667 exists for legacy compatibility. **This is the one unencrypted client path.** We recommend TLS for all connections.

### Authentication (SASL ATPROTO-CHALLENGE)

The authentication flow is cryptographically sound:

1. Server generates a random challenge (session-bound nonce + timestamp)
2. Client signs the challenge with their DID's private key (secp256k1 or ed25519)
3. Server resolves the DID document, extracts the public key, verifies the signature
4. **Private keys never leave the client device**
5. Challenge expires after 60 seconds and is invalidated after use (no replay)

### Auth Broker Communication

The auth broker (handles OAuth with Bluesky PDS) is secured at multiple levels:

- All communication over HTTPS
- Every request body is HMAC-SHA256 signed with a shared secret
- Server verifies the signature before processing any broker message
- OAuth tokens use DPoP (Demonstrating Proof of Possession) — tokens are bound to the client's key pair
- PDS credentials are held in-memory on the broker only; never sent to the IRC server

### Server-to-Server Federation

S2S federation uses [iroh](https://iroh.computer/), which provides:

- **QUIC transport**: All data encrypted in transit
- **Noise protocol**: Mutual authentication via Ed25519 keypairs
- **Peer identity**: Each server has a stable Ed25519 identity derived from a persistent key
- **NAT traversal**: Works across NATs without exposing plain ports

### Hostname Cloaking

User IPs are never visible to other users:

- DID-authenticated users: `freeq/plc/xxxxxxxx` (8-char hash of DID)
- Guest users: `freeq/guest`
- The server knows the real IP (for rate limiting), but it's never broadcast

---

## What's NOT Encrypted (Yet)

### 1. Message Content — Server Sees Everything

**This is the biggest gap.**

freeq currently operates like Slack, Discord, and every other centralized chat: the server can read all messages. Transport encryption (TLS) protects messages from network observers, but the server itself has full access.

This matters because:
- A compromised server leaks all history
- The server operator can read DMs
- Law enforcement requests to the server operator expose content

### 2. Data at Rest (Partial)

Message content is now encrypted at rest using AES-256-GCM. Each message is individually encrypted before SQLite storage, with a key derived from the server's signing key via HMAC-SHA256. Legacy messages (stored before encryption was enabled) remain readable as plaintext.

**What's encrypted**: Message text in the `messages` table (PRIVMSG, NOTICE, edits).

**What's NOT encrypted**: Channel metadata, policies, identities, sender nicks, timestamps. A compromised disk still reveals who talked to whom and when — but not what they said.

### 3. Message Signatures (Partial)

Messages are now signed with server-attested ed25519 signatures. Every PRIVMSG/NOTICE from a DID-authenticated user carries a `+freeq.at/sig` tag containing a base64url-encoded signature over `{sender_did}\0{target}\0{text}\0{timestamp}`. The server's signing public key is published at `/api/v1/signing-key`.

**What this provides:**
- Federated servers can verify message provenance
- Signed messages are distinguishable from unsigned (guest) messages
- Signatures survive S2S relay

**What this does NOT provide (yet):**
- The server could still theoretically forge signatures (it holds the signing key)
- True end-to-end non-repudiation requires client-side signing (Phase 2)

### 4. File Uploads

Uploaded media lives on the user's AT Protocol PDS (typically Bluesky infrastructure). freeq doesn't control PDS encryption policies. The server proxies uploads over TLS, but the PDS storage is opaque to us.

### 5. Verifier Signing Key

The credential verifier's signing key is stored as a plaintext file on disk. It should be in a hardware security module (HSM) or at minimum an encrypted keystore.

---

## Roadmap

### Phase 1 + 1.5: Message Signing (P0) ✅ SHIPPED

**Status**: Implemented (client-side + server fallback)

Every message from a DID-authenticated user is cryptographically signed:

```
@+freeq.at/sig=<base64url-signature> PRIVMSG #channel :Hello world
```

- **Signed data**: `{target}\0{text}\0{timestamp}` (canonical form)
- **Key types**: secp256k1 (required), ed25519 (recommended)
- **Verification**: Anyone can verify against the sender's DID document
- **Scope**: PRIVMSG, NOTICE, TOPIC, KICK
- **Guest messages**: Unsigned — clearly distinguishable from verified messages

**Client-side signing (Phase 1.5)** is now shipped. Clients (SDK, web, iOS) generate a per-session ed25519 keypair, register the public key with the server via `MSGSIG`, and sign every outgoing PRIVMSG. The server verifies the client's signature and relays it unchanged — the server **cannot forge** client-signed messages.

For clients that don't support signing (legacy IRC clients), the server still signs as a fallback, providing message provenance through federation.

Client session signing keys are published at `GET /api/v1/signing-keys/{did}` so any party can verify signatures independently.

### Phase 2: End-to-End Encryption for DMs ✅ SHIPPED

**Status**: Implemented (web client)

DMs between DID-authenticated users are end-to-end encrypted:

- X25519 key exchange (X3DH — Extended Triple Diffie-Hellman)
- Double Ratchet with AES-256-GCM message encryption
- Pre-key bundles uploaded to server for async key exchange
- Sessions persisted in IndexedDB (survive page reload)
- Canonical DH ordering ensures both sides derive the same shared secret
- Server stores ciphertext only (`ENC3:` prefix) — can't read DM content
- Auto-session establishment on first message or first received encrypted message

**Remaining**: Multi-device key sync.

Recent improvements:
- Pre-key bundles are now persisted to SQLite (survive server restart)
- SPK signatures are verified using Ed25519 signing keys (prevents MITM)
- Safety number verification UX (Signal-style 60-digit fingerprint)
- DH ratchet step every 10 messages (forward secrecy on key compromise)
- iOS E2EE via Rust FFI (FreeqE2ee manager: generate/restore keys, establish sessions, encrypt/decrypt, safety numbers, session import/export for Keychain persistence)

### Phase 3: E2E Encrypted Channels

**Status**: Future research

Group E2E encryption is hard. Approaches under consideration:

- **MLS (Messaging Layer Security)**: IETF standard for group E2E, but complex
- **Sender keys**: Simpler, used by Signal for groups, weaker forward secrecy
- **Per-message encryption to each member**: Doesn't scale past ~50 members

Trade-offs:
- E2E channels can't have server-side search or history for new members
- Moderation becomes harder (server can't inspect content)
- This may be opt-in per channel rather than default

### Phase 4: Encrypted Storage at Rest ✅ SHIPPED (message content)

Message text is encrypted with AES-256-GCM before SQLite storage. Key stored in a **separate** `db-encryption-key.secret` file, independent of the message signing key. On first run, the key is derived from the signing key for backward compatibility with existing encrypted data, then persisted separately. This ensures a signing key compromise does not also compromise encrypted data.

**Remaining**: Encrypt channel metadata, policies, and identity tables. Full-database encryption via SQLCipher.

### Phase 5: HSM for Server Keys

- Verifier signing key in hardware
- TLS private key in hardware
- Iroh identity key in hardware

---

## Comparison

| Feature | freeq (today) | Slack | Discord | Signal | Matrix |
|---------|:---:|:---:|:---:|:---:|:---:|
| Transport encryption | ✅ | ✅ | ✅ | ✅ | ✅ |
| E2E DMs | ✅ | ❌ | ❌ | ✅ | ✅* |
| E2E group chat | ❌ | ❌ | ❌ | ✅ | ✅* |
| Message signatures | ✅* | ❌ | ❌ | ✅ | ❌ |
| Decentralized identity | ✅ | ❌ | ❌ | ❌ | ✅ |
| Server can read messages | Yes | Yes | Yes | No | Yes* |
| Open protocol | ✅ | ❌ | ❌ | ✅ | ✅ |
| Encrypted at rest | ✅* | Unknown | Unknown | ✅ | Varies |
| IP cloaking | ✅ | N/A | ✅ | ✅ | Varies |

*Matrix E2E is opt-in and [has had verification UX issues](https://matrix.org/blog/2024/matrix-2-0/).  
*Client-side session key signing shipped; server fallback for legacy clients.

---

## Threat Model

### What freeq protects against today

- **Network eavesdropping**: All production connections use TLS
- **Identity spoofing**: DID-based authentication with cryptographic challenge-response
- **Credential theft**: Private keys never leave the client; OAuth uses DPoP token binding
- **IP exposure**: Hostname cloaking hides real addresses
- **Nick squatting**: DID-to-nick binding prevents impersonation
- **Replay attacks**: SASL challenges are nonce-based, time-limited, single-use
- **Broker tampering**: HMAC signatures on all broker API calls

### What freeq does NOT protect against today

- **Compromised server operator**: Can read all messages and metadata
- **Compromised server host**: Plaintext database on disk
- **Metadata analysis**: Server knows who talks to whom, when, and how often
- **Compromised PDS**: Uploaded media controlled by PDS operator
- **Message forgery by server**: Closed for modern clients (client-side signing). Legacy clients still use server-attested signatures.
- **Pre-key bundle substitution**: Mitigated — SPK signatures are verified with Ed25519 signing keys. Safety number verification available for out-of-band confirmation.

### Federation security (S2S)

Federated peers are now authorization-checked:

- **Mode changes** (+o, +v, +t, +i, +n, +m, +k): Receiving server verifies the setter is an op before executing
- **Kicks**: Receiving server verifies the kicker is an op
- **Topic** (+t channels): Only ops can set topics — no "trust the peer" fallback
- **Joins**: Receiving server enforces bans and +i (invite-only) on incoming S2S joins
- **Rate limiting**: 100 events/sec per peer; excess dropped with warning log

A rogue federated peer **cannot**:
- Grant themselves op status
- Kick users from channels they don't control
- Change topics on locked channels
- Bypass bans by joining from a different server
- Flood the server with events

---

## Philosophy

We believe encryption should be **default, not optional**. The current gaps exist because we shipped transport security first (the layer that matters most immediately) and are building message-layer security in the open.

We're not going to claim E2E when we don't have it. We're not going to hide the fact that the server can read your messages today. Instead, we're publishing this document, shipping the fixes in order of impact, and inviting scrutiny.

The AT Protocol gives us a unique advantage: every user already has a cryptographic identity (DID) with signing keys. We don't need to invent a key distribution system — it already exists. Message signing and E2E encryption can build on infrastructure that's already deployed to millions of users.

**If you find a security issue**, please report it to security@freeq.at or open a GitHub issue.
