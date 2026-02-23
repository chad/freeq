# Authentication

freeq uses the AT Protocol (Bluesky's identity layer) for authentication. Your DID — a globally unique, cryptographically verifiable identifier — becomes your IRC identity.

## How it works

### Web client flow

1. Click **Sign in with Bluesky**
2. AT Protocol OAuth popup opens
3. Authorize the freeq application
4. OAuth callback generates a one-time `web-token`
5. Token is used for SASL authentication over WebSocket
6. Your Bluesky handle becomes your IRC nick

### IRC client flow (SASL)

freeq implements `ATPROTO-CHALLENGE`, a custom SASL mechanism:

```
Client                          Server
  │                                │
  │ CAP REQ :sasl                  │
  │───────────────────────────────>│
  │ CAP ACK :sasl                  │
  │<───────────────────────────────│
  │                                │
  │ AUTHENTICATE ATPROTO-CHALLENGE │
  │───────────────────────────────>│
  │ AUTHENTICATE +                 │
  │<───────────────────────────────│
  │                                │
  │ AUTHENTICATE <base64 json>     │
  │  { "method": "crypto",        │
  │    "did": "did:plc:...",       │
  │    "handle": "alice.bsky.app"} │
  │───────────────────────────────>│
  │                                │
  │ AUTHENTICATE <base64 challenge>│
  │  { "challenge": "...",         │
  │    "session_id": "...",        │
  │    "nonce": "...",             │
  │    "timestamp": "..." }        │
  │<───────────────────────────────│
  │                                │
  │ AUTHENTICATE <base64 response> │
  │  { "signature": "base64url",   │
  │    "session_id": "..." }       │
  │───────────────────────────────>│
  │                                │
  │ 903 :SASL authentication       │
  │     successful                 │
  │<───────────────────────────────│
```

### Challenge security

- **`session_id`** — Unique per TCP connection
- **`nonce`** — Cryptographically random
- **Timestamp** — Valid for ≤ 60 seconds
- **Single use** — Challenge invalidated after use

### Signature verification

The server:

1. Resolves the DID document
2. Extracts the `authentication` verification key
3. Verifies the Ed25519 or secp256k1 signature over the exact challenge bytes

Private keys **never** leave the client.

## Guest mode

Clients that don't request SASL connect as guests. Guest nicks are randomly generated (`Guest42817`). Guests can:

- Join open channels
- Send and receive messages
- Use all basic IRC features

Guests cannot:

- Join policy-gated channels
- Upload media
- Have persistent nick ownership

## DID-based nick ownership

When you authenticate, your DID is bound to your connection. If you disconnect and reconnect with the same DID, the server automatically ghosts the old connection and restores your nick.

DID-based channel ops survive server restarts (stored in the database). NickServ-style registration is unnecessary.

## Web tokens

For the web client, OAuth produces a one-time cryptographic token:

1. OAuth callback generates a 32-byte random token
2. Token is stored server-side with the user's DID and a 5-minute expiry
3. Web client receives the token via `postMessage`
4. Client uses the token for SASL authentication
5. Token is consumed on use — cannot be replayed

This avoids sending AT Protocol credentials over the WebSocket connection.
