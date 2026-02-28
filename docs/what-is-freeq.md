# What is freeq?

freeq is an IRC server with modern identity built on the [AT Protocol](https://atproto.com/) (the protocol behind Bluesky).

## The problem

Every chat platform makes you create a new account. Your identity is locked to the platform. Your messages belong to them. If they shut down or change the rules, you lose everything.

IRC solved the protocol problem decades ago — but it never solved identity. You're just a nickname, trivially impersonated.

## What freeq does differently

freeq keeps IRC's open protocol and adds cryptographic identity:

- **Your identity is yours.** Authenticate with your Bluesky/AT Protocol DID. Your identity works across any freeq server.
- **Messages are signed.** Every message from an authenticated user carries a cryptographic signature. No impersonation.
- **End-to-end encryption.** DMs use X3DH key agreement and Double Ratchet for forward secrecy.
- **Policy, not power.** Channel access rules are expressed as verifiable credentials — transparent and auditable.
- **Any IRC client works.** Standard IRC clients connect as guests. No lock-in.

## How it works

1. Connect with any IRC client, the [web app](https://irc.freeq.at), or the iOS app
2. Optionally authenticate with your Bluesky account (OAuth — no passwords sent to freeq)
3. Your DID becomes your identity. Your nick is a display alias.
4. Messages you send are cryptographically signed with your session key
5. Channel policies can gate access based on verifiable credentials (GitHub org membership, Bluesky follows, etc.)

## What it's not

- Not a Bluesky client (though it uses Bluesky identity)
- Not a replacement for Slack/Discord (it's infrastructure, not a product)
- Not a blockchain thing (DIDs are decentralized identifiers, not tokens)

## Tech stack

- **Server**: Rust, async (tokio), SQLite
- **Web client**: React + TypeScript + Vite
- **iOS app**: SwiftUI + Rust SDK via FFI
- **SDK**: Rust, with bot framework
- **Federation**: Server-to-server via iroh QUIC with CRDT convergence
- **Identity**: AT Protocol DIDs, SASL ATPROTO-CHALLENGE
