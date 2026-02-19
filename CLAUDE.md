**Requirements:**
- `session_id` must be unique per TCP connection
- `nonce` must be cryptographically random
- Timestamp validity window: **≤ 60 seconds**
- Challenge must be invalidated after use

---

### 3.6 Signature Verification

The server must:

1. Resolve the DID document
2. Extract acceptable verification keys
3. Verify the signature over the exact challenge bytes

#### Key Rules

- Accept keys listed under:
  - `authentication`
  - (optional fallback) `assertionMethod`
- Do **not** accept delegation keys
- Supported curves:
  - `secp256k1` (MUST)
  - `ed25519` (SHOULD)

#### Signature Encoding

- Signature is `base64url` (unpadded)
- Signature is over raw challenge bytes
- No hashing unless explicitly required by key type

---

### 3.7 Post-Authentication Behavior

On success:

- Bind the connection to the DID
- Treat the IRC nick as a **display alias**
- Internal account identity = DID
- Emit standard IRC numeric `903`

On failure:

- Emit numeric `904`
- Terminate SASL flow cleanly
- Allow fallback to guest auth

---

### 3.8 Backward Compatibility

- Clients that do not request SASL must still connect
- Clients that do not support `ATPROTO-CHALLENGE` must still connect
- No existing IRC behavior may break

---

## 4. Deliverable B: Minimal TUI Client

### 4.1 Purpose

The client exists to:
- Prove the SASL mechanism works
- Demonstrate a realistic user flow
- Serve as a reference implementation

This is **not** a full IRC client.

---

### 4.2 Base Requirements

- Language: Go **or** Rust
- Runs in a terminal
- Uses a simple text UI (no mouse, no GUI toolkit required)
- Connects to the custom IRC server

---

### 4.3 Client Capabilities

The client must:

- Perform IRC registration
- Negotiate IRCv3 capabilities
- Perform SASL authentication using `ATPROTO-CHALLENGE`
- Join a channel
- Send and receive plain text messages

---

### 4.4 AT Authentication Flow (Client-Side)

The client must:

1. Ask the user for:
   - AT identifier (DID or handle)
2. Resolve handle → DID (if needed)
3. Authenticate to the user’s AT identity provider
   - OAuth or app-password is acceptable
4. Receive server challenge
5. Sign challenge with the user’s private key
6. Send signature via SASL
7. Complete IRC registration

Private keys **must never** be sent to the IRC server.

---

### 4.5 UX Expectations

Minimal but clear:

- Status line showing:
  - connection state
  - authenticated DID
- Clear error messages on auth failure
- No crashes on malformed server responses

---

## 5. Testing & Validation

### 5.1 Required Tests

- Successful auth with valid DID
- Failure on:
  - expired challenge
  - replayed nonce
  - invalid signature
  - unsupported key type
- Connection without SASL still works
- Standard IRC client can connect in guest mode

---

### 5.2 Manual Demo Scenario

Contractor must be able to demonstrate:

1. Start server locally
2. Connect with:
   - a standard IRC client (guest)
   - the custom TUI client (authenticated)
3. Join the same channel
4. Exchange messages

---

## 6. Documentation Deliverables

The contractor must provide:

1. **README**
   - How to build server
   - How to run server
   - How to run client
2. **Protocol Notes**
   - Any deviations or assumptions
3. **Known Limitations**
   - Explicit list

---

## 7. Acceptance Criteria

This project is complete when:

- Server successfully authenticates users via AT-backed SASL
- Client completes full auth flow without hacks
- System behaves as a normal IRC server for non-AT clients
- Code is readable, commented, and auditable
- The implementation could plausibly be referenced in an IRCv3 WG proposal

---

## 8. Philosophy (Context for the Implementer)

This project treats IRC as **infrastructure**, not a product.

The goal is to modernize identity without:
- centralization
- UX regressions
- protocol breakage

If something feels “too clever,” it’s probably wrong.

---

## TODO

### P0 — Critical (do next)

- [x] **`msgid` on all messages** — ✅ DONE. ULID on every PRIVMSG/NOTICE, carried in IRCv3 `msgid` tag, stored in DB + history, included in CHATHISTORY replay and JOIN history. S2S preserves msgid across federation.
- [ ] **Message signing by default** — All messages from DID-authenticated users should be cryptographically signed. This is the foundational trust property: if you have a DID, your messages are provably yours. Design:
  - Authenticated users sign every PRIVMSG/NOTICE/TOPIC with their DID key
  - Signature carried via IRCv3 message tag (e.g. `+freeq.at/sig=<base64url>`)
  - Signed data: `{target}\0{text}\0{timestamp}` (canonical form)
  - Server verifies signature on receipt (reject forged messages from peers)
  - S2S relayed messages carry the original signature (end-to-end verifiable)
  - Clients can verify signatures against the sender's DID document
  - Guest (unauthenticated) messages are unsigned — clearly distinguishable
  - Key types: secp256k1 (MUST), ed25519 (SHOULD) — same as SASL
  - **Scope**: PRIVMSG, NOTICE, TOPIC, KICK (actions with attribution)
  - **Non-goal for now**: signing JOIN/PART/MODE (low attribution value)

### P1 — High priority

- [ ] **Message editing** — `+draft/edit` TAGMSG referencing original `msgid`. Server enforces authorship (match DID or session), stores as new message with `replaces` field. CHATHISTORY returns edits correctly. ~100 lines. (See `docs/WEB-APP-PLAN.md` §2.4)
- [ ] **Message deletion** — `+draft/delete` TAGMSG referencing `msgid`. Soft delete (mark deleted, clients hide). Same authorship check. ~50 lines.
- [x] **`away-notify` cap** — ✅ DONE. Broadcast AWAY changes to shared channel members. Server, SDK, TUI, and web client all support it.
- [ ] **S2S authorization on Kick/Mode** — Receiving server should verify the kicker/mode-setter has authority (is an op) before executing. Currently any peer can forge kicks/ops.
- [ ] **S2S authorization on Topic** — Verify `set_by` belongs to the authenticated peer, not a spoofed nick.
- [ ] **SyncResponse channel creation limit** — Cap channels created via sync to prevent a rogue peer from creating thousands of channels.
- [ ] **ChannelCreated should propagate default modes** — Receiving side uses `or_default()` which sets all modes to false. Should inherit +nt defaults so remote channels have standard protections.
- [ ] **Invites should sync via S2S** — Currently invites are local server state only. A user invited on server A can only join on server A. Relay invite tokens to peers.
- [ ] **S2S rate limiting** — Connected peers can flood events without throttling.
- [ ] **DPoP nonce retry for SASL verification** — PDS nonce rotation causes server-side verification to fail.

### P2 — Important

- [ ] **Topic merge consistency** — SyncResponse ignores remote topic if local is set, but CRDT reconciliation overwrites. Two systems with different merge strategies cause flapping.
- [ ] **Channel key removal propagation** — `-k` can't propagate via SyncResponse (only additive). Needs protocol change or CRDT-backed key state.
- [ ] **S2S authentication (allowlist enforcement)** — `--s2s-allowed-peers` only checks incoming. Formalize mutual auth.
- [ ] **Ban sync + enforcement** — Bans are local-only despite CRDT support. Wire up S2S ban propagation.
- [ ] **S2S Join enforcement** — Incoming S2S Joins don't check bans or +i.
- [ ] **Hostname cloaking** — All users show `host` placeholder. Implement cloaking for public deployments.
- [ ] **IRCv3: account-notify / extended-join** — Broadcast DID on auth and in JOIN.
- [ ] **IRCv3: CHATHISTORY** — On-demand history retrieval (persistence layer supports it).
- [ ] **Connection limits** — Per-IP connection limits.
- [ ] **OPER command** — Server operator status for remote admin.
- [ ] **TUI auto-reconnection** — Reconnect with backoff, rejoin channels.
- [ ] **Normalize nick_to_session to lowercase keys** — Avoids O(n) linear scan on every case-insensitive nick lookup. Currently all nick lookups iterate the full map.

### P2.5 — Web App Prerequisites (see `docs/WEB-APP-PLAN.md`)

- [ ] **Web app (Phase 1)** — React+TS+Vite+Tailwind. IRC-over-WebSocket adapter, Zustand store, AT Protocol profile resolution, basic channel/message/member UX. Separate repo (`freeq-app`).
- [ ] **Search (FTS5)** — SQLite FTS5 for message search. REST endpoint or IRC SEARCH command.
- [ ] **Pinned messages** — Channel metadata via TAGMSG or MODE variant.

### P3 — Future

- [ ] Wire CRDT to live S2S (replace ad-hoc JSON for durable state)
- [ ] DID-based key exchange for E2EE (replace passphrase-based)
- [ ] Full-text search (SQLite FTS5)
- [ ] Bot framework (formalize SDK pattern)
- [ ] AT Protocol record-backed channels
- [ ] Reputation/trust via social graph
- [ ] Serverless P2P mode
- [ ] IRCv3 WG proposal for ATPROTO-CHALLENGE
- [ ] Web client (separate repo, PWA)
- [ ] Moderation event log (CRDT-backed, ULID-keyed)
- [ ] AT Protocol label integration for moderation

### Done (this session)

- [x] Case-insensitive remote_members helpers (`remote_member()`, `has_remote_member()`, `remove_remote_member()`)
- [x] All S2S handlers use case-insensitive nick lookups (Privmsg +n/+m, Part, Quit, NickChange, Mode +o/+v, Kick, Topic)
- [x] SyncResponse mode protection (never weakens local +n/+i/+t/+m)
- [x] Topic flow fix (S2S Topic +t trusts peer authorization for unknown users)
- [x] KICK sending-side case-insensitive remove
- [x] 15 new edge case acceptance tests (96 total, all passing)
- [x] Full S2S sync audit (`docs/SYNC-AUDIT.md`)
- [x] Lint updated to catch raw remote_members access

---

**End of document**
