# Freeq CRDT Federation Audit — Issues & Recommended Path Forward

Audience: Freeq server developers working on S2S federation + Automerge CRDT integration  
Scope: CRDT doc (`freeq-server/src/crdt.rs`), S2S transport/protocol (`freeq-server/src/s2s.rs`, `server.rs`, `iroh.rs`), and their interaction.

---

## Executive summary

Freeq's core choice — **hybrid federation** (S2S event stream for "chat" + CRDT for convergent "facts") — is a good architecture.

However, there are a handful of issues that can cause **non-convergence, excessive re-sync traffic, ghost state, split-brain semantics, and operational pain** as the cluster grows beyond "two nodes in a happy path."

This doc lists the problems found and recommends a CRDT-expert path forward:
- Make peer identity consistent and cryptographic
- Define explicit ownership / authority boundaries for "who is allowed to write what"
- Make presence lease-based (or non-CRDT)
- Make CRDT application + S2S event application idempotent and replay-safe
- Add compaction + snapshotting and real sync observability
- Replace blocking locks in async hot paths

---

## Observed architecture (so we're on the same page)

### Two layers of federation
1. **S2S "events"**: newline-delimited JSON messages over a QUIC stream (iroh) to propagate IRC-like events (`JOIN`, `PART`, `PRIVMSG`, etc.).
2. **CRDT doc**: an Automerge document that stores cluster "facts" (membership-ish, topic, founder, DID ops, bans, nick ownership), replicated using Automerge sync messages embedded in S2S messages.

### CRDT schema style
- Uses **flat keys** at the Automerge root (e.g., `topic:{channel}`, `member:{channel}:{nick}`) to avoid nested-map creation conflicts.

This is good, pragmatic CRDT engineering.

---

## Critical problems and advice

### 1) CRDT peer identity mismatch (likely to break sync correctness/efficiency)

**Symptom class**
- Sync traffic stays high and never "settles."
- Some peers appear perpetually behind, or do repeated "catch up" bursts.
- Automerge `generate_sync_message` keeps producing data even when you expect `None`.
- In worst cases, it can stall convergence in some topologies.

**Why this happens**
Automerge sync requires that each counterparty have a stable per-peer sync state keyed consistently. If we key sync state by one identifier when sending, but a different identifier when receiving, we effectively maintain *two* sync states for "the same peer," which wrecks the protocol's efficiency and can impact correctness in practice.

**Freeq-specific risk pattern**
- Outbound sync state is keyed by the *iroh endpoint id* (cryptographic transport identity).
- Inbound sync state is keyed by the *server_name string* (logical identity) taken from message origin.

That's a mismatch. One peer = two IDs.

**Advice / fix**
- **Key Automerge sync state by the transport's cryptographic identity** (iroh endpoint id) everywhere.
- Include both in the message if you want human debugging:
  - `origin_peer_id` (iroh endpoint id) — use for sync state keys
  - `origin_server_name` — use for display/logging
- Ensure that whatever you pass into `crdt_receive_sync(peer_id, ...)` matches what you use for `generate_sync_message(peer_id)`.

**Implementation note**
- Treat `server_name` as untrusted display metadata unless you cryptographically bind it to the endpoint identity.

---

### 2) Presence encoded as CRDT keys will create "ghost users" without leases

**Symptom class**
- Users appear in NAMES even after disconnects/crashes.
- Remote membership never fully drains if a server dies without emitting PART/QUIT events.
- Cluster doc grows unbounded with stale `member:*` keys.

**Why**
Presence is inherently ephemeral and failure-prone. CRDTs are excellent at converging durable facts, but presence requires **leases** (time-bounded assertions) or a separate authority model.

In "pure CRDT presence," removal requires a removal event. Crashes remove the ability to emit that event.

**Advice / fixes (pick one)**
A) **Lease-based presence (recommended if you want CRDT presence)**
- Store presence records with:
  - `member:{chan}:{nick}:{origin_server} -> {last_seen_ts}`
- Each server periodically refreshes its own presence for its users.
- Each server locally considers a member "present" only if `now - last_seen_ts < TTL`.
- No global deletion needed; stale records just age out.
- Optional: periodic compaction that deletes obviously ancient leases.

B) **Make presence NOT CRDT**
- Keep presence as S2S event-driven only, owned by origin server, with periodic "full state resync" on reconnect.
- CRDT stores only durable authorization facts (founder, ops DIDs, bans, nick ownership, channel config).

**Expert guidance**
Presence is the most common "CRDT footgun." If you keep it in CRDT, do leases.

---

### 3) "Founder is first-write-wins" is not guaranteed under concurrency

**Symptom class**
- Two servers create the same channel concurrently and disagree on founder initially.
- Eventually they converge, but founder might be arbitrary (deterministic but not semantically "first").

**Why**
Automerge resolves concurrent writes deterministically, not by wall-clock first. "Only set if absent" is not safe under concurrency without causal ordering.

**Advice**
Decide what founder *means*:

- If founder is a convenience pointer: deterministic winner is fine.
- If founder is a security boundary: you need a stronger invariant.

**Options**
A) **Make founder explicitly owned**
- Founder is set by the server that hosts the first authenticated join with a DID, and the message is signed/authorized by that DID (or by a server policy).
- Or restrict channel creation to one "authority server" per channel prefix / shard.

B) **Use a CRDT-friendly "min" register**
- Store a tuple `(timestamp, actor_id, did)` and take the minimal tuple.  
- Requires trustworthy timestamp policy (or monotonic server ticks) and accepted actor ordering.

C) **Two-phase channel creation**
- S2S handshake: "channel create request" -> "channel create accepted" to serialize creation.
- More complex, but strong semantics.

---

### 4) Authority boundaries are underspecified (who is allowed to write CRDT keys)

**Symptom class**
- A malicious or buggy peer can:
  - claim ops DIDs for itself
  - set bans for arbitrary channels
  - set nick_owner values
- Or simply diverge policy and poison the shared doc.

**Why**
CRDTs replicate *whatever operations a peer performs*. Without an authority model, replication is "trust everyone."

**Advice**
Define **write authority** per key-space:
- `topic:{channel}`: only channel ops (DID-based) or a designated "channel authority server"
- `ban:{channel}:*`: only ops
- `did_op:{channel}:*`: only founder or existing ops (bootstrap issue)
- `nick_owner:{nick}`: only the DID proving ownership (via SASL) or a global registry rule

**Enforcement strategies**
1) **Soft enforcement** (fastest to ship)
- Accept all CRDT ops, but treat values as *claims*.
- Before using a claim, check:
  - does the origin server have authority?
  - is the DID authenticated on that server?
  - does it pass policy?
- This requires storing provenance (who wrote it), which Automerge can provide via actor IDs, but you'll want explicit metadata too.

2) **Hard enforcement** (real security)
- Sign CRDT operations or signed "state claims" (ed25519) and only apply verified changes.
- Or run a constrained CRDT API: peers don't send raw CRDT ops; they send signed high-level intents that each server applies locally to its doc.

**Practical recommendation**
Start with soft enforcement + provenance + logging; migrate to hard enforcement if the cluster becomes adversarial.

---

### 5) Hybrid S2S events + CRDT facts risks duplication / ordering confusion without idempotency

**Symptom class**
- Duplicate joins/parts, duplicated messages, weird topic flips, inconsistent NAMES.
- Hard-to-reproduce race conditions during reconnect/resync.

**Why**
You have:
- an event stream that can be re-sent after reconnect
- a CRDT doc that converges separately
- and potentially a SyncRequest/SyncResponse bootstrap path

Unless every applied event is idempotent and/or deduped, reconnect behavior creates "double-apply" bugs.

**Advice**
- Give every S2S event a stable `event_id`:
  - `(origin_peer_id, monotonic_counter)` or `(origin_peer_id, uuidv7)`
- Maintain a bounded LRU per origin of seen event ids.
- Make "apply event" idempotent:
  - `JOIN`: set membership; don't assume not present
  - `PART`: remove membership; don't assume present
  - `TOPIC`: if you keep topic in CRDT, don't also set topic via event (pick one source of truth)

**Key decision**
Pick *one* source of truth for each domain:
- Topic: CRDT or S2S, not both
- Membership: leases (CRDT) or S2S + resync, not both

---

### 6) CRDT doc growth and compaction: Automerge can bloat without maintenance

**Symptom class**
- Memory usage grows over time.
- Sync payload sizes gradually increase.
- Latency spikes when saving/loading doc.

**Why**
CRDTs accumulate history. Automerge needs periodic compaction/snapshotting strategies in long-lived deployments.

**Advice**
- Implement periodic:
  - `save()` to durable storage (already present)
  - compaction strategy: snapshot the current state and discard old history where safe
- Consider:
  - One doc per channel (or per shard) instead of one global doc
  - Or segment "presence" into a separate doc you can discard aggressively

**Operational instrumentation**
- Track doc size, number of changes, sync message sizes, time-to-generate-sync.

---

### 7) Blocking `std::sync::Mutex` in async paths can stall the runtime

**Symptom class**
- Latency spikes under load.
- Occasional "stutter" where all clients feel it.
- Deadlock risk if lock ordering gets complex.

**Why**
`std::sync::Mutex` blocks the executor thread. In async Rust servers, prefer `tokio::sync::{Mutex,RwLock}` or isolate shared mutable state into a single task with message passing.

**Advice**
- Move CRDT mutation into a dedicated "state task":
  - inbound messages -> channel -> state task
  - state task mutates doc + produces outbound updates
- Or switch to `tokio::sync::RwLock` where reads dominate.

---

### 8) Transport identity vs logical identity: need explicit binding

**Symptom class**
- Confusing logs: server_name spoofing
- Peer lists showing inconsistent identities
- Potential policy bypass if any logic trusts server_name

**Advice**
- Treat iroh endpoint id as the root identity.
- Bind `server_name` to endpoint id via:
  - config allowlist mapping
  - or a signed "hello" handshake at connection establishment

---

## Recommended path forward (practical, staged)

### Phase 0: Observability + invariants (do this first)
- Add metrics:
  - sync msg size distribution
  - time between sync settles
  - doc size / change count
- Add debug endpoint/command:
  - print known peers + their sync "in sync?" state
  - list channels + founder + ops set + topic provenance

### Phase 1: Fix correctness hazards
1) **Unify peer identity used for CRDT sync state** (iroh endpoint id everywhere).
2) Decide source of truth:
   - topic: CRDT only, or S2S only
   - presence: lease CRDT, or S2S-only
3) Add event id + dedupe for S2S messages.
4) Add authoritative write boundaries (at least soft enforcement).

### Phase 2: Make it robust under failure
- Presence leases (if CRDT presence)
- Reconnect protocol:
  - exchange CRDT sync until stable
  - then replay missed events (or skip if CRDT covers it)
- Pruning / cleanup strategy for stale keys.

### Phase 3: Scale + security hardening
- Doc sharding (per-channel or per-shard)
- Compaction/snapshot policy
- Optional signed claims / high-level intent replication

---

## Concrete design recommendations (a "good default")

### A. Use CRDT for durable facts, not for chat or transient state
CRDT doc should contain:
- channel config + durable authority:
  - `founder:{chan} -> did`
  - `op:{chan}:{did} -> 1`
  - `ban:{chan}:{target} -> {policy}`
  - `topic:{chan} -> {text, set_by, ts}` (if you want)
- nick ownership:
  - `nick_owner:{nick} -> did` (but enforce by proof)

Avoid:
- message history in CRDT (unless you want full replicated logs)
- ephemeral presence without leases

### B. Presence either:
- S2S events only + periodic resync, OR
- CRDT leases with TTL interpretation, not "set/unset" keys

### C. Make federation idempotent by construction
- Every S2S event has an `event_id`
- Every handler is "set-based," not "assume and mutate"
- Reconnect can replay safely

---

## Testing plan (CRDT + federation reliability)

### 1) Deterministic simulation tests (most valuable)
Build a "cluster simulator" test harness:
- N servers
- random partitions / heals
- message drops / reorder / duplicates
- concurrent operations (topic, ops, bans, nick claims)

Assertions:
- convergence: all servers reach the same derived "facts" eventually
- no unbounded growth in presence
- dedupe correctness: applying same event twice yields no change

### 2) Soak tests
- 3–5 nodes, churn clients, kill -9 nodes randomly, run for hours
- watch sync traffic + doc size

### 3) Adversarial peer tests
- peer sends invalid server_name
- peer tries to set ops/bans without authority
- verify local policy rejects/ignores

---

## "If we do nothing" risk assessment

If you ship federation as-is and use it beyond toy scale:
- CRDT sync may become inefficient or inconsistent due to peer-id mismatch
- ghost presence will accumulate
- reconnect partitions will create duplicate-apply bugs
- authority poisoning is possible if peers aren't fully trusted
- runtime latency spikes may emerge from blocking locks

---

## Suggested next PRs (small, high-impact)

1) **Normalize CRDT peer IDs**
- Add `origin_peer_id` to CRDT sync messages
- Key sync state by that everywhere

2) **Add S2S event ids + dedupe**
- minimal LRU per peer

3) **Choose source of truth for topic + presence**
- write it down and enforce it in code

4) **Presence leases OR presence out of CRDT**
- implement one; delete the other pathway

5) **Add metrics**
- doc size, sync sizes, sync settle rate

---

## Notes on "flat key" schema

This was a smart choice. Keep it.
But:
- add namespacing discipline (`topic|ban|op|founder|presence|nick_owner`)
- avoid storing large values in single keys (topic ok; history not)

---

## Closing

Freeq's federation can be a strong, modern system if we:
- treat CRDT as the convergent store of durable facts,
- make ephemeral state lease-based or non-CRDT,
- ensure peer identity consistency,
- define write authority boundaries,
- and make S2S idempotent.

That's the difference between "cool demo" and "it holds up under partitions."
