# Channel Policy System

freeq channels can have **access policies** that control who can join and what roles they receive. Policies are credential-based: users prove something about their identity (e.g., GitHub membership, Bluesky follows) to gain access.

## Quick Start

### 1. Set Channel Rules

As a channel operator (`+o`), set the rules users must accept:

```
/msg ChanServ POLICY #mychannel SET Be respectful. Follow our Code of Conduct.
```

Or use the **Channel Settings** panel in the web UI (click the gear icon).

This creates a basic "accept rules" policy. Users see the rules and must accept to join.

### 2. Add Credential Verifiers (Optional)

Require users to prove something beyond accepting rules:

```
# Require GitHub repo access
/msg ChanServ POLICY #mychannel REQUIRE github_repo issuer=did:web:irc.freeq.at:verify url=/verify/github/start?repo=owner/repo label=GitHub_Repo

# Require GitHub org membership
/msg ChanServ POLICY #mychannel REQUIRE github_membership issuer=did:web:irc.freeq.at:verify url=/verify/github/start?org=myorg label=GitHub_Org

# Require Bluesky follow
/msg ChanServ POLICY #mychannel REQUIRE bluesky_follower issuer=did:web:irc.freeq.at:verify url=/verify/bluesky/start?target=handle.bsky.social label=Bluesky_Follow
```

### 3. Configure Role Escalation (Optional)

Auto-grant channel modes based on credentials:

```
# GitHub repo contributors get op (+o)
/msg ChanServ POLICY #mychannel SET-ROLE op {"type":"PRESENT","credential_type":"github_repo","issuer":"did:web:irc.freeq.at:verify"}

# Moderators get halfop (+h)
/msg ChanServ POLICY #mychannel SET-ROLE moderator {"type":"PRESENT","credential_type":"channel_moderator","issuer":"did:web:irc.freeq.at:verify"}
```

### 4. Remove Policy

```
/msg ChanServ POLICY #mychannel CLEAR
```

## How It Works

1. User tries to join a channel with a policy
2. Server checks if the user has valid credentials
3. If not, the web/iOS client shows a gate modal with:
   - The channel rules
   - Links to verify credentials (GitHub OAuth, etc.)
4. User completes verification → receives a signed credential
5. User can now join the channel
6. If role rules are configured, the user is auto-granted modes (op/voice/etc.)

## Credential Types

| Type | Description | Verifier |
|------|-------------|----------|
| `github_repo` | Push access to a GitHub repository | GitHub OAuth |
| `github_membership` | Member of a GitHub organization | GitHub OAuth |
| `bluesky_follower` | Follows a specific Bluesky account | AT Protocol |
| `channel_moderator` | Appointed by channel ops | Manual |

## Web UI

The easiest way to manage policies is through the **Channel Settings** panel:

1. Click the ⚙️ gear icon in the channel header
2. Go to the **Rules** tab to set/update channel rules
3. Go to the **Verifiers** tab to add credential requirements
4. Go to the **Roles** tab to configure auto-granted modes

The UI provides templates for common setups and shows the current policy in human-readable form.

## Architecture

- Policies are stored in a separate SQLite database (`irc-policy.db`)
- Credentials are signed by the server's verifier DID (`did:web:irc.freeq.at:verify`)
- Credentials are reusable within their TTL (default 5 minutes)
- DID operators and channel founders always bypass policy checks
- Policies are versioned — updating rules increments the version

## API

```
GET  /api/v1/policy/{channel}    — Fetch current policy
POST /api/v1/policy/{channel}/accept — Accept channel rules (returns credential)
```

## Notes

- Only channel operators (`+o`) can set/modify policies
- DID-authenticated users (`did:plc:...`) who are channel founders or DID-ops bypass all policy checks
- Guest users can satisfy `ACCEPT` requirements but not credential-based ones (they need a DID)
- Policies compose: `REQUIRE` adds to existing requirements with AND logic
