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

## Identity & provenance

A message's sender identity — the IRCv3 `account` tag, i.e. the sender's DID —
is carried across S2S, so a federated user renders with their real handle and
avatar on the receiving server, not just a bare nick. The DID is always stamped
by the origin server from the sender's authenticated session; clients never set
it.

Federated identity is **peer-vouched, not locally verified.** The receiving
server did not authenticate the remote sender — it relays the origin's claim on
the same peer trust it already extends to the message body. To keep this honest,
federated messages also carry `+freeq.at/origin=<server>` naming the origin, so
clients can distinguish:

- **Locally verified** (no `+freeq.at/origin`): this server authenticated the
  sender via SASL — render as verified.
- **Peer-vouched** (`+freeq.at/origin` present): relayed from that server —
  render as "via {origin}", not as locally verified.

The signature (`+freeq.at/sig`) is **not** verifiable across servers today (its
canonical inputs aren't all reconstructable downstream), so clients must not
show a "cryptographically verified" affordance on federated messages.
End-to-end-verifiable federated identity is future work.

`account` and `+freeq.at/origin` are persisted, so provenance survives history
replay (CHATHISTORY), not only live delivery.

## Security

- **Allowlist**: Use `--s2s-allowed-peers` to restrict federation to trusted peers only.
  Open federation (default) is suitable for development but not production.
- **Rate limiting**: 100 events/sec per peer, excess dropped with warning.
- **Authorization**: All mode/kick/topic/join operations verified server-side.

See [Security Hardening Guide](/docs/security/) for full details.

## Limitations

- Invites are local-only (not yet synced)
- Channel key removal (`-k`) doesn't propagate via SyncResponse
- `--s2s-allowed-peers` only enforces incoming; outgoing relies on `--s2s-peers` consistency

See [S2S Audit](/docs/s2s/) for a detailed protocol analysis.
