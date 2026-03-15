# Server DID Setup

A server DID gives your freeq instance a human-readable, verifiable identity for federation. Instead of peering by raw endpoint IDs like `e6451207ec12414a...`, peers can reference your server as `did:web:irc.example.com`.

## How did:web Works

`did:web` resolves by making an HTTPS request:

```
did:web:irc.example.com  →  GET https://irc.example.com/.well-known/did.json
```

The response is a JSON-LD DID document containing the server's public keys and service endpoints.

## Setup

### 1. Start your server and note the endpoint ID

```bash
freeq-server --iroh --data-dir /var/lib/freeq ...
# Output: Iroh ready. Connect with: --iroh-addr e6451207ec12414a...
```

The endpoint ID is derived from the server's transport keypair stored in `/var/lib/freeq/iroh-key.secret`. Use the provided tool to extract the ed25519 public key and encode it as Multikey (don't assume the endpoint ID string *is* the raw pubkey — the derivation may change).

### 2. Generate the publicKeyMultibase value

```bash
python3 scripts/iroh-id-to-multibase.py <endpoint_id_from_startup>
```

This outputs the `publicKeyMultibase` value (z-prefixed base58btc with ed25519 multicodec header) for your DID document.

### 3. Create the DID document

Create `/.well-known/did.json` on your web server. The document uses **separate key IDs** for identity and S2S signing — today they're the same key, but this structure allows splitting them later without changing the document shape.

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/multikey/v1"
  ],
  "id": "did:web:irc.example.com",
  "verificationMethod": [
    {
      "id": "did:web:irc.example.com#id-1",
      "type": "Multikey",
      "controller": "did:web:irc.example.com",
      "publicKeyMultibase": "z6Mk..."
    },
    {
      "id": "did:web:irc.example.com#s2s-sig-1",
      "type": "Multikey",
      "controller": "did:web:irc.example.com",
      "publicKeyMultibase": "z6Mk..."
    }
  ],
  "authentication": ["did:web:irc.example.com#id-1"],
  "assertionMethod": ["did:web:irc.example.com#s2s-sig-1"],
  "service": [
    {
      "id": "did:web:irc.example.com#freeq-s2s",
      "type": "FreeqS2S",
      "serviceEndpoint": {
        "uri": "iroh:e6451207ec12414a...",
        "accept": ["application/freeq+s2s-v2"],
        "routing": ["direct", "relay"]
      }
    },
    {
      "id": "did:web:irc.example.com#freeq-irc",
      "type": "FreeqIRC",
      "serviceEndpoint": {
        "uri": "ircs://irc.example.com:6697"
      }
    }
  ]
}
```

**Key structure rationale:**
- `#id-1` — Server identity key. Used for authentication and DID resolution.
- `#s2s-sig-1` — S2S message signing key. Used for signed envelopes between servers. Today this is the same key as `#id-1`, but having a separate entry lets you rotate the signing key independently of identity, or use a different key type for S2S.
- Service endpoints use structured objects with `uri`, `accept` (protocol version), and `routing` hints, so you can version and negotiate without inventing new top-level fields later.

### 4. Configure freeq with the DID

```bash
freeq-server \
  --iroh \
  --server-did did:web:irc.example.com \
  --s2s-peers <peer_endpoint_id> \
  --s2s-allowed-peers <peer_endpoint_id> \
  ...
```

The `--server-did` is included in Hello handshakes so peers know your human-readable identity.

### 5. Serve the DID document

If you're running nginx in front of freeq:

```nginx
location /.well-known/did.json {
    alias /var/lib/freeq/did.json;
    add_header Content-Type application/json;
    add_header Access-Control-Allow-Origin *;
    add_header Cache-Control "public, max-age=3600";
}
```

Or if freeq's web listener is your primary server, place the file in `--web-static-dir`:

```bash
mkdir -p /var/lib/freeq/static/.well-known
cp did.json /var/lib/freeq/static/.well-known/did.json
freeq-server --web-static-dir /var/lib/freeq/static ...
```

## Resolver Behavior

Peers that resolve your DID should follow these rules:

- **Cache**: Honor `Cache-Control` headers. Recommended TTL: 1 hour for normal operation.
- **Re-fetch on mismatch**: If a signature check fails against the cached key, immediately re-resolve the DID document. The key may have rotated.
- **Grace period**: After receiving an in-band `KeyRotation` message, accept the new key for up to 24 hours even if the DID document hasn't updated yet. This covers DNS propagation delays.
- **ETag support**: Optional but helpful — use `If-None-Match` for cheap polling without full re-download.

## Verifying Your DID

```bash
curl -s https://irc.example.com/.well-known/did.json | jq .
```

Verify that:
- `id` matches your `--server-did` value
- `publicKeyMultibase` in `#id-1` matches the output of `iroh-id-to-multibase.py`
- The S2S service endpoint `uri` matches your iroh endpoint ID

## Key Rotation

Key rotation uses two complementary mechanisms:

### In-band: KeyRotation message (continuity proof)
1. The server sends `KeyRotation { old_id, new_id, timestamp, signature }` to all peers
2. The signature is by the **old** key, proving the current key holder authorized the change
3. Peers record the pending rotation and accept the new endpoint ID on reconnect

### Out-of-band: DID document update (authoritative)
1. Generate a new keypair (delete `iroh-key.secret` and restart)
2. Update `publicKeyMultibase` in your DID document with the new key
3. Update the S2S service endpoint `uri` with the new endpoint ID

**Peers accept a new key if either:**
- The DID document says so (authoritative — domain controls identity), **OR**
- The old key signed the rotation AND the DID document eventually matches (grace period)

This protects against both "peer missed the in-band message" and "domain was temporarily compromised but in-band rotation was legitimate."

## did:plc as an Alternative

`did:web` depends on DNS + TLS. If your domain or TLS termination is compromised, your DID is compromised. For higher security:

**`did:plc`** (AT Protocol's DID method) is registry-backed and DNS-independent:
- Key rotation requires signing by the recovery key, not just domain control
- Survives domain changes, hosting migrations, CA compromise
- Used by all Bluesky/AT Protocol identities

**Hybrid approach (recommended for production):**
1. Use `did:web` as the operator-friendly default (easy to set up, self-hosted)
2. Optionally create a `did:plc` for stronger security guarantees
3. Link them: add `alsoKnownAs: ["did:web:irc.example.com"]` to your `did:plc` document
4. Peers can start with `did:web`, later require `did:plc` without breaking existing config

To use `did:plc`:
```bash
freeq-server --server-did did:plc:abc123...
```

The migration path: start on `did:web`, later add `did:plc` as the primary identity. Peers that already trust your `did:web` can verify the link via `alsoKnownAs`.

## Security Notes

- The DID document **must** be served over HTTPS (required by the did:web spec)
- `did:web` security is bounded by DNS + TLS — a compromised domain = compromised identity
- For multi-server deployments, each server should have its own DID (don't share keypairs)
- The `#s2s-sig-1` key is what peers verify S2S message signatures against — keep the signing key file (`iroh-key.secret`) protected with filesystem permissions
- Consider `did:plc` for production deployments where DNS security is a concern
