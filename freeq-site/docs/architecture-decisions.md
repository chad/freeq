# Architecture Decisions: Protocol Boundary and Moderation

## 1. The Two-Tier Problem

The moment we add capabilities that legacy clients can't see — DID-based identity, CRDT-synced state, iroh transport, E2EE channels — we create two tiers of experience. This is the central design tension.

### Our Position

**Two tiers are inevitable and correct.** The question is whether the degradation is graceful.

Think of it like HTTP/1.1 and HTTP/2. Both work. One gets more features. Neither breaks the other. The protocol boundary must be explicit, and the degradation must be clean.

### Where We Draw the Line

| Feature | Legacy Client | Modern Client | Mechanism |
|---------|--------------|---------------|-----------|
| Connect, join, chat | ✓ | ✓ | IRC protocol |
| Nick registration | Server-local only | DID-bound, portable | SASL ATPROTO-CHALLENGE |
| Identity in WHOIS | nick!user@host | + DID + AT handle + iroh ID | IRCv3 CAP |
| E2EE channels | See ciphertext `ENC1:...` | Decrypted messages | Client-side |
| P2P DMs | Not available | Encrypted, server-free | iroh transport |
| Channel state after netsplit | Server authority | CRDT merge | Invisible to clients |
| Message tags (reactions, typing) | Not available | ✓ | IRCv3 `message-tags` CAP |

### The Seam: IRCv3 CAP Negotiation

CAP negotiation is the right boundary. It was designed for exactly this:

```
Client: CAP LS
Server: CAP * LS :sasl message-tags
Client: CAP REQ :sasl message-tags
Server: CAP * ACK :sasl message-tags
```

We currently advertise: `sasl message-tags`

We should extend to:

```
CAP * LS :sasl message-tags did-identity channel-crdt p2p-dm
```

Where:
- **`did-identity`** — Server will include DID and AT handle in WHOIS numerics (672, etc.)
- **`channel-crdt`** — Channel state is CRDT-backed; client may see members from other servers
- **`p2p-dm`** — Server supports relaying iroh endpoint IDs for peer-to-peer DMs

A legacy client that doesn't REQ these caps sees a standard IRC server. No special cases in the protocol handler. The CAP bitmask determines what extra information flows.

### What Legacy Clients Miss (Explicitly)

1. **Identity portability** — Your nick is server-local. No DID, no proof of identity.
2. **E2EE** — You see `ENC1:nonce:ciphertext` as the message body. You can't decrypt it. This is intentional — encrypted rooms are opt-in and participants must use a capable client.
3. **P2P DMs** — Not available. DMs go through the server as standard PRIVMSG.
4. **Moderation context** — You can't see that a ban is DID-based, not hostmask-based. It still works (the server enforces it), but you can't inspect why.

### What Legacy Clients DO Get

1. **Full IRC** — JOIN, PART, PRIVMSG, MODE, KICK, TOPIC, NAMES, WHOIS, all work.
2. **Guest access** — Connect without SASL, get a nick, chat.
3. **Operator privileges** — If granted by an authenticated user, work normally.
4. **Plaintext channels** — Any channel without E2EE enabled works identically.

---

## 2. Moderation in a DID-First World

This is the harder problem. IRC's moderation model assumes server authority:

- **Chanops** are granted by other chanops or the server
- **Server opers** have god mode
- **Bans** are hostmask patterns, tied to connection metadata
- **K-lines** are server-level bans

When identity is portable (DIDs) and servers are peers (CRDT mesh), every one of these assumptions breaks.

### 2.1 What Breaks

**Nick-based ops lose meaning across servers.** If Alice is a chanop on Server A and Bob joins from Server B, who granted Alice's authority? Server A did. But Server B has never seen Alice.

**Hostmask bans become meaningless with portable identity.** A ban on `alice!*@*` doesn't work when Alice reconnects via iroh with a different transport address. DID-based bans work (`did:plc:abc123`), but they require the ban to be a semantic statement about *identity*, not *connection metadata*.

**Server authority fragments.** In classic IRC, the server you're connected to is the authority. In a mesh, there's no single authority for a channel.

### 2.2 What We're Building

#### DID-Based Bans (Already Implemented)

We already support banning by DID:

```
/ban did:plc:abc123
```

This bans the cryptographic identity, not a hostname. It works regardless of how the user connects (TCP, TLS, iroh, different IP). The ban entry is stored in `BanEntry` with both hostmask and DID matching.

#### Moderation Log as a CRDT

The feedback is exactly right: **the moderation log itself must be a CRDT.**

Current state: our `crdt.rs` uses flat keys like `ban:{channel}:{mask} → set_by`. This handles add/remove cleanly via Automerge's put/delete semantics.

But it doesn't capture:
- **Who** performed the action
- **When** (causal ordering)
- **Why** (reason)
- **Conflict resolution** for concurrent ban + unban

#### Proposed: Moderation Events in the CRDT

```
modaction:{channel}:{ulid} → {
  "action": "ban",
  "target": "did:plc:abc123",
  "by": "did:plc:operator",
  "reason": "spam",
  "timestamp": 1707500000
}
```

Using ULIDs (time-sortable unique IDs) as keys means:
- No two moderation actions conflict (unique keys)
- Temporal ordering is preserved
- The full moderation history is auditable
- Concurrent ban + unban produces *both* events in the log

To determine current ban state, you **fold the moderation log**: replay actions in causal order, applying bans and unbans. The last action wins.

This is more expensive than a simple set, but it gives us:
- **Auditability** — Full history of who did what and when
- **Conflict resolution** — Concurrent actions are both recorded; latest wins
- **Portability** — The moderation log travels with the channel state across servers
- **Accountability** — Every moderation action is attributed to a DID

#### Authority Model

The hard question: **who is allowed to moderate?**

Options, from simplest to most sophisticated:

**Option A: Channel founder model** (simplest)
- The DID that creates a channel is the founder
- Founders can grant/revoke operator status
- Operator grants are themselves CRDT entries: `op:{channel}:{did} → granting_did`
- Only operators can add moderation events
- Server validates operator status before accepting ban/kick commands

**Option B: Threshold model**
- Moderation actions require N-of-M operator agreement
- A ban proposed by one operator becomes effective only when K others confirm
- The CRDT stores proposals and confirmations separately
- More complex, but resistant to rogue operators

**Option C: Reputation/stake model** (future)
- Moderation weight tied to identity reputation
- AT Protocol social graph as a trust signal
- Out of scope for now, but the CRDT structure supports it

**Our choice: Option A first**, with the CRDT schema designed to support B later.

### 2.3 The Merge Problem

What happens when two servers in a mesh have conflicting moderation state?

**Scenario:** Server A bans `did:plc:evil`. Server B unbans `did:plc:evil`. Both happen concurrently before sync.

**With our moderation log approach:**
1. After sync, both events exist in the CRDT
2. Both have timestamps (and Automerge causal ordering)
3. Fold the log: the temporally-later action wins
4. If truly simultaneous: Automerge's deterministic conflict resolution picks one
5. Both servers converge to the same state

**Scenario:** Server A's operator bans a user. Server B doesn't recognize A's operator.

**Resolution:**
1. Operator grants are also in the CRDT
2. If the grant hasn't synced yet, Server B will reject the ban locally
3. Once the grant syncs, Server B can re-evaluate
4. Alternatively: accept all moderation events and validate authority during fold

This is why the moderation log is better than a simple ban set — you can retroactively validate authority as state converges.

### 2.4 What's Implemented Now vs. What's Next

**Now:**
- DID-based bans (server-local, `BanEntry` with DID matching)
- Automerge CRDT with `ban:{channel}:{mask} → set_by`
- Channel ops granted per-session

**Next:**
- [ ] Moderation event log in CRDT (ULIDs, full attribution)
- [ ] Operator grants as CRDT entries (DID → DID)
- [ ] Authority validation during ban fold
- [ ] Channel founder registration (first DID to create channel)
- [ ] CAP extension for moderation transparency (`mod-log` capability?)

---

## 3. Why This Matters (BlueYard Thesis Alignment)

This project sits at the intersection of:

1. **Open protocol infrastructure** — IRC is the original open chat protocol. We're not replacing it; we're upgrading the substrate.

2. **Crypto-native identity** — DIDs from the AT Protocol give us portable, self-sovereign identity without a new identity system. Your Bluesky account IS your IRC account.

3. **P2P transport** — Iroh gives us encrypted, NAT-traversing, relay-capable connectivity without centralized infrastructure.

4. **Local-first data** — Automerge CRDTs mean server state is replicated, conflict-free, and doesn't require a consensus leader.

The thesis: **you can modernize a 37-year-old protocol without breaking it, by upgrading identity, transport, and state management independently.** Each layer is separable. Each layer degrades gracefully. No single point of failure.

If this works for IRC, it's a template for upgrading any open protocol infrastructure.
