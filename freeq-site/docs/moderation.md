# Moderation

Freeq's moderation system is built on the same **decoupled credential architecture** that powers all channel policies. Moderators are appointed via signed credentials — the freeq server never contains moderation logic itself, it just verifies signatures and maps roles to IRC modes.

## How It Works

### The Basics

1. A **channel operator** sets a moderation policy on their channel
2. A **moderation service** issues signed credentials to appointed moderators
3. When a moderator joins the channel, the server verifies the credential and grants **halfop (+h)** status
4. Halfops can kick, ban, and voice users — but cannot change channel settings or affect other moderators/operators

### What Halfops Can Do

| Action | Halfop (+h) | Op (+o) |
|--------|:-----------:|:-------:|
| Send messages | ✅ | ✅ |
| Send in +m (moderated) channels | ✅ | ✅ |
| Kick regular users | ✅ | ✅ |
| Ban regular users | ✅ | ✅ |
| Voice/unvoice users (+v) | ✅ | ✅ |
| Kick other halfops | ❌ | ✅ |
| Kick operators | ❌ | ❌¹ |
| Set channel modes (+m, +t, +i) | ❌ | ✅ |
| Grant +o or +h | ❌ | ✅ |
| Change channel policy | ❌ | ✅ |

¹ *Founders cannot be de-opped by anyone.*

### In the Member List

Moderators appear with a **%** prefix in NAMES and the member list:

```
@ChannelOwner    ← operator
%ModeratorNick   ← halfop/moderator
+VoicedUser      ← voiced
RegularUser      ← regular member
```

## Setting Up Moderation

### Using the Built-in Moderation Service

Freeq ships with a moderation verifier at `/verify/mod/`. To use it:

**1. Set a channel policy with the moderator role:**

```
/policy #yourchannel SET-ROLE moderator {"type":"PRESENT","credential_type":"channel_moderator"}
```

Or use the **Channel Settings** panel in the web app:
- Click the ⚙️ gear icon in the channel top bar
- Go to the **Roles** tab
- Select "Moderator (+h)" role
- Select "channel_moderator" credential type
- Click "Add Role"

**2. Appoint moderators:**

Visit the appointment page:
```
https://your-server/verify/mod/start?channel=%23yourchannel&appointer_did=did:plc:your-did
```

Or in the web app:
- Go to **Requirements** tab in Channel Settings
- Select the "Moderator" verifier preset
- Enter the DID or handle of the person to appoint
- Choose a duration (7 days to 1 year)

**3. Moderator presents credential:**

When the appointed user runs `POLICY ACCEPT` (or the web app handles it automatically), their `channel_moderator` credential is verified and they receive +h on join.

### Quick Setup via IRC

```irc
/policy #mychannel SET-ROLE moderator {"type":"PRESENT","credential_type":"channel_moderator"}
```

Then appoint someone via the web UI or API:

```bash
curl -X POST https://your-server/verify/mod/appoint \
  -d "subject=did:plc:moderator-did" \
  -d "channel=%23mychannel" \
  -d "appointer_did=did:plc:your-did" \
  -d "duration=30" \
  -d "callback=https://your-server/api/v1/credentials/present"
```

### Revoking a Moderator

```bash
curl -X POST https://your-server/verify/mod/revoke \
  -H "Content-Type: application/json" \
  -d '{"subject_did":"did:plc:moderator-did","channel":"#mychannel"}'
```

Revoked credentials are caught by the server's **revalidation sweep** (runs every 60 seconds). The moderator's +h is removed automatically.

### Viewing the Roster

```bash
curl https://your-server/verify/mod/roster?channel=%23mychannel
```

Returns:
```json
{
  "channel": "#mychannel",
  "moderators": [
    {
      "subject_did": "did:plc:abc123",
      "appointed_by": "did:plc:channel-owner",
      "appointed_at": "2026-02-18T08:00:00Z",
      "expires_at": "2026-03-20T08:00:00Z",
      "revoked": false
    }
  ]
}
```

## Building a Third-Party Moderation Service

The built-in moderation service is convenient, but any service can issue `channel_moderator` credentials. The freeq server only checks:

1. The credential has `credential_type: "channel_moderator"`
2. The issuer DID matches the one configured in the channel policy
3. The Ed25519 signature is valid
4. The credential hasn't expired

### Credential Format

```json
{
  "type": "FreeqCredential/v1",
  "issuer": "did:web:your-mod-service.com",
  "subject": "did:plc:the-moderator",
  "credential_type": "channel_moderator",
  "claims": {
    "channel": "#yourchannel",
    "appointed_by": "did:plc:channel-owner",
    "powers": ["kick", "ban", "voice", "mute"]
  },
  "issued_at": "2026-02-18T08:00:00Z",
  "expires_at": "2026-03-20T08:00:00Z",
  "signature": "base64url-ed25519-signature"
}
```

### Required Components

Your moderation service needs:

**1. A DID document** at `/.well-known/did.json` with your Ed25519 public key:

```json
{
  "@context": ["https://www.w3.org/ns/did/v1", "https://w3id.org/security/multikey/v1"],
  "id": "did:web:your-mod-service.com",
  "verificationMethod": [{
    "id": "did:web:your-mod-service.com#key-1",
    "type": "Multikey",
    "controller": "did:web:your-mod-service.com",
    "publicKeyMultibase": "z6Mk..."
  }],
  "assertionMethod": ["did:web:your-mod-service.com#key-1"]
}
```

**2. An appointment endpoint** that issues signed credentials (see [Verifiable Credentials](/docs/verifiers) for signing details).

**3. A callback mechanism** — when a credential is issued, POST it to the freeq server:

```bash
POST https://freeq-server/api/v1/credentials/present
Content-Type: application/json

{
  "credential": { ... the signed VC ... }
}
```

**4. (Optional) A revocation endpoint** — the server's 60-second revalidation sweep will call your DID document to re-verify; if you rotate your signing key, old credentials become invalid.

### Signing Credentials

```python
# Python example using ed25519
import json
import base64
import hashlib
from nacl.signing import SigningKey

def sign_credential(credential, private_key_bytes):
    # Set signature to empty for signing
    credential["signature"] = ""
    
    # JCS canonicalization (sorted keys, no whitespace)
    canonical = json.dumps(credential, sort_keys=True, separators=(',', ':'))
    
    # SHA-256 hash
    digest = hashlib.sha256(canonical.encode()).digest()
    
    # Ed25519 sign
    signing_key = SigningKey(private_key_bytes)
    signature = signing_key.sign(digest).signature
    
    # Base64url encode (unpadded)
    credential["signature"] = base64.urlsafe_b64encode(signature).rstrip(b'=').decode()
    return credential
```

### Configuring the Channel

The channel operator references your service's issuer DID:

```
/policy #channel SET-ROLE moderator {"type":"PRESENT","credential_type":"channel_moderator","issuer":"did:web:your-mod-service.com"}
```

That's it. The freeq server resolves `did:web:your-mod-service.com`, extracts your public key, and verifies any `channel_moderator` credentials signed by it.

### Example: Organization-Specific Moderation

A company could run their own moderation service that:

1. Authenticates employees via SSO/SAML
2. Lets team leads appoint moderators for their project channels
3. Auto-revokes when someone leaves the company
4. Issues short-lived credentials (e.g., 24 hours) that auto-renew

The freeq server never knows about your SSO, your org chart, or your HR system. It just sees valid signatures.

### Example: Community Election

A community could build a moderation service that:

1. Holds periodic elections for moderator positions
2. Issues time-limited credentials to winners
3. Publishes the roster publicly for transparency
4. Integrates with voting systems (ranked choice, etc.)

### Example: Reputation-Based

A moderation service could:

1. Track user behavior across channels
2. Auto-issue moderator credentials to trusted long-time members
3. Revoke credentials if behavior degrades
4. Use a scoring algorithm (message count, upvotes, tenure)

## Architecture

```
┌──────────────────┐     ┌─────────────────────┐
│  Channel Owner   │     │  Moderation Service  │
│  (sets policy)   │     │  (issues credentials)│
└────────┬─────────┘     └──────────┬──────────┘
         │                          │
         │ POLICY SET-ROLE          │ Signs VCs with
         │ moderator ...            │ Ed25519 key
         │                          │
         ▼                          ▼
┌──────────────────────────────────────────────┐
│              freeq server                     │
│                                               │
│  1. Receives credential via /credentials/present │
│  2. Resolves issuer DID → gets public key     │
│  3. Verifies Ed25519 signature                │
│  4. Checks credential_type matches policy     │
│  5. Maps "moderator" role → +h mode           │
│  6. Revalidates every 60 seconds              │
│                                               │
│  Zero moderation logic. Zero API keys.        │
│  Zero knowledge of how moderators are chosen. │
└──────────────────────────────────────────────┘
```

## FAQ

**Q: Can I use the built-in service for some channels and a custom one for others?**

Yes. Each channel's policy specifies its own issuer DID. Channel A can use the built-in service, channel B can use your company's service, channel C can use a community-run service.

**Q: What happens if the moderation service goes down?**

Existing credentials continue to work until they expire. New appointments can't be made until the service is back. The freeq server is not affected.

**Q: Can a moderator be both halfop and op?**

If someone has both a `channel_moderator` credential (→ +h) and an `op` role credential (→ +o), they get +o (the higher privilege wins).

**Q: How do I remove all moderation from a channel?**

```
/policy #channel CLEAR
```

This removes all policies, credentials, and attestations. Moderators lose +h immediately.

**Q: Is the moderator roster public?**

The built-in service exposes `/verify/mod/roster?channel=...` as a public read-only endpoint. Third-party services can choose whether to make their rosters public.
