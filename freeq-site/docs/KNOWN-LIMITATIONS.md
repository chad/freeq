# Known Limitations

## Authentication

- **DID method support**: Only `did:plc` and `did:web` are supported.
  Other DID methods (e.g. `did:key`, `did:ion`) are not implemented.
- **Key rotation**: If a user rotates their DID document keys, existing
  sessions are not invalidated. The server does not poll for key changes.
- **Handle verification**: The server resolves handles to DIDs at auth time
  but does not re-verify handles periodically. If a handle changes ownership,
  the server won't notice until the next authentication.
- **DPoP nonce rotation**: PDS nonce rotation during SASL is handled via
  automatic retry (server signals nonce, client retries). If the PDS rotates
  nonces again during the retry, authentication will fail (requires reconnect).

## IRC Protocol

- **No user limits (+l)**: Channel user limits are not implemented.
- **No secret/private channels (+s/+p)**: Channels always appear in LIST.
- **No WALLOPS, LINKS, STATS**: Server-to-server informational commands
  are not implemented.
- **USERHOST is simplified**: Returns `nick@host` with a cloaked hostname
  rather than the real connected host.
- **No services integration (NickServ/ChanServ)**: Identity is DID-based,
  not services-based.

## S2S Federation

- **Invite sync**: Invites are local-only. A user invited on server A can
  only join on server A. Relay of invite tokens to peers is not implemented.
- **Channel key removal**: `-k` cannot propagate via SyncResponse (additive
  only). Needs a protocol change or CRDT-backed key state.
- **Outgoing peer enforcement**: `--s2s-allowed-peers` only checks incoming
  connections. Outgoing connections go to whatever `--s2s-peers` specifies.
  Ensure both flags are consistent for mutual authorization.
- **Founder race condition**: If two servers simultaneously create the
  same channel, both may assign different founders. The CRDT resolves
  this deterministically after sync (first-write-wins), but there is a
  brief inconsistency window.
- **Topic merge strategy**: SyncResponse ignores remote topic if local is set,
  but CRDT reconciliation uses last-write-wins. The two merge strategies can
  cause flapping in edge cases.

## Persistence

- **No message retention by age**: Message pruning is count-based only
  (`--max-messages-per-channel`). There is no `--message-retention-days`.
- **No full-text search**: SQLite FTS5 is not wired up. Message search
  would require a separate index.
- **Single-server SQLite**: The database is a single SQLite file. There
  is no replication or multi-server persistence (state sync happens at
  the CRDT/S2S layer instead).

## E2EE

- **No forward secrecy for channels**: Channel encryption keys are derived
  from a static passphrase. There is no ratcheting or key rotation.
  (DMs use X3DH + Double Ratchet and do have forward secrecy.)
- **Key distribution is manual**: Users must share the channel passphrase
  out-of-band. There is no key exchange protocol for channels.
- **ENC2 group size**: DID-based group encryption requires all members'
  DIDs to derive the group key. Very large groups would have slow key
  derivation.

## Transports

- **WebSocket is uncompressed**: No per-message compression.
- **iroh relay dependency**: iroh uses relay servers for NAT traversal.
  If iroh's relay infrastructure is unavailable, direct connections may
  fail for users behind restrictive NATs.

## Web Client

- **No offline mode**: Requires active WebSocket connection. No service
  worker message caching.
- **No push notifications**: Desktop notifications only work while the
  tab is open.

## TUI Client

- **No auto-reconnection**: If the connection drops, you must restart.
- **Not a full IRC client**: The TUI is a reference implementation. It
  lacks DCC, scripts, multiple networks, etc.
- **No mouse support**: Terminal mouse events are not handled.

## Plugin System

- **Compiled-in only**: Plugins must be compiled into the server binary.
  There is no dynamic loading. New plugins require a rebuild.
- **No async hooks**: Plugin hooks are synchronous. Long-running plugin
  logic should spawn tasks rather than blocking the hook.
- **Limited hook set**: Currently only `on_connect`, `on_auth`, `on_join`,
  `on_message`, and `on_nick_change` are available.

## Resolved (no longer limitations)

The following were previously listed as limitations and have been fixed:

- ~~No server operators (OPER)~~ → OPER command + `--oper-dids` auto-oper
- ~~No hostname cloaking~~ → `freeq/plc/xxxxxxxx` for DID users, `freeq/guest` for guests
- ~~No S2S ban enforcement~~ → Bans sync via S2S, enforced on join (nick + DID)
- ~~No S2S authorization~~ → Mode/kick/topic/join all verified server-side
- ~~No S2S rate limiting~~ → 100 events/sec per peer
- ~~No CHATHISTORY~~ → IRCv3 CHATHISTORY with batch support
- ~~No account-notify~~ → IRCv3 account-notify + extended-join
- ~~Open federation by default~~ → `--s2s-allowed-peers` for allowlist mode
- ~~Per-IP connection limits~~ → 20 connections/IP on TCP + WebSocket
