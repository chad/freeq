# Federation

freeq supports server-to-server (S2S) federation via iroh QUIC, allowing separate server instances to share channels, users, and state.

## How it works

Two freeq servers can peer with each other. When peered:

- Channels are shared across servers
- Messages from one server appear on the other
- User presence (joins, parts, quits) is synchronized
- Bans and modes are propagated

## Transport

Federation uses [iroh](https://iroh.computer/) QUIC for transport:

- **Encrypted** — All traffic is encrypted via QUIC TLS
- **NAT-traversing** — Works behind firewalls via hole-punching
- **Efficient** — Multiplexed streams over a single connection

## Configuration

```bash
freeq-server \
  --s2s-peer <iroh-endpoint-id> \
  --s2s-allowed-peers <comma-separated-ids>
```

## State synchronization

On peer connection, servers exchange a `SyncResponse` containing:

- Channel list with modes, topics, and members
- Ban lists
- Channel creation events

### CRDT convergence

Channel state uses operation-based CRDTs for eventual consistency:

- **Modes**: Additive merge (never weakens +n/+i/+t/+m)
- **Bans**: Additive merge (remote bans supplement local)
- **Topics**: Timestamp-based last-write-wins
- **Members**: Join/Part events applied in order

## Authorization

S2S operations are authorized:

- **Mode changes** — Verified against remote member op status
- **Kicks** — Verified that kicker has op privileges
- **Topic changes** — Verified in +t channels
- **Joins** — Checked against bans and invite-only
- **Rate limiting** — 100 events/sec per peer

## Limitations

- Invites are local-only (not yet synced)
- Channel key removal (`-k`) doesn't propagate via SyncResponse
- No mutual auth verification on outgoing connections yet

See [S2S Audit](/docs/s2s/) for a detailed protocol analysis.
