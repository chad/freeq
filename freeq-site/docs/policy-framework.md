# Policy Framework

freeq's policy system lets channel operators define access rules using verifiable credentials. Instead of "trust the op," access decisions are transparent and auditable.

## How it works

1. Channel operator sets a **policy** — a set of rules
2. Rules reference **credential types** (e.g., "GitHub org member," "Bluesky follower")
3. Users who meet the rules are issued **verifiable credentials** by the server's verifier
4. Credentials are checked on JOIN — if you have the right credential, you're in

## Example

```
POLICY #dev SET REQUIRE github:org:mycompany
```

This means: to join `#dev`, you must prove you're a member of the `mycompany` GitHub org. The server's GitHub verifier checks this via OAuth and issues a credential.

## Policy DSL

```
POLICY #channel SET REQUIRE <credential-type>
POLICY #channel SET REQUIRE ACCEPT-RULES
POLICY #channel SET REQUIRE <type1> AND <type2>
POLICY #channel INFO
POLICY #channel CLEAR
```

## Credential types

| Type | What it checks |
|---|---|
| `github:org:<name>` | GitHub organization membership |
| `github:repo:<owner/repo>` | GitHub repository collaborator |
| `accept-rules` | User clicked "I accept" on channel rules |

More types planned: Bluesky follows, domain handle, minimum follower count.

## Web UI

The web client has a visual policy editor in Channel Settings:

- **Templates** — One-click setup for common patterns (Code of Conduct, GitHub Contributors, Bluesky Community)
- **Rules text** — Markdown rules that users must accept
- **Credential management** — See active verifiers and assign roles

## DID ops bypass

Users listed as DID operators for a channel always bypass policy checks. This prevents founders from being locked out of their own channels.

## Architecture

- **Policy store** — SQLite database (`irc-policy.db`)
- **Credential verifier** — DID-based (`did:web:irc.freeq.at:verify`)
- **Credential format** — Signed JSON with issuer DID, subject DID, credential type, expiry
- **Signing key** — Persistent ed25519 key at `verifier-signing-key.secret`

See [Policy System](/docs/policy-system/) for the full technical reference.
