# Start Here

## What is freeq?

freeq is IRC with modern identity. Your Bluesky account is your chat identity —
portable, cryptographically verifiable, and not locked to any platform.

Messages are signed. DMs are end-to-end encrypted. Channels can gate access
with verifiable credentials. Standard IRC clients still work. No lock-in.

**Try it now:** [irc.freeq.at](https://irc.freeq.at)

## Why does this exist?

Every chat platform makes you create a new account. Your identity belongs to
them. IRC solved the protocol problem decades ago but never solved identity —
you're just a nickname, trivially impersonated.

freeq keeps IRC's open protocol and adds what was missing:

- **Cryptographic identity** via AT Protocol DIDs
- **Signed messages** that can't be forged
- **End-to-end encrypted DMs** with forward secrecy
- **Credential-gated channels** (GitHub org member? Bluesky follower? Prove it.)
- **Federation** between independent servers

## Quickstarts

| I want to... | Go here |
|---|---|
| Chat in my browser | [irc.freeq.at](https://irc.freeq.at) |
| Connect with an IRC client | [Getting Started](getting-started.md) |
| Run my own server | [Self-Hosting Guide](self-hosting.md) |
| Build a bot | [Bot Quickstart](BOT-QUICKSTART.md) |
| Understand the protocol | [Protocol Spec](PROTOCOL.md) |
| Set up federation | [Federation](federation.md) |
| Harden a production deploy | [Security Guide](SECURITY.md) |
| See all features | [Feature List](Features.md) |
| Know what's not done yet | [Known Limitations](KNOWN-LIMITATIONS.md) |

## Architecture at a glance

```
freeq-server/       Rust IRC server (SASL, WebSocket, S2S, REST API)
freeq-app/          React web client (irc.freeq.at)
freeq-sdk/          Rust client SDK + bot framework
freeq-tui/          Terminal client
freeq-auth-broker/  AT Protocol OAuth broker
freeq-site/         Marketing site (freeq.at)
```

Transports: TCP · TLS · WebSocket · iroh QUIC
Identity: AT Protocol DIDs · SASL ATPROTO-CHALLENGE
Encryption: ed25519 signatures · AES-256-GCM channels · X3DH + Double Ratchet DMs
Federation: iroh QUIC · Automerge CRDTs · eventual consistency

## License

[MIT](../LICENSE)
