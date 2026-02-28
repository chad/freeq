# Authentication

freeq uses the AT Protocol (Bluesky) for identity. Authentication is optional — unauthenticated users connect as guests with standard IRC features.

## How it works

1. **Client requests SASL** during IRC capability negotiation
2. **Server offers `ATPROTO-CHALLENGE`** mechanism
3. **Client proves DID ownership** via one of:
   - **OAuth token** (web/iOS): Browser-based Bluesky login → broker mints a web-token
   - **PDS session** (TUI/CLI): App password or cached OAuth session → signs challenge via PDS
   - **Crypto key** (bots): Direct ed25519/secp256k1 key → signs challenge directly
4. **Server verifies** against the user's DID document
5. **Connection bound to DID** — nick is a display alias, identity is cryptographic

## Web & iOS (OAuth flow)

```
User → auth.freeq.at/auth/login → Bluesky OAuth popup
    → broker gets PDS token → mints web-token → pushes to server
    → client sends web-token via SASL → authenticated
```

No passwords are sent to freeq. The broker talks to the user's PDS (Personal Data Server) via standard AT Protocol OAuth.

## TUI & CLI

```bash
# OAuth (opens browser)
freeq-tui --handle alice.bsky.social

# App password (legacy)
freeq-tui --handle alice.bsky.social --app-password xxx-xxxx-xxxx
```

Sessions are cached at `~/.config/freeq/` and reused across connections.

## Bots (direct key signing)

```bash
freeq-tui --did did:plc:abc123 --key-file key.hex --key-type ed25519
```

Or use the SDK's `KeySigner`:

```rust
let signer = KeySigner::new(did, private_key);
let conn = establish_connection(&config).await?;
let (handle, events) = connect_with_stream(conn, config, Some(Arc::new(signer)));
```

## What authentication gives you

| Feature | Guest | Authenticated |
|---|---|---|
| Chat in channels | ✅ | ✅ |
| Message signing | ❌ | ✅ (automatic) |
| E2EE DMs | ❌ | ✅ |
| Policy-gated channels | ❌ | ✅ |
| Media uploads | ❌ | ✅ (via PDS) |
| Hostname cloaking | `freeq/guest` | `freeq/plc/xxxxxxxx` |
| Multi-device | ❌ | ✅ (same DID) |
| Ghost grace period | ❌ | ✅ (30s reconnect window) |

## Key types

- **ed25519** (recommended): Fast, compact signatures
- **secp256k1**: Compatible with Bitcoin/Ethereum key infrastructure

## Security properties

- Private keys **never** leave the client
- Challenges are single-use with 60-second expiry
- Nonces are cryptographically random
- SASL limited to 3 failures before disconnect
