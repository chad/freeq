# Federation

freeq servers can federate with each other over encrypted QUIC connections using [iroh](https://iroh.computer). Channels, messages, topics, modes, and membership state converge automatically across servers using CRDTs ([Automerge](https://automerge.org)).

## How it works

Each freeq server gets a cryptographic identity from iroh — an Ed25519 keypair that uniquely identifies it on the network. Servers connect peer-to-peer over QUIC with automatic NAT traversal.

When servers are linked:

1. **Initial sync** — Each server sends its full channel state (members, modes, topics) as a CRDT document.
2. **Real-time relay** — Messages, joins, parts, kicks, modes, and topics are relayed as they happen.
3. **Conflict resolution** — CRDTs ensure eventual consistency. Concurrent mode changes merge towards the more restrictive state.
4. **Policy sync** — Channel policies federate automatically. Policy changes broadcast to all peers.

## Setting up federation

### Start two servers

```bash
# Server A
./target/release/freeq-server \
  --iroh \
  --listen-addr 0.0.0.0:6667 \
  --server-name server-a.example.com

# Note the iroh endpoint ID from the log:
# iroh endpoint: <32-byte-hex-id>
```

```bash
# Server B
./target/release/freeq-server \
  --iroh \
  --listen-addr 0.0.0.0:6668 \
  --server-name server-b.example.com \
  --s2s-peers <server-a-endpoint-id>
```

Both servers will sync state and relay messages bidirectionally.

## What federates

| Feature | Federated? | Notes |
|---------|-----------|-------|
| Channel messages | ✅ | Full relay with msgid |
| Private messages | ✅ | Relay to all peers (recipient check at destination) |
| JOIN/PART | ✅ | Remote members shown in NAMES/WHOIS |
| KICK | ✅ | First-class S2S message |
| MODE changes | ✅ | Including +o, +v, +k, +l, +i, +b |
| TOPIC | ✅ | Respects +t on both sides |
| INVITE | ✅ | Relayed to correct peer |
| Channel policies | ✅ | `PolicySync` message on SET/CLEAR |
| User identity (DID) | ✅ | DID propagated with CRDT state |
| CRDT document | ✅ | Full Automerge sync on connect |

## CRDT convergence

Channel state is maintained in Automerge documents. Each server independently tracks:

- Member list
- Channel modes
- Topic
- Ban list

When servers sync, Automerge merges the documents deterministically. Conflicts are resolved by:

- **Members**: Union of both servers' member lists
- **Modes**: More restrictive wins (if local members exist)
- **Topic**: Last-writer-wins by timestamp
- **Bans**: Union

## Remote members

Remote members (users connected to a peer server) are tracked as a display cache. They appear in NAMES and WHOIS responses but are not a routing gate — messages to unknown users are always relayed to all peers.

When a peer disconnects, all remote members from that peer are cleaned up immediately via a `PeerDisconnected` event.

## Testing federation locally

```bash
./scripts/test-local-e2e.sh
```

This starts two linked servers on different ports, runs the full acceptance test suite, and tears everything down.
