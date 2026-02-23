# Credential Verifiers

freeq's Policy Framework requires external credentials for channel access control. Credential verifiers are standalone services that verify real-world identity claims and issue signed Verifiable Credentials. The freeq server never contacts external services — it only verifies Ed25519 signatures.

## Architecture

```
User ──→ External Verifier ──→ GitHub / Email / DNS / etc.
                │
                │ Signs FreeqCredential/v1
                │
                ↓
User ──→ freeq server ──→ Resolves issuer DID
                         ──→ Extracts Ed25519 public key
                         ──→ Verifies signature
                         ──→ Stores credential
```

The freeq server has **zero** provider-specific API keys. No GitHub tokens, no email SMTP, no DNS API keys. All of that lives in the verifier.

## Built-in GitHub verifier

freeq ships with a GitHub organization membership verifier hosted at `/verify/`. This is architecturally separate from the core server — it just happens to run in the same process.

### How it works

1. Channel op adds a `REQUIRE` to the policy:
   ```
   /POLICY #channel REQUIRE github_membership issuer=did:web:irc.freeq.at:verify url=/verify/github/start label=Verify_with_GitHub
   ```

2. User starts verification:
   ```
   /POLICY #channel VERIFY github myorg
   ```
   Server returns a URL like: `/verify/github/start?org=myorg&subject_did=did:plc:abc123&callback=/api/v1/credentials/present`

3. User clicks the URL → GitHub OAuth → authorizes → org membership checked

4. Verifier signs a `FreeqCredential/v1` and POSTs it to the callback URL

5. User runs `/POLICY #channel ACCEPT` → credential is auto-included → access granted

### Server configuration

Set environment variables:

```bash
export GITHUB_CLIENT_ID=your_github_oauth_app_id
export GITHUB_CLIENT_SECRET=your_github_oauth_app_secret
```

The GitHub OAuth app needs:
- **Authorization callback URL**: `https://your-server.com/verify/github/callback`
- **Scopes**: `read:org` (for org membership verification)

### DID document

The verifier serves its DID document at `/verify/.well-known/did.json`:

```json
{
  "@context": "https://www.w3.org/ns/did/v1",
  "id": "did:web:irc.freeq.at:verify",
  "verificationMethod": [{
    "id": "did:web:irc.freeq.at:verify#signing-key",
    "type": "Ed25519VerificationKey2020",
    "controller": "did:web:irc.freeq.at:verify",
    "publicKeyMultibase": "z6Mk..."
  }],
  "authentication": ["did:web:irc.freeq.at:verify#signing-key"],
  "assertionMethod": ["did:web:irc.freeq.at:verify#signing-key"]
}
```

## Building a custom verifier

Any HTTP service can issue credentials. Here's the minimal interface:

### 1. Generate an Ed25519 keypair

```rust
use ed25519_dalek::SigningKey;
let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
```

### 2. Serve a DID document

Host `/.well-known/did.json` with your public key encoded as multibase (z-prefix base58btc with 0xed01 multicodec prefix) or JWK.

### 3. Verify whatever you want

Your verification logic is completely up to you:

- **GitHub**: OAuth + org membership check
- **Email**: Send verification email with magic link
- **DNS**: Check TXT record for DID
- **Corporate directory**: LDAP/SAML lookup
- **Government ID**: IDV provider integration
- **NFT ownership**: On-chain lookup
- **Anything else**: Your verifier, your rules

### 4. Sign a credential

```json
{
  "type": "FreeqCredential/v1",
  "issuer": "did:web:your-verifier.com",
  "subject": "did:plc:user-did",
  "credential_type": "github_membership",
  "claims": {
    "github_username": "chad",
    "org": "freeq"
  },
  "issued_at": "2026-02-18T00:00:00Z",
  "expires_at": "2026-03-18T00:00:00Z",
  "signature": ""
}
```

Sign with JCS (RFC 8785) canonicalization:

1. Serialize to JSON with `signature: ""`
2. Canonicalize per RFC 8785 (sorted keys, no whitespace)
3. Sign the canonical bytes with Ed25519
4. Set `signature` to base64url-encoded signature

### 5. POST to the callback

```bash
POST /api/v1/credentials/present
Content-Type: application/json

{
  "credential": { ... signed credential ... }
}
```

The freeq server will:
1. Resolve your DID document
2. Extract the Ed25519 public key
3. Verify the signature over the JCS-canonical form
4. Store the credential if valid

### Reference implementation

See `freeq-server/src/verifiers/github.rs` for a complete working example, or `freeq-server/src/bin/credential-issuer.rs` for a standalone issuer binary.

## Endpoint namespacing

Verifiers on the freeq server live under `/verify/`:

| Endpoint | Description |
|----------|-------------|
| `/verify/.well-known/did.json` | Verifier DID document |
| `/verify/github/start` | Start GitHub OAuth flow |
| `/verify/github/callback` | GitHub OAuth callback |

Future verifiers would add their own namespaces:

| Planned | Namespace |
|---------|-----------|
| Email verification | `/verify/email/` |
| DNS ownership | `/verify/dns/` |
| Corporate SSO | `/verify/saml/` |

Each verifier is independent — different channels can trust different issuers.
