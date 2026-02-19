# S2S Sync Audit — Full Code Review

**Date**: 2026-02-18  
**Scope**: `server.rs` (process_s2s_message, reconcile_crdt_to_local), `s2s.rs` (transport, dedup, connection lifecycle)  
**Status**: Audit complete. Findings below.

---

## ✅ Things that are correct

### Transport identity
- Peer identity is `conn.remote_id()` (iroh QUIC cryptographic endpoint ID). Cannot be spoofed.
- `AuthenticatedS2sEvent` threads the transport identity through the event channel.
- CRDT sync is keyed by `authenticated_peer_id`, not the JSON `origin` field.
- Hello handler warns on mismatch but always uses authenticated ID.

### Event dedup
- Two-layer: monotonic high-water mark + ring buffer.
- High-water mark resets on peer disconnect (prevents clock skew issues).
- Event IDs seeded from wall-clock microseconds — survives restarts.
- Connection generation counter prevents cleanup races (newer connection keeps its entry).

### Broadcast ordering
- Single broadcast worker task ensures messages reach QUIC in event_id order.
- Without this, the monotonic dedup on the receiver would reject out-of-order events.

### CRDT sync protocol
- Respond only to sender (`crdt_sync_with_peer`), not broadcast — prevents amplification.
- Periodic reconciliation (every 60s) applies CRDT truth to local state.
- Compaction every 30 minutes bounds doc growth.

### Ghost cleanup
- `PeerDisconnected` synthetic event cleans remote_members when link drops.
- SyncResponse replaces (not merges) remote_members per peer before adding.
- Dedup state cleared on disconnect.

### Channel normalization
- All S2S handlers now normalize channel names to lowercase.
- `deliver_to_channel` also lowercases internally.

---

## ⚠️ Bugs found (to fix)

### BUG 1: S2S Mode +o/+v nick lookup is still case-sensitive

**File**: `server.rs` ~line 1420  
**Code**:
```rust
let target_sid = state.nick_to_session.lock().unwrap()
    .get(target_nick).cloned();
```

The Mode handler for `o`/`v` does a raw case-sensitive lookup in `nick_to_session`. Should use case-insensitive search like the Kick handler does.

**Impact**: Remote +o/+v silently fails if nick case differs.

### BUG 2: S2S +n check uses case-sensitive `remote_members.contains_key`

**File**: `server.rs` ~line 940  
**Code**:
```rust
let is_member = ch.remote_members.contains_key(nick)
```

The +n enforcement on incoming S2S Privmsg checks `remote_members.contains_key(nick)` case-sensitively. If the nick in the S2S message has different case than what's in `remote_members`, a legitimate member's message gets blocked.

**Impact**: Intermittent message drops for remote users in +n channels when nick case varies.

### BUG 3: S2S +m check uses case-sensitive `remote_members.get`

**File**: `server.rs` ~line 949  
**Code**:
```rust
let is_privileged = ch.remote_members.get(nick)
    .is_some_and(|rm| rm.is_op);
```

Same case-sensitivity issue for moderated channel check.

### BUG 4: S2S Part/Quit use case-sensitive `remote_members.remove`

**File**: `server.rs` ~line 1001, 1017  
**Code**:
```rust
ch.remote_members.remove(&nick);
```

If the nick in the Part/Quit message has different case than the key in `remote_members`, the removal silently fails and the user becomes a ghost.

### BUG 5: NickChange uses case-sensitive `remote_members.remove`

**File**: `server.rs` ~line 1539  
The NickChange handler does `ch.remote_members.remove(&old)` case-sensitively.

### BUG 6: SyncResponse channel modes overwrite local settings unconditionally

**File**: `server.rs` ~lines 1317-1323  
**Code**:
```rust
ch.topic_locked = info.topic_locked;
ch.invite_only = info.invite_only;
ch.no_ext_msg = info.no_ext_msg;
ch.moderated = info.moderated;
```

When a SyncResponse arrives, channel modes are overwritten with the remote peer's values. If server A has `+n` (from local creation defaults) and server B syncs with `no_ext_msg: false` (because B's channel was created before the +nt default), server A loses its +n protection.

**Impact**: Security — a peer can silently disable +n, +m, +i, or +t on local channels by syncing stale/default state.

**Fix**: Only adopt remote modes if the local channel has no local members (i.e., the channel only exists remotely). If local members exist, local modes take precedence (or merge via CRDT).

### BUG 7: SyncResponse topic merge only checks "is None"

**File**: `server.rs` ~line 1306  
**Code**:
```rust
if ch.topic.is_none() {
    if let Some(ref topic) = info.topic { ... }
}
```

If the local channel has a topic set, a remote topic is ignored. But the CRDT reconciliation (every 60s) may overwrite it anyway. The two systems (SyncResponse vs CRDT reconciliation) have different merge strategies for topics, which can cause flapping.

**Impact**: Topic inconsistency between servers. Not a security issue but confusing UX.

### BUG 8: SyncResponse key merge is additive only

**File**: `server.rs` ~line 1324  
**Code**:
```rust
if info.key.is_some() {
    ch.key = info.key.clone();
}
```

If server A removes a channel key (-k), and server B syncs with the old key, server A re-adopts the key. There's no way to propagate key removal via SyncResponse.

**Impact**: Channel key can't be reliably removed when federation is active.

---

## ⚠️ Edge cases (not bugs but risky)

### EDGE 1: SyncResponse + Kick race condition

If server A kicks user B, removing them from remote_members, but server B sends a SyncResponse before processing the kick, server A re-adds user B to remote_members (from the SyncResponse's member list). User B appears as a ghost until the next SyncResponse from B (which won't include them because B processed the kick locally).

**Window**: time between kick and next SyncResponse (typically seconds).  
**Mitigation**: The kicked user doesn't receive messages (removed from ch.members on B), so the ghost is cosmetic only (visible in NAMES but can't send/receive).

### EDGE 2: No authorization on S2S Kick

Any connected peer can send an S2S Kick and the receiving server will execute it without checking whether the kicker was actually an op. A rogue or compromised peer could kick arbitrary users from any channel.

**Fix**: The receiving server should verify the `by` nick is an op in the channel (either local op or remote member with is_op/did_ops authority) before executing the kick.

### EDGE 3: No authorization on S2S Mode

Same as EDGE 2 — any peer can send `S2sMessage::Mode { mode: "+o", ... }` and the receiver applies it without checking whether `set_by` had authority to make that change.

### EDGE 4: S2S Topic +t enforcement is bypassable

The +t check on incoming S2S Topic uses `remote_members.get(&set_by)` to check authorization. But `set_by` is a free-form string in the S2S message — a malicious peer can set it to any nick that happens to be an op.

**Fix**: The receiving server should verify `set_by` is associated with the `authenticated_peer_id` (i.e., the nick belongs to the peer that sent the message).

### EDGE 5: Dedup counter assumes monotonic wall clock

Event IDs use `SystemTime::now().as_micros()` as the counter seed. If the system clock jumps backward (NTP correction, VM resume), the counter could produce values below the peer's high-water mark, causing events to be silently dropped.

The high-water mark IS reset on disconnect, so this only matters for backward jumps during an active session. The `AtomicU64::fetch_add` ensures local monotonicity within a session, so the only risk is if the initial seed on restart is lower than the peer's saved high-water mark from the previous session — but disconnect resets that too.

**Verdict**: Safe in practice, but a comment documenting this would help.

### EDGE 6: `send_names_update` holds two locks simultaneously

`send_names_update` acquires `channels` lock, then `nick_to_session` lock, then `connections` lock — all in one function. While the lock order is consistent (which prevents deadlocks), holding three Mutex locks simultaneously in a hot path increases contention.

---

## 🔒 Security issues

### SEC 1: No peer allowlist enforcement on outgoing connections

`handle_incoming_s2s` checks `s2s_allowed_peers`, but outgoing connections (`connect_peer`) do not. The allowlist only gates incoming — an attacker who can get their peer ID into `--s2s-peers` bypasses the allowlist.

**Impact**: Low (requires config modification), but the asymmetry is surprising.

### SEC 2: S2S messages not rate-limited

A connected peer can flood S2S messages (Join, Part, Privmsg, etc.) without any rate limiting. The event channel is buffered (1024) but a fast peer can overwhelm the event processor.

### SEC 3: SyncResponse can create arbitrary channels

`channels.entry(info.name.clone()).or_default()` in the SyncResponse handler creates channels that don't exist locally. A peer can create thousands of channels by sending large SyncResponses.

### SEC 4: Kicked user still in remote_members until next sync

After a local kick removes a user from ch.members, the user's nick may still appear in remote_members on other servers until the next SyncResponse propagates. During this window, the ghost entry could be used to satisfy nick resolution (e.g., for INVITE).

---

## Status

### Fixed in this commit

- **BUG 1**: S2S Mode +o/+v — now uses case-insensitive local nick lookup
- **BUG 2-3**: S2S Privmsg +n/+m — now uses `has_remote_member()`/`remote_member()` helpers
- **BUG 4**: S2S Part — now uses `remove_remote_member()`
- **BUG 5**: S2S NickChange — now uses `remove_remote_member()`
- **BUG 6**: SyncResponse modes — now only adopts modes from peer if no local members; with local members, only makes channels MORE restrictive (never weakens protections)
- **Topic not flowing**: S2S Topic +t enforcement was rejecting legitimate remote topic changes because (a) case-sensitive `remote_members.get()` and (b) the remote op might not be in `remote_members` yet (race with Join). Now uses case-insensitive lookup and accepts topic changes from unknown remote users (trusts peer's authorization decision).
- **Kick handler**: refactored to use `remove_remote_member()` helper (was inline case-insensitive code)
- **ChannelState helpers**: Added `remote_member()`, `remote_member_mut()`, `has_remote_member()`, `remove_remote_member()` — all case-insensitive. No more raw `remote_members.get()`/`contains_key()`/`remove()` in S2S handlers.

### Remaining (design decisions needed)

- **BUG 7**: Topic merge (SyncResponse ignores if set, CRDT overwrites) — needs design
- **BUG 8**: Channel key removal can't propagate via SyncResponse — needs protocol change
- **EDGE 2-3**: No auth on S2S Kick/Mode — needs authority verification
- **EDGE 4**: Topic +t bypass via spoofed `set_by` — needs origin verification
- **SEC 2**: No S2S rate limiting — needs design
- **SEC 3**: SyncResponse creates arbitrary channels — needs limit

## Recommended priority

| # | Type | Severity | Effort |
|---|------|----------|--------|
| BUG 1-5 | Case-sensitive nick lookups | High | Small (same pattern as Kick fix) |
| BUG 6 | SyncResponse overwrites modes | High (security) | Medium |
| EDGE 2-3 | No auth on S2S Kick/Mode | High (security) | Medium |
| SEC 3 | SyncResponse creates channels | Medium | Small (add limit) |
| BUG 7-8 | Topic/key merge inconsistency | Low | Medium (needs design) |
| EDGE 4 | Topic +t bypassable | Medium | Medium |
| SEC 2 | No rate limiting | Medium | Medium |
| EDGE 1 | Kick+Sync race | Low (cosmetic) | Deferred |
| EDGE 5 | Clock monotonicity | Low (safe) | Comment only |
| EDGE 6 | Lock contention | Low | Deferred |
