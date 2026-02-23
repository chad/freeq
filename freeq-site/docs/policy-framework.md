# Policy & Authority Framework

freeq's Policy Framework adds cryptographic channel governance to IRC. Channel operators can require users to accept rules, verify external credentials, or prove capabilities before joining. Every membership decision is signed, auditable, and federated.

## Overview

Channels without policies work exactly like normal IRC — open join, no restrictions. When a channel operator sets a policy, joining requires satisfying the policy's requirements.

```
/POLICY #project SET By contributing you agree to our Code of Conduct.
```

Now users must accept the policy before they can join:

```
/POLICY #project ACCEPT
→ Policy accepted — role: member. You may now JOIN.

/join #project
→ (success)
```

Guests (unauthenticated users) cannot join policy-gated channels.

## Concepts

### Policy Document

An immutable, versioned JSON document that defines channel requirements. Updates create new versions chained via SHA-256 hashes — a tamper-evident history.

```json
{
  "channel_id": "#project",
  "version": 2,
  "previous_policy_hash": "abc123...",
  "requirements": { "type": "ACCEPT", "hash": "def456..." },
  "role_requirements": {
    "op": {
      "type": "ALL",
      "requirements": [
        { "type": "ACCEPT", "hash": "def456..." },
        { "type": "PRESENT", "credential_type": "github_membership", "issuer": "did:web:..." }
      ]
    }
  },
  "credential_endpoints": {
    "github_membership": {
      "issuer": "did:web:irc.freeq.at:verify",
      "url": "/verify/github/start",
      "label": "Verify with GitHub"
    }
  }
}
```

### Requirement DSL

Composable rules for channel access:

| Type | Description | Example |
|------|-------------|---------|
| `ACCEPT` | User must accept a rules document | Code of Conduct |
| `PRESENT` | User must have a verified credential | GitHub org membership |
| `PROVE` | User must prove a capability | Domain ownership |
| `ALL` | All sub-requirements must be met | CoC AND GitHub |
| `ANY` | At least one sub-requirement | Email OR GitHub |
| `NOT` | Sub-requirement must NOT be met | Not banned |

Max depth: 8 levels. Max nodes: 64. Deterministic, fail-closed evaluation.

### Verifiable Credentials

Portable, signed credentials issued by external verifiers. The freeq server never talks to GitHub, email providers, or any external service — it only verifies Ed25519 signatures.

```
User → External Verifier → GitHub OAuth → Signed credential
User → freeq server → Verify signature → Accept credential
```

A credential looks like:

```json
{
  "type": "FreeqCredential/v1",
  "issuer": "did:web:irc.freeq.at:verify",
  "subject": "did:plc:abc123",
  "credential_type": "github_membership",
  "claims": { "github_username": "chad", "org": "freeq" },
  "issued_at": "2026-02-18T...",
  "expires_at": "2026-03-18T...",
  "signature": "Ed25519-base64url..."
}
```

Anyone can run a verifier. The server just checks the signature against the issuer's DID document.

### Membership Attestation

When a user satisfies all requirements, the server issues a signed attestation proving their membership and role. This attestation is what gates the IRC JOIN command.

### Transparency Log

Every attestation issuance is logged in a privacy-preserving audit trail. Log entries contain attestation hashes, not user DIDs — you can prove *that* attestations were issued without revealing *to whom*.

## IRC Commands

```
POLICY #channel SET <rules text>                    — Create/update policy (ops)
POLICY #channel SET-ROLE <role> <requirement json>  — Add role escalation (ops)
POLICY #channel REQUIRE <type> issuer=<did> url=<url> label=<text> — Add credential endpoint (ops)
POLICY #channel INFO                                — View current policy
POLICY #channel ACCEPT                              — Accept policy + present credentials
POLICY #channel VERIFY github <org>                 — Start GitHub verification
POLICY #channel CLEAR                               — Remove policy (ops)
```

## HTTP API

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/policy/{channel}` | Current policy + authority set |
| GET | `/api/v1/policy/{channel}/history` | Full version chain |
| POST | `/api/v1/policy/{channel}/join` | Submit evidence, receive attestation |
| POST | `/api/v1/policy/{channel}/check` | Personalized requirements checklist |
| GET | `/api/v1/policy/{channel}/membership/{did}` | Check membership |
| GET | `/api/v1/policy/{channel}/transparency` | Audit log |
| POST | `/api/v1/credentials/present` | Submit external credential |
| GET | `/api/v1/credentials/{did}` | List stored credentials |

### Personalized requirements check

```bash
curl -X POST https://irc.freeq.at/api/v1/policy/%23project/check \
  -H 'Content-Type: application/json' \
  -d '{"did": "did:plc:abc123"}'
```

Returns per-requirement status with action URLs:

```json
{
  "channel": "#project",
  "can_join": false,
  "status": "unsatisfied",
  "requirements": [
    {
      "requirement_type": "accept",
      "description": "Accept the channel rules",
      "satisfied": false,
      "action": { "action_type": "accept_rules", "label": "Accept Rules", "accept_hash": "abc..." }
    },
    {
      "requirement_type": "present",
      "description": "Credential: github_membership",
      "satisfied": false,
      "action": { "action_type": "verify_external", "url": "/verify/github/start?subject_did=...", "label": "Verify with GitHub" }
    }
  ]
}
```

## GitHub Verification Example

### Step 1: Set up the policy

```
/join #myproject
/POLICY #myproject SET By contributing you agree to our Code of Conduct.
```

### Step 2: Add GitHub role escalation

Get the rules hash from `/POLICY #myproject INFO`, then:

```
/POLICY #myproject SET-ROLE op {"type":"ALL","requirements":[{"type":"ACCEPT","hash":"FULL_HASH"},{"type":"PRESENT","credential_type":"github_membership","issuer":"did:web:irc.freeq.at:verify"}]}
```

### Step 3: Add the credential endpoint

```
/POLICY #myproject REQUIRE github_membership issuer=did:web:irc.freeq.at:verify url=/verify/github/start label=Verify_with_GitHub
```

### Step 4: Users verify and join

1. User tries `/join #myproject` → rejected, told to accept policy
2. User runs `/POLICY #myproject VERIFY github myorg` → gets OAuth URL
3. User clicks URL → GitHub OAuth → org membership verified → credential stored
4. User runs `/POLICY #myproject ACCEPT` → credentials auto-included → role: op
5. User runs `/join #myproject` → joined with +o

Users without GitHub membership get `member` role (no ops).

## Building Custom Verifiers

Any HTTP service can be a credential verifier. Requirements:

1. **Serve a DID document** at `/.well-known/did.json` with your Ed25519 public key
2. **Verify whatever you want** (GitHub, email, DNS, NFT, employee directory)
3. **Sign a `FreeqCredential/v1`** with your Ed25519 key
4. **POST it to the callback URL** the client provides

The freeq server resolves your DID, extracts your public key, and verifies the signature. Zero coupling — your verifier never needs to know about freeq.

See the reference implementation at `freeq-server/src/verifiers/github.rs`.

## Architecture

```
Channel Op                    freeq server               External Verifier
    │                              │                           │
    │ POLICY SET + REQUIRE         │                           │
    │─────────────────────────────>│                           │
    │                              │                           │
    │                        Stores policy with               │
    │                        credential_endpoints              │
    │                              │                           │
User                               │                           │
    │ /join #channel               │                           │
    │─────────────────────────────>│                           │
    │ 477: policy acceptance needed│                           │
    │<─────────────────────────────│                           │
    │                              │                           │
    │ GET /api/v1/policy/check     │                           │
    │─────────────────────────────>│                           │
    │ requirements + action URLs   │                           │
    │<─────────────────────────────│                           │
    │                              │                           │
    │ Open verifier URL ──────────────────────────────────────>│
    │                              │          GitHub OAuth      │
    │                              │      Verify membership    │
    │                              │      Sign credential      │
    │                              │<──────── POST credential  │
    │                              │                           │
    │ POLICY ACCEPT                │                           │
    │─────────────────────────────>│                           │
    │                        Evaluate requirements             │
    │                        Issue attestation                 │
    │ role: op                     │                           │
    │<─────────────────────────────│                           │
    │                              │                           │
    │ /join #channel               │                           │
    │─────────────────────────────>│                           │
    │ Joined with +o               │                           │
    │<─────────────────────────────│                           │
```

## Web UI

The web client has built-in UI for policy management:

### JoinGateModal (for users)

When you try to join a policy-gated channel, a modal appears showing:

- All requirements with ✅/❌ status
- Action buttons for each requirement (Accept Rules, Verify with GitHub, Follow on Bluesky)
- "Join Channel" button that enables when all requirements are met
- Automatic re-check after completing verification

### Channel Settings Panel (for ops)

Click the ⚙️ gear icon in any channel's top bar:

- **Rules tab**: Set or update the channel policy (accept-rules, custom JSON)
- **Verifiers tab**: Add credential verifiers from presets:
  - 🐙 GitHub Repo Collaborator
  - 🏢 GitHub Org Member
  - 🦋 Bluesky Follower
- **Roles tab**: Assign auto-op or auto-voice based on credential type

Non-ops see a read-only view of the current policy.

## Quick start examples

### Bluesky follower gate (no config needed)

```
/POLICY #vip SET accept-rules
/POLICY #vip REQUIRE bluesky_follower issuer=did:web:irc.freeq.at:verify url=/verify/bluesky/start?target=yourhandle.bsky.social label=Follow_on_Bluesky
```

### GitHub org gate

```
/POLICY #team SET accept-rules
/POLICY #team REQUIRE github_membership issuer=did:web:irc.freeq.at:verify url=/verify/github/start?org=yourorg label=Verify_GitHub
```

### Auto-op for GitHub repo collaborators

```
/POLICY #project SET accept-rules
/POLICY #project SET-ROLE op {"type":"PRESENT","credential_type":"github_repo","issuer":"did:web:irc.freeq.at:verify"}
/POLICY #project REQUIRE github_repo issuer=did:web:irc.freeq.at:verify url=/verify/github/start?repo=owner/repo label=Verify_Repo_Access
```
