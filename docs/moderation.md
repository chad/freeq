# Moderation

freeq provides IRC-standard moderation tools enhanced with cryptographic identity.

## Channel modes

| Mode | Meaning |
|---|---|
| `+o nick` | Operator — full channel control |
| `+h nick` | Half-op — can kick/ban, can't change modes |
| `+v nick` | Voice — can speak in moderated (+m) channels |
| `+b mask` | Ban — prevent user from joining |
| `+i` | Invite-only |
| `+m` | Moderated — only voiced/ops can speak |
| `+t` | Topic locked — only ops can change topic |
| `+n` | No external messages |
| `+k key` | Channel key (password) |

## DID-based moderation

Because users have cryptographic identities, moderation actions are more meaningful:

- **Bans by DID** — `MODE #chan +b did:plc:abc123` bans the identity, not just a nick
- **Persistent ops** — Op status is stored by DID, survives reconnects
- **Audit trail** — Who did what, with cryptographic attribution

## Operator commands

```
/op nick          — Give operator status
/deop nick        — Remove operator status
/voice nick       — Give voice
/kick nick reason — Kick from channel
/ban did:plc:...  — Ban by DID
/ban nick!*@*     — Ban by hostmask pattern
/unban mask       — Remove ban
/mode #chan +i     — Set invite-only
/invite nick      — Invite to +i channel
```

## Policy-based access

Instead of manual `/invite` and `/ban`, channels can use the [Policy Framework](/docs/policy-framework/) for automated, credential-based access control.

Example: Only GitHub org members can join `#dev`:
```
POLICY #dev SET REQUIRE github:org:mycompany
```

## Server operators

Server operators have global privileges:

```
OPER_DIDS=did:plc:abc123  # in server environment
```

Opers can:
- Operate in any channel
- Set global modes
- Access server administration

## Flood protection

Built-in per-user rate limiting:
- 5 messages per 2 seconds per session
- Line length limit: 8KB
- Nick validation: 1-64 chars, no control characters
- SASL: 3 failures → disconnect

## Best practices

1. Use DID bans over hostmask bans — they're identity-level
2. Set `+nt` on all channels (default on new channels)
3. Use policies for large communities — scales better than manual ops
4. DID ops bypass policy — ensure trusted founders are listed
