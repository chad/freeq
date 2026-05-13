# freeq-bot-id

CLI utility for minting and managing freeq bot identities.

This is the **Rust-side identity utility**: it generates the ed25519 keypair (did:key or did:web), persists the seed at `~/.freeq/bots/<name>/key.ed25519` with mode 0600, and optionally writes a signed delegation certificate binding the bot to a creator DID.

## Who uses this

- **Rust bot authors** — your typical setup is a one-shot `freeq-bot-id create --name X`, then your bot reads the persisted seed in code via `freeq_sdk::auth::KeySigner::from_seed(...)`. The Rust SDK has the cryptographic primitives (`PrivateKey::generate_ed25519`, `PrivateKey::ed25519_from_bytes`) but no "load-or-create at the right path with the right perms" helper — this CLI is that helper.

- **TypeScript bot authors** — you don't need this. [`@freeq/bot-kit`](../freeq-bot-kit-js/)'s `FreeqBot.create({name, ownerDid, ...})` handles identity persistence internally on first run. The on-disk layout matches what `freeq-bot-id` writes, so a Rust bot and a TS bot can interoperate on the same keys.

## Install

```bash
cargo install --path freeq-bot-id
```

## Subcommands

```bash
# Mint a fresh bot identity (writes seed + DID document under ~/.freeq/bots/<name>/)
freeq-bot-id create --name myagent

# Quick did:key one-liner (no delegation, just generates and prints)
freeq-bot-id did-key --name myagent

# Inspect an existing identity
freeq-bot-id info --name myagent
```

For did:web identities (org-scoped), pass `--domain example.com` to `create`. For signed delegation certs (v1.1 format), pass `--creator-did` and `--creator-key`.

## File layout

```
~/.freeq/bots/<name>/
├── key.ed25519        # 32-byte seed (mode 0600)
├── did-document.json  # DID document (did:web only)
└── delegation.json    # FreeqBotDelegation/v1 cert (when --creator-did given)
```

## Scope

Compatibility with `@freeq/bot-kit`'s file layout is intentional. TS bots created via `FreeqBot.create({name: 'X'})` write files at `~/.freeq/bots/X/agent.key` and `~/.freeq/bots/X/delegation.json`; the seed format and cert schema are interchangeable with what this CLI produces. A bot can be moved from Rust to TS (or vice versa) without re-minting its DID.

## See also

- [`@freeq/bot-kit`](../freeq-bot-kit-js/) — TS bot framework that subsumes this utility for TS workflows
- [`freeq-sdk`](../freeq-sdk/) — Rust SDK whose `auth::KeySigner` consumes the seed file this writes
- [`docs/agents.md`](../docs/agents.md) — full agent-protocol reference, including identity + provenance details
