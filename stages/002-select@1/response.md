I have everything I need. Here is my analysis and recommendation:

---

## Chosen File: `freeq-sdk/src/client.rs` (Rust, `freeq-sdk` crate)

### Why it's high-risk and undertested

The hotspot analysis scores `freeq-sdk/src/client.rs` at **gamma 14** — third highest overall, behind only the massive `s2s_acceptance.rs` test file and `server.rs`. CLAUDE.md explicitly flags `sdk/client.rs` as **"ZERO unit tests on connection state machine"** (as of the note's writing).

The file is 3,712 lines and implements the entire IRC connection state machine: capability negotiation, SASL authentication, multiline batch assembly, inbound event dispatch (KICK, INVITE, AWAY, TOPIC, QUIT, MODE, NICK, WHOIS numerics, server NOTICE, PRIVMSG routing), command queuing, keepalive/ping, and reconnect logic.

### Current test coverage gaps

The existing `#[cfg(test)]` block at the bottom of `client.rs` covers:
- multiline batch assembly (8 tests)
- `ConnectConfig` validation (4 tests)
- PING → PONG, 001 welcome, nick-in-use retries, SASL 904, raw injection stripping, signing presence, JOIN event, PART event, EOF disconnect (9 tests in `run_irc_tests`)

**Completely untested inbound dispatch paths** (none of the 17 current `run_irc_tests` touch these):
1. **KICK** — `run_irc` parses KICK and emits `Event::Kicked`; zero tests
2. **INVITE** — parses INVITE and emits `Event::Invited`; zero tests
3. **AWAY** — parses AWAY and emits `Event::AwayChanged`; zero tests
4. **TOPIC / 332** — parses live TOPIC changes and RPL_TOPIC (332) and emits `Event::TopicChanged`; zero tests
5. **QUIT** — parses QUIT and emits `Event::UserQuit`; zero tests
6. **NICK** — parses NICK change and emits `Event::NickChanged`; zero tests

### Specific behaviors I intend to pin

1. **`KICK` → `Event::Kicked`**: Server sends `:kicker!u@h KICK #chan victim :Too noisy\r\n`; verify `Event::Kicked { channel, nick, by, reason }` is emitted with correct fields.
2. **`INVITE` → `Event::Invited`**: Server sends `:sender!u@h INVITE target #secret\r\n`; verify `Event::Invited { channel, by }`.
3. **`AWAY` → `Event::AwayChanged` (set and unset)**: Server sends `:nick!u@h AWAY :On lunch\r\n` (away_msg = `Some`) and `:nick!u@h AWAY\r\n` (no params → `None`); both paths tested.
4. **`TOPIC` → `Event::TopicChanged` with set_by**: Server sends `:alice!u@h TOPIC #chan :New topic\r\n`; verify channel, topic, and `set_by = Some("alice")`.
5. **`332` RPL_TOPIC → `Event::TopicChanged` with no set_by**: Server sends `:server 332 me #chan :The topic\r\n`; verify `set_by = None`.
6. **`QUIT` → `Event::UserQuit`**: Server sends `:leaver!u@h QUIT :Goodbye\r\n`; verify `nick` and `reason`.

### Test convention to follow

All new tests live in the existing `#[cfg(test)] mod run_irc_tests` block inside `freeq-sdk/src/client.rs` (lines ~3240–3712). Each test uses the `start_run_irc(nick)` helper already present there, which:
- Creates a `tokio::io::duplex` pair
- Spawns `run_irc` on the client side
- Returns the server-side half (for writing IRC lines) + event receiver + command sender

New tests follow the exact same pattern: write an IRC line to `server`, then `tokio::time::timeout`-wait for the expected event in `events.recv()`.