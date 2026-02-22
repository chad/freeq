# REST API Reference

freeq exposes a read-only REST API on the web address (`--web-addr`). All responses are JSON.

## General

### Health check

```
GET /api/v1/health
```

```json
{ "status": "ok", "server_name": "irc.freeq.at", "users": 42, "channels": 7 }
```

## Channels

### List channels

```
GET /api/v1/channels
```

```json
[
  { "name": "#general", "topic": "Welcome to freeq", "members": 15 },
  { "name": "#dev", "topic": "Development discussion", "members": 8 }
]
```

### Channel history

```
GET /api/v1/channels/{name}/history?limit=50&before=<msgid>
```

```json
{
  "messages": [
    {
      "msgid": "01HXY...",
      "from": "chad",
      "text": "Hello world",
      "timestamp": "2026-02-18T08:00:00Z",
      "tags": {}
    }
  ]
}
```

### Channel topic

```
GET /api/v1/channels/{name}/topic
```

```json
{ "topic": "Welcome to freeq", "setter": "chad", "set_at": "2026-02-18T08:00:00Z" }
```

## Users

### User info

```
GET /api/v1/users/{nick}
```

```json
{
  "nick": "chad",
  "did": "did:plc:abc123",
  "channels": ["#general", "#dev"],
  "away": null
}
```

### User WHOIS

```
GET /api/v1/users/{nick}/whois
```

```json
{
  "nick": "chad",
  "user": "chad",
  "host": "at/did:plc:abc123",
  "realname": "Chad Fowler",
  "did": "did:plc:abc123",
  "channels": ["@#general", "#dev"]
}
```

## Media

### Upload (authenticated)

```
POST /api/v1/upload
Content-Type: multipart/form-data

file: <binary>
```

Requires an active AT Protocol session (web-token authenticated). Returns:

```json
{ "url": "https://cdn.bsky.app/img/feed_thumbnail/..." }
```

## Policy API

### Get channel policy

```
GET /api/v1/policy/{channel}
```

```json
{
  "policy": {
    "channel_id": "#project",
    "version": 1,
    "requirements": { "type": "ACCEPT", "hash": "abc..." },
    "role_requirements": {},
    "credential_endpoints": {}
  },
  "authority_set": null
}
```

### Policy version history

```
GET /api/v1/policy/{channel}/history
```

### Submit join evidence

```
POST /api/v1/policy/{channel}/join
Content-Type: application/json

{
  "did": "did:plc:abc123",
  "accepted_hashes": ["abc..."],
  "credentials": [],
  "proofs": []
}
```

### Personalized requirements check

```
POST /api/v1/policy/{channel}/check
Content-Type: application/json

{ "did": "did:plc:abc123" }
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
      "action": {
        "action_type": "accept_rules",
        "label": "Accept Rules",
        "accept_hash": "abc..."
      }
    }
  ]
}
```

### Check membership

```
GET /api/v1/policy/{channel}/membership/{did}
```

### Transparency log

```
GET /api/v1/policy/{channel}/transparency
```

### Present external credential

```
POST /api/v1/credentials/present
Content-Type: application/json

{
  "credential": {
    "type": "FreeqCredential/v1",
    "issuer": "did:web:verify.example.com",
    "subject": "did:plc:abc123",
    "credential_type": "github_membership",
    "claims": { "org": "myorg" },
    "issued_at": "2026-02-18T00:00:00Z",
    "expires_at": "2026-03-18T00:00:00Z",
    "signature": "base64url..."
  }
}
```

The server resolves the issuer DID, extracts the Ed25519 public key, and verifies the signature.

### List credentials for DID

```
GET /api/v1/credentials/{did}
```

## WebSocket

### IRC over WebSocket

```
ws://host:port/irc
wss://host:port/irc
```

Standard IRC protocol over WebSocket. Each WebSocket text frame is one IRC message (with or without `\r\n`).

## OAuth

### Start OAuth login

```
GET /auth/login?handle=user.bsky.social
```

Redirects to the user's AT Protocol authorization server.

### OAuth callback

```
GET /auth/callback?code=...&state=...&iss=...
```

Completes the OAuth flow, generates a one-time web-token, and returns an HTML page that posts the token to the opener window.

### Client metadata

```
GET /client-metadata.json
```

OAuth client metadata per RFC 7591 / AT Protocol spec.
