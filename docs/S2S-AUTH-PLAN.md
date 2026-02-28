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

A server can rotate its iroh keypair without breaking peering:

1. Server generates a new keypair
2. Sends `KeyRotation { old_id, new_id, timestamp, signature }` to all peers
3. The signature is by the **old** key over `rotate:{old_id}:{new_id}:{timestamp}`
4. Peers verify the signature, record the pending rotation
5. When the server reconnects with the new ID, peers accept it

Rotation signatures must be within 5 minutes of current time (replay protection).

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

## Layer 6: DID-Based Server Identity (Phase 5)

Servers can optionally identify via DID:

```bash
--server-did did:web:irc.example.com
```

This DID is included in the Hello handshake. Future work:
- Peers can allowlist by DID instead of endpoint ID
- DID document publishes the endpoint ID as a service endpoint
- Key rotation = DID document update
- Opens the door to AT Protocol integration for server discovery

---

## Startup Validation

The server enforces safe defaults at startup:

1. If `--s2s-peers` is set, `--s2s-allowed-peers` is **required** (prevents accidental open federation)
2. If iroh is enabled without an allowlist, a **warning** is logged
3. If an outbound peer isn't in the allowlist, a **warning** is logged (config mismatch)

## Rate Limiting

S2S events are rate-limited to 100 events/sec per peer. Excess events are dropped with a warning log.
