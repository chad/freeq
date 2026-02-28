# S2S Federation Authentication — Current State & Hardening Plan

## Current State (v0.1)

### What we have

1. **Transport-level identity (strong)**: S2S uses iroh QUIC connections. Each server has a persistent ed25519 keypair. The QUIC handshake cryptographically proves the peer's identity — you cannot spoof another server's endpoint ID.

2. **Inbound allowlist**: `--s2s-allowed-peers <id1>,<id2>` restricts which endpoint IDs can connect. Unauthorized peers get `conn.close("not authorized")`.

3. **Identity verification on Hello**: When a peer sends its `Hello` message, the server compares the claimed `peer_id` against the transport-authenticated identity from `remote_id()`. Mismatches are logged and the authenticated ID is used.

4. **Startup validation**: If `--s2s-peers` is set (outbound connections), `--s2s-allowed-peers` is now **required**. This prevents accidentally running an open federation. A warning is also logged if iroh is enabled without an allowlist.

5. **Rate limiting**: 100 events/sec per peer, excess dropped with warning.

### What's missing

- **Mutual auth enforcement**: Server A can restrict who connects to it, but can't force Server B to do the same. A rogue server could accept connections from anyone while appearing trusted to A.
- **No peer capability negotiation**: All peers are equal. No way to say "this peer can relay messages but not set ops."
- **No message-level signatures**: S2S messages are trusted based on transport identity alone. A compromised peer can forge any message type.
- **No revocation**: If a peer's key is compromised, the only mitigation is removing it from every other server's allowlist and restarting.
- **No audit trail**: S2S events are logged but not cryptographically attributable. You can't prove which peer originated a specific action.

---

## Hardening Plan

### Phase 1: Mutual Allowlist Enforcement (do now)

**Goal**: Both sides of a peering relationship must explicitly accept each other.

- On outbound connection, after receiving `Hello`, verify the peer's endpoint ID is in our allowlist (already done for inbound; add for outbound).
- Add a `HelloAck` message: after receiving `Hello`, respond with our own allowlist hash so the peer can verify they're trusted.
- If a peer receives a `Hello` from an endpoint not in their allowlist, disconnect with a clear error.

**Complexity**: Low. ~50 lines of code.

### Phase 2: Signed S2S Messages (next)

**Goal**: Every S2S message carries a signature from the originating server's ed25519 key. Receiving servers verify before processing.

Design:
```
S2sMessage::Envelope {
    origin_id: String,           // originating server's endpoint ID
    signature: Vec<u8>,          // ed25519 signature over payload
    payload: Vec<u8>,            // serialized inner S2sMessage
}
```

- Each server signs outbound messages with its iroh private key
- Receiving server verifies signature against the transport-authenticated peer ID
- This prevents a compromised relay from modifying messages in transit
- Also enables future multi-hop federation where messages pass through intermediaries

**Complexity**: Medium. Signing/verification is ~100 lines; refactoring all S2S message paths is ~200.

### Phase 3: Capability-Based Peering (later)

**Goal**: Different peers get different trust levels.

```toml
[s2s.peers.abc123...]
name = "community-server"
trust = "relay"          # can relay messages and presence
# trust = "full"         # can relay + set ops + set bans + set modes
# trust = "readonly"     # can observe but not write

[s2s.peers.def456...]
name = "partner-server"
trust = "full"
```

- `relay`: Can relay PRIVMSG, JOIN, PART, QUIT, NICK, TOPIC (if not +t). Cannot set modes, kick, or ban.
- `full`: Current behavior. Full trust.
- `readonly`: Receives channel state but cannot originate events. For monitoring/logging.

This replaces the current binary trust model (allowed or not) with a graduated one.

**Complexity**: Medium-high. Requires refactoring all `handle_s2s_message` dispatch paths to check trust level.

### Phase 4: Key Rotation & Revocation (later)

**Goal**: Graceful key rotation without downtime. Immediate revocation of compromised peers.

- **Rotation**: Server generates a new keypair, announces it via S2S `KeyRotation` message signed by the old key, peers update their allowlists automatically.
- **Revocation**: Admin command `OPER REVOKE-PEER <endpoint-id>` immediately disconnects and blocks a peer. Broadcast to all other peers so they also block.
- **Key pinning**: Optional TOFU (trust on first use) mode where the first connection from a peer pins their key, and subsequent connections must match.

**Complexity**: High. Requires persistent peer state, distributed revocation protocol.

### Phase 5: DID-Based Server Identity (future)

**Goal**: Servers identify via DID documents, not just raw endpoint IDs.

- Each server publishes a DID document containing its iroh endpoint ID as a service endpoint
- Peering config uses DIDs instead of endpoint IDs: `--s2s-peers did:web:irc.example.com`
- DID resolution verifies the endpoint ID matches the published document
- Key rotation becomes standard DID document updates
- Opens the door to AT Protocol integration for server discovery

**Complexity**: High. Requires DID document publication, resolution during S2S handshake, and handling DID document updates.

---

## Priority

| Phase | Effort | Impact | When |
|-------|--------|--------|------|
| 1. Mutual allowlist | Low | Closes the "one-sided trust" gap | **Now** |
| 2. Signed messages | Medium | Non-repudiation, relay safety | Before public multi-server |
| 3. Capability peering | Medium-high | Graduated trust for open federation | When >2 servers |
| 4. Key rotation | High | Operational safety | When running in production |
| 5. DID-based identity | High | Full AT Protocol alignment | Long-term |

Phase 1 is the immediate priority. Phases 2-3 should land before any public multi-server deployment. Phases 4-5 are for when the federation grows beyond trusted operators.
