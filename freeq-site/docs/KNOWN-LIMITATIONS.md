# Known Limitations

## Authentication

- **DID method support**: Only `did:plc` and `did:web` are supported.
  Other DID methods (e.g. `did:key`, `did:ion`) are not implemented.
- **Key rotation**: If a user rotates their DID document keys, existing
  sessions are not invalidated. The server does not poll for key changes.
- **Handle verification**: The server resolves handles to DIDs at auth time
  but does not re-verify handles periodically. If a handle changes ownership,
  the server won't notice until the next authentication.

## IRC Protocol

- **No server operators (OPER)**: There is no concept of IRC operators
  (IRCops). Server administration is done via CLI flags and config.
- **No hostname cloaking**: All users appear as `nick!user@host` with a
  literal `host` placeholder. Real IP addresses are not exposed, but
  cloaking (vhosts) is not implemented.
- **No user limits (+l)**: Channel user limits are not implemented.
- **No secret/private channels (+s/+p)**: Channels always appear in LIST.
- **No WALLOPS, LINKS, STATS**: Server-to-server informational commands
  are not implemented.
- **USERHOST is simplified**: Returns `nick@host` with a generic hostname
  rather than the real connected host.

## S2S Federation

- **Ban propagation**: Bans set on one server are not automatically
  enforced on federated peers. Each server enforces its own ban list.
  The CRDT tracks bans, but enforcement is local only.
- **Invite-only bypass**: Remote JOINs from S2S peers do not check +i
  or bans. The remote server is trusted to enforce its own policies.
- **Founder race condition**: If two servers simultaneously create the
  same channel, both may assign different founders. The CRDT resolves
  this deterministically after sync (first-write-wins in causal order),
  but there is a brief inconsistency window.
- **No authorization-on-write for DID ops**: A rogue federated server
  could grant DID ops to arbitrary users. There is no cryptographic
  verification that the granting server had authority.
- **No S2S authentication**: Any iroh endpoint can connect as a peer
  unless `--s2s-allowed-peers` is configured. Open federation is the
  default.

## Persistence

- **No message retention by age**: Message pruning is count-based only
  (`--max-messages-per-channel`). There is no `--message-retention-days`.
- **No full-text search**: SQLite FTS5 is not wired up. Message search
  would require a separate index.
- **Single-server SQLite**: The database is a single SQLite file. There
  is no replication or multi-server persistence (state sync happens at
  the CRDT/S2S layer instead).

## E2EE

- **No forward secrecy**: Channel encryption keys are derived from a
  static passphrase. There is no ratcheting or key rotation.
- **Key distribution is manual**: Users must share the channel passphrase
  out-of-band. There is no key exchange protocol.
- **ENC2 group size**: DID-based group encryption requires all members'
  DIDs to derive the group key. Very large groups would have slow key
  derivation.

## Transports

- **WebSocket is uncompressed**: No per-message compression.
- **iroh relay dependency**: iroh uses relay servers for NAT traversal.
  If iroh's relay infrastructure is unavailable, direct connections may
  fail for users behind restrictive NATs.

## TUI Client

- **Not a full IRC client**: The TUI is a reference implementation for
  testing the SASL mechanism. It lacks many features expected of a
  production IRC client (DCC, scripts, multiple networks, etc.).
- **No mouse support**: Terminal mouse events are not handled.

## Plugin System

- **Compiled-in only**: Plugins must be compiled into the server binary.
  There is no dynamic loading (shared libraries). New plugins require
  a rebuild.
- **No async hooks**: Plugin hooks are synchronous. Long-running plugin
  logic should spawn tasks rather than blocking the hook.
- **Limited hook set**: Currently only `on_connect`, `on_auth`, `on_join`,
  `on_message`, and `on_nick_change` are available.
