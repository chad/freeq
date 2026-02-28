# Credential Verifiers

Verifiers are services that check real-world claims and issue cryptographic credentials. freeq's policy system uses these credentials to gate channel access.

## Built-in: GitHub Verifier

The GitHub verifier checks organization membership and repository access via GitHub OAuth.

### How it works

1. User tries to join a policy-gated channel
2. Server redirects to GitHub OAuth
3. User authorizes, server checks org/repo membership
4. If valid, server issues a signed credential
5. Credential is stored and checked on future JOINs

### Configuration

Set the GitHub OAuth App client ID and secret in the server environment:

```
GITHUB_CLIENT_ID=Iv23li...
GITHUB_CLIENT_SECRET=...
```

### Supported checks

- `github:org:<name>` — Is the user a member of this GitHub org?
- `github:repo:<owner/repo>` — Is the user a collaborator on this repo?

## Built-in: Accept-Rules

The simplest verifier: user reads the channel rules and clicks "I accept."

- Credential type: `accept-rules`
- No external service needed
- Rules text set via `POLICY #channel SET RULES <markdown>`

## Verifier Architecture

```
User → JOIN #channel
  → Server checks policy
  → Missing credential? Redirect to verifier
  → Verifier checks claim (GitHub, etc.)
  → Issues signed credential (JWT-like, ed25519)
  → Credential stored in policy DB
  → User can now join
```

### Credential format

```json
{
  "iss": "did:web:irc.freeq.at:verify",
  "sub": "did:plc:user123",
  "type": "github:org:mycompany",
  "iat": 1709000000,
  "exp": 1709604800
}
```

Signed with the verifier's ed25519 key. Credentials have a TTL and are automatically revalidated.

## Building custom verifiers

A verifier is any service that:

1. Receives a verification request (user DID + credential type)
2. Checks the claim against an external source
3. Returns a signed credential or rejection

The server's verifier endpoint is at `/.well-known/did.json` for DID resolution. Custom verifiers can be added by implementing the credential issuance API.

## Planned verifiers

- **Bluesky follows** — Does a specific account follow this user?
- **Bluesky list member** — Is the user on a specific Bluesky list?
- **Domain handle** — Does the user's handle match a domain pattern?
- **Minimum followers** — Does the user have N+ followers?
