# freeq Policy & Authority Framework — Demo Walkthrough

This demonstrates freeq's cryptographic channel governance: policy-gated channels with auditable membership, built on standard IRC.

## Prerequisites

- freeq server binary (`cargo build --release --bin freeq-server`)
- A Bluesky account (for authenticated demo)
- Any IRC client (for guest demo)
- `curl` and `python3` (for API inspection)

## Quick Start

```bash
# Terminal 1: Start server
cargo run --release --bin freeq-server -- \
  --listen-addr 127.0.0.1:16799 \
  --web-addr 127.0.0.1:8080

# Terminal 2: Run interactive demo
./scripts/demo.sh
```

The demo script walks you through each scene with pauses. Or follow the manual steps below.

---

## Manual Walkthrough

### 1. Connect a Guest (Standard IRC Client)

Any IRC client works. Using netcat to prove the point:

```
$ nc 127.0.0.1 16799
NICK guest42
USER guest 0 * :Guest
JOIN #general
PRIVMSG #general :Hello from a standard IRC client!
```

This works. Always will. Policy framework doesn't break existing IRC.

### 2. Connect an Authenticated User

Open the web client at `http://127.0.0.1:8080`:

1. Click "Sign in with Bluesky"
2. Complete OAuth flow
3. You're now authenticated with your AT Protocol DID

Or use the TUI client with SASL.

### 3. Create a Channel and Set a Policy

As the authenticated user (who'll be channel op):

```
/join #freeq-dev
```

You're the founder — you have ops. Now set a policy:

```
/POLICY #freeq-dev SET By joining this channel you agree to the freeq Code of Conduct: be respectful, no spam, no harassment. Violations result in removal.
```

Response:
```
Policy set for #freeq-dev (version 1, rules_hash=a1b2c3d4e5f6, policy_id=d4e5f6789abc)
```

You're auto-attested (the op who created the policy is always accepted).

### 4. Guest Tries to Join — Rejected

The guest user tries:

```
JOIN #freeq-dev
```

Response:
```
477 guest42 #freeq-dev :This channel requires authentication — sign in to join
```

Clean rejection. The guest can still use `#general` and any other open channel.

### 5. Second Authenticated User Accepts Policy

A second user signs in with Bluesky, then:

```
/POLICY #freeq-dev INFO
```

Response:
```
Policy for #freeq-dev:
  Version: 1
  Policy ID: d4e5f6789abc...
  Effective: 2026-02-18T15:55:32+00:00
  Validity: JoinTime
  Requirement: ACCEPT(a1b2c3d4e5f6...)
```

Accept the policy:

```
/POLICY #freeq-dev ACCEPT
```

Response:
```
Policy accepted for #freeq-dev — role: member. You may now JOIN.
```

Now join:

```
/join #freeq-dev
```

Success. They're in with a verified badge.

### 6. Inspect the Audit Trail

**Current policy:**
```bash
curl -s http://127.0.0.1:8080/api/v1/policy/%23freeq-dev | python3 -m json.tool
```

```json
{
    "policy": {
        "channel_id": "#freeq-dev",
        "policy_id": "d4e5f6...",
        "version": 1,
        "effective_at": "2026-02-18T15:55:32.123456+00:00",
        "authority_set_hash": "abc123...",
        "requirements": {
            "type": "ACCEPT",
            "hash": "a1b2c3..."
        },
        "validity_model": "join_time",
        "receipt_embedding": "require"
    },
    "authority_set": {
        "channel_id": "#freeq-dev",
        "signers": [{"did": "did:web:127.0.0.1", ...}],
        "policy_threshold": 1
    }
}
```

**Transparency log (privacy-preserving — no DIDs):**
```bash
curl -s http://127.0.0.1:8080/api/v1/policy/%23freeq-dev/transparency | python3 -m json.tool
```

```json
[
    {
        "entry_version": 1,
        "channel_id": "#freeq-dev",
        "policy_id": "d4e5f6...",
        "attestation_hash": "789abc...",
        "issued_at": "2026-02-18T15:56:01.000000+00:00",
        "issuer_authority_id": "did:web:127.0.0.1"
    }
]
```

**Check specific membership:**
```bash
curl -s http://127.0.0.1:8080/api/v1/policy/%23freeq-dev/membership/did:plc:abc123 | python3 -m json.tool
```

**Policy version history:**
```bash
curl -s http://127.0.0.1:8080/api/v1/policy/%23freeq-dev/history | python3 -m json.tool
```

### 7. Update the Policy

```
/POLICY #freeq-dev SET Updated Code of Conduct v2: be respectful, no spam, no harassment, no AI-generated content without disclosure.
```

This creates version 2, chained to version 1. Existing members keep their attestations (JoinTime model). New joiners must accept v2.

### 8. Remove the Policy

```
/POLICY #freeq-dev CLEAR
```

Channel returns to open join. All attestations and logs are removed.

---

## Use Cases

### Open Source Project Governance
```
POLICY #myproject SET <Code of Conduct text>
```
- Contributors accept CoC on join
- Audit trail proves CoC was in place when incident occurred
- Policy updates create version chain — full history preserved

### Role-Based Access via GitHub Org Membership

This is a full working example. A project channel where accepting the Code of Conduct gets you in, but GitHub org members automatically get ops.

**Step 1: Create the channel and set base policy**
```
/join #myproject
/POLICY #myproject SET By contributing to this project you agree to our Code of Conduct: be respectful, inclusive, and constructive.
```

**Step 2: Add role escalation — org members get ops**

First, get the rules hash from POLICY INFO:
```
/POLICY #myproject INFO
→ Requirement: ACCEPT(a1b2c3d4e5f6...)
```

Then set the "op" role requirement (replace the hash):
```
/POLICY #myproject SET-ROLE op {"type":"ALL","requirements":[{"type":"ACCEPT","hash":"a1b2c3d4e5f6...full-hash..."},{"type":"PRESENT","credential_type":"github_membership","issuer":"github"}]}
```

**Step 3: A contributor verifies their GitHub membership**
```
/POLICY #myproject VERIFY github octocat myorg
→ Checking GitHub: is octocat a public member of myorg?
→ ✓ Verified: octocat is a member of myorg. Credential stored.
```

**Step 4: The contributor accepts the policy**
```
/POLICY #myproject ACCEPT
→ Policy accepted for #myproject — role: op. You may now JOIN.
```

They got `op` because they have both the CoC acceptance AND the GitHub credential.

**Step 5: Join — auto +o**
```
/join #myproject
→ (joined with +o automatically)
```

A user without GitHub membership who accepts the same policy gets `member` role — no ops, but they can chat.

**API alternative (for web/mobile):**
```bash
# Verify GitHub membership
curl -X POST http://localhost:8080/api/v1/verify/github \
  -H 'Content-Type: application/json' \
  -d '{"did":"did:plc:abc123","github_username":"octocat","org":"myorg"}'

# Check stored credentials
curl http://localhost:8080/api/v1/credentials/did:plc:abc123

# Submit join with evidence
curl -X POST http://localhost:8080/api/v1/policy/%23myproject/join \
  -H 'Content-Type: application/json' \
  -d '{"subject_did":"did:plc:abc123","accepted_hashes":["a1b2c3..."],"credentials":[{"credential_type":"github_membership","issuer":"github"}]}'
```

- Regular users who accept CoC → `member` role
- GitHub org members who accept CoC → `op` role → automatic +o on join
- No manual op management needed

### Invite-Only Communities
```
POLICY #private SET PRESENT(invitation, issuer=did:plc:channelowner)
```
- Only users with an invitation credential from the owner can join
- Invitation issuance is a separate step (via API)

### Token-Gated Channels
```
POLICY #holders SET PROVE(nft_ownership)
```
- Users must prove ownership of a specific token
- Proof verification is pluggable

### Company Channels
```
POLICY #acme-eng SET ALL(ACCEPT(employee_handbook), PRESENT(email_verified, issuer=acme.com))
```
- Must accept employee handbook AND have verified @acme.com email
- Policy changes cascade to S2S peers

---

## Architecture Notes

### What happens at each layer

| Action | IRC Layer | Policy Layer | Crypto Layer |
|--------|-----------|--------------|--------------|
| `/POLICY SET` | NOTICE reply | PolicyDocument created, versioned | SHA-256 hash chain |
| `/POLICY ACCEPT` | NOTICE reply | Evidence evaluated, JoinReceipt + MembershipAttestation created | HMAC-SHA256 signed attestation |
| `JOIN #channel` | Standard JOIN flow | Attestation checked (or skipped if no policy) | Signature verified |
| Policy update | NOTICE reply | New version chained to previous | Hash chain integrity |
| S2S sync | — | PolicySync message to peers | Attestation signatures portable |

### Requirement DSL

```
ACCEPT { hash }              — User accepted rules document
PRESENT { type, issuer }     — User has credential of type from issuer
PROVE { proof_type }         — User can prove capability
ALL { requirements[] }       — All must be satisfied
ANY { requirements[] }       — At least one must be satisfied
NOT { requirement }          — Must NOT be satisfied
```

Max depth: 8. Max nodes: 64. Deterministic, fail-closed.

### Validity Models

- **JoinTime** — Evaluate once at join. Attestation never expires.
- **Continuous** — Attestation expires (default: 1 hour). Background task revalidates.

### Privacy

- Transparency log entries contain **attestation hashes**, not DIDs
- Membership checks require knowing the specific DID to query
- Log proves *that* attestations were issued, not *to whom*

---

## HTTP API Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/policy/{channel}` | Current policy + authority set |
| GET | `/api/v1/policy/{channel}/history` | Full policy version chain |
| POST | `/api/v1/policy/{channel}/join` | Submit evidence, receive attestation |
| GET | `/api/v1/policy/{channel}/membership/{did}` | Check membership status |
| GET | `/api/v1/policy/{channel}/transparency` | Audit log entries |
| GET | `/api/v1/authority/{hash}` | Authority set by hash |

### POST /api/v1/policy/{channel}/join

```json
{
    "subject_did": "did:plc:abc123",
    "accepted_hashes": ["a1b2c3..."],
    "credentials": [
        {"credential_type": "github_membership", "issuer": "github"}
    ],
    "proofs": ["nft_ownership_proof"]
}
```

Response (success):
```json
{
    "status": "confirmed",
    "join_id": "deadbeef...",
    "attestation": {
        "attestation_id": "...",
        "channel_id": "#freeq-dev",
        "subject_did": "did:plc:abc123",
        "role": "member",
        "issued_at": "2026-02-18T15:56:01Z",
        "signature": "hmac-sha256:..."
    }
}
```

Response (rejected):
```json
{
    "status": "failed",
    "error": "Requirement not satisfied: ACCEPT(a1b2c3...)"
}
```
