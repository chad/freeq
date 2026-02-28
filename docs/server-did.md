# Server DID Setup

A server DID gives your freeq instance a human-readable identity for federation. Instead of peering by raw endpoint IDs like `e6451207ec12414a...`, peers can reference your server as `did:web:irc.example.com`.

## How did:web Works

`did:web` resolves by making an HTTPS request:

```
did:web:irc.example.com  →  GET https://irc.example.com/.well-known/did.json
did:web:irc.example.com:federation  →  GET https://irc.example.com/federation/did.json
```

The response is a JSON-LD DID document containing the server's public keys and service endpoints.

## Setup

### 1. Start your server and note the endpoint ID

```bash
freeq-server --iroh --data-dir /var/lib/freeq ...
# Output: Iroh ready. Connect with: --iroh-addr e6451207ec12414a...
```

The iroh endpoint ID is derived from the persistent keypair in `/var/lib/freeq/iroh-key.secret`.

### 2. Create the DID document

Create `/.well-known/did.json` on your web server (the same domain you'll use in the DID):

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/multikey/v1"
  ],
  "id": "did:web:irc.example.com",
  "verificationMethod": [
    {
      "id": "did:web:irc.example.com#iroh-1",
      "type": "Multikey",
      "controller": "did:web:irc.example.com",
      "publicKeyMultibase": "z6Mk..."
    }
  ],
  "authentication": ["did:web:irc.example.com#iroh-1"],
  "service": [
    {
      "id": "did:web:irc.example.com#s2s",
      "type": "FreeqS2S",
      "serviceEndpoint": "iroh:e6451207ec12414a..."
    },
    {
      "id": "did:web:irc.example.com#irc",
      "type": "FreeqIRC",
      "serviceEndpoint": "ircs://irc.example.com:6697"
    }
  ]
}
```

### 3. Generate the publicKeyMultibase value

The iroh keypair is ed25519. To get the multibase-encoded public key:

```bash
# Read the hex secret key
SECRET_HEX=$(cat /var/lib/freeq/iroh-key.secret)

# The endpoint ID printed at startup IS the public key (hex-encoded).
# Convert to multibase (z = base58btc, 0xed01 = ed25519 multicodec prefix):
python3 -c "
import base58
endpoint_id = 'e6451207ec12414a...'  # your full endpoint ID
pub_bytes = bytes.fromhex(endpoint_id)
multicodec = b'\\xed\\x01' + pub_bytes
print('z' + base58.b58encode(multicodec).decode())
"
```

Or use the helper script:

```bash
# From the freeq repo:
python3 scripts/iroh-id-to-multibase.py /var/lib/freeq/iroh-key.secret
```

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
}
```

Or if freeq's web listener is your primary server, you can place the file in `--web-static-dir`:

```bash
cp did.json /var/lib/freeq/static/.well-known/did.json
freeq-server --web-static-dir /var/lib/freeq/static ...
```

## Verifying Your DID

Test that it resolves:

```bash
curl https://irc.example.com/.well-known/did.json | jq .
```

You should see your DID document with the correct endpoint ID in the service endpoint.

## Key Rotation

When you rotate your iroh keypair:

1. Generate a new keypair (delete `iroh-key.secret` and restart — a new one is created)
2. The server sends a `KeyRotation` message to all peers (signed by the old key)
3. Update your DID document with the new public key
4. Peers automatically accept the new endpoint ID

The `KeyRotation` message provides cryptographic proof of continuity — the old key signs off on the new one.

## Security Notes

- The DID document must be served over HTTPS (required by the did:web spec)
- The `publicKeyMultibase` in the document should match your iroh endpoint ID
- Peers can verify your identity by resolving your DID and checking the endpoint ID matches
- `did:web` security depends on DNS + TLS — if your domain is compromised, your DID is compromised
- For higher security, consider `did:plc` (AT Protocol's DID method) which is DNS-independent
