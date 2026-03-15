# S2S Federation Authentication

## Architecture

Freeq's server-to-server (S2S) federation uses a layered security model:

```
Layer 1: Transport Identity (iroh QUIC)     — WHO is connecting
Layer 2: Mutual Hello/HelloAck              — BOTH sides agree to peer
Layer 3: Signed Message Envelopes           — messages can't be tampered
Layer 4: Capability-Based Trust             — WHAT each peer can do
Layer 5: Key Rotation & Revocation          — operational safety
Layer 6: DID-Based Server Identity          — human-readable peering
```

All layers are implemented and active.

---

## Layer 1: Transport Identity

S2S connections use **iroh QUIC**, which provides ed25519 keypair identity at the transport level. Each server has a persistent keypair (`iroh-key.secret` in the data directory). The QUIC handshake cryptographically proves the peer's identity — spoofing is impossible.

- `conn.remote_id()` returns the peer's public key (endpoint ID)
- This is the root of trust for everything else

## Layer 2: Mutual HelloAck

When two servers connect:

1. Both send `Hello` with their endpoint ID, server name, protocol version, and trust level
2. Each side verifies the peer is in their `--s2s-allowed-peers` allowlist
3. Each side responds with `HelloAck { accepted: bool, trust_level }`
4. If either side sends `accepted: false`, the link is torn down

This ensures **both servers** explicitly consent to peering. A rogue server cannot join the federation by connecting to one server — the other servers will reject it.

**Config:**
```bash
# Server A
--s2s-peers <B_endpoint_id> --s2s-allowed-peers <B_endpoint_id>

# Server B
--s2s-peers <A_endpoint_id> --s2s-allowed-peers <A_endpoint_id>
```

Starting with `--s2s-peers` but without `--s2s-allowed-peers` is a **startup error**.

## Layer 3: Signed Message Envelopes

Every S2S message (except Hello, HelloAck, and KeyRotation) is wrapped in a `Signed` envelope:

```json
{
  "type": "signed",
  "payload": "<base64url-encoded JSON of inner message>",
  "signature": "<base64url ed25519 signature over payload bytes>",
  "signer": "<endpoint ID of signing server>"
}
```

The receiving server:
1. Verifies `signer` matches the transport-authenticated peer ID
2. Verifies the ed25519 signature over the raw payload bytes
3. Deserializes the inner message only if signature is valid

Messages with invalid signatures are dropped with a warning log.

This provides **non-repudiation**: you can prove which server originated a message, even in multi-hop scenarios.

## Layer 4: Capability-Based Trust

Each peer is assigned a trust level that controls what operations they can perform:

| Trust Level | Messages | Presence | Modes/Kick/Ban | Channel Create |
|-------------|----------|----------|----------------|----------------|
| `full`      | ✓        | ✓        | ✓              | ✓              |
| `relay`     | ✓        | ✓        | ✗              | ✗              |
| `readonly`  | ✗        | ✗        | ✗              | ✗              |

**Config:**
```bash
# Give partner-server full trust, community-server relay-only
--s2s-peer-trust "abc123...:full,def456...:relay"
```

Peers not listed default to `full` (backward compatible). Trust is enforced server-side — a relay peer's MODE/KICK/BAN messages are silently dropped.

## Layer 5: Key Rotation & Revocation

### Key Rotation

Key rotation uses two complementary mechanisms for safety:

**In-band (continuity proof):**
1. Server sends `KeyRotation { old_id, new_id, timestamp, signature }` to all peers
2. Signature is by the **old** key over `rotate:{old_id}:{new_id}:{timestamp}`
3. Peers verify, record the pending rotation, accept new ID on reconnect
4. Rotation signatures must be within 5 minutes of current time (replay protection)

**Out-of-band (authoritative):**
1. Update the DID document (`/.well-known/did.json`) with the new public key
2. Peers re-resolve the DID on signature mismatch

**Acceptance rule:** Peers accept a new key if either:
- The DID document says so (authoritative — domain controls identity), **OR**
- The old key signed the rotation AND the DID document eventually matches (24h grace period)

This protects against both "peer missed the in-band message" and "domain was temporarily compromised but the in-band rotation was legitimate."

### Peer Revocation

Server operators can immediately revoke a peer's access:

```
OPER admin <password>
REVOKEPEER <endpoint_id>
```

This:
- Disconnects the peer immediately
- Removes them from authenticated peers
- Clears their dedup state
- Logs the revocation

To permanently block a peer, remove them from `--s2s-allowed-peers` and restart.

## Layer 6: DID-Based Server Identity

Servers can optionally identify via DID:

```bash
--server-did did:web:irc.example.com   # default: easy, DNS-based
--server-did did:plc:abc123...          # advanced: registry-backed, DNS-independent
```

The DID is included in Hello handshakes. The DID document publishes:
- **Identity key** (`#id-1`): stable server identity
- **S2S signing key** (`#s2s-sig-1`): used for message envelope signatures
- **Service endpoints**: iroh transport address, IRC connection info

Today both keys are the same (the iroh keypair). The DID document is structured with separate key IDs so they can be split later without changing the document shape.

**`did:web` vs `did:plc`:**
- `did:web` is simpler (just serve a JSON file over HTTPS) but depends on DNS + TLS security
- `did:plc` is registry-backed, survives domain changes and CA compromise, but requires the PLC directory
- Hybrid approach: start with `did:web`, add `did:plc` later, link via `alsoKnownAs`

See [Server DID Setup](server-did.md) for full setup instructions.

---

## Startup Validation

The server enforces safe defaults at startup:

1. If `--s2s-peers` is set, `--s2s-allowed-peers` is **required** (prevents accidental open federation)
2. If iroh is enabled without an allowlist, a **warning** is logged
3. If an outbound peer isn't in the allowlist, a **warning** is logged (config mismatch)

## Rate Limiting

S2S events are rate-limited to 100 events/sec per peer. Excess events are dropped with a warning log.
