Now I have a comprehensive picture. Let me assess both candidates side by side:

**`freeq-sdk/src/client.rs`** (3712 lines, 62 public functions, 28 tests):
- CLAUDE.md flags it explicitly: "sdk/client.rs (gamma 104) — ZERO unit tests on connection state machine"
- The existing 28 tests cover: multiline assembly (14 tests), basic connection state machine behaviors (14 tests: PING/PONG, 001, nick-in-use, 904, raw, privmsg signing, JOIN, PART, EOF)
- **Zero tests** for: NICK change events, QUIT events, KICK events, AWAY events, TOPIC events, INVITE events, MODE events, NAMES/353, TAGMSG dispatch, server NOTICE parsing, `legacy +freeq.at/multiline \n` normalization, `ConnectConfig::validate()` invalid char cases (nick with `#`, `!`, `@`, etc.)

**`freeq-app/src/components/MessageList.tsx`** (1327 lines):
- Has 391 lines of tests already — reasonably covered.

The clear winner is `freeq-sdk/src/client.rs`.

---

## My Choice: `freeq-sdk/src/client.rs`

**Language/crate:** Rust, `freeq-sdk`

### Why it's high-risk and undertested

CLAUDE.md explicitly flags it: **"sdk/client.rs (gamma 104) — ZERO unit tests on connection state machine."** At 3,712 lines with 62 public functions (117 `fn` definitions total), only 28 `#[tokio::test]` cases exist — and the majority of those (14) test `multiline` batch assembly, a self-contained sub-feature. The core IRC event dispatch loop (`run_irc`) handles over a dozen server-sent message types; of those, only PING, 001 RPL_WELCOME, 433 nick-in-use, 904 SASL failure, JOIN, and PART have any test coverage. Every other incoming event type is completely unexercised by automated tests.

### 3–6 specific behaviors I'll pin with tests

1. **`NICK` → `Event::NickChanged`** — Server sends `:alice!u@h NICK :alice2`. The client must emit `Event::NickChanged { old_nick: "alice", new_nick: "alice2" }`. Critically, the nick is parsed from the prefix (before `!`), not from params.

2. **`QUIT` → `Event::UserQuit`** — Server sends `:bob!u@h QUIT :Gone` and the client must emit `Event::UserQuit { nick: "bob", reason: "Gone" }`. Edge case: quit with empty/missing reason must not panic.

3. **`KICK` → `Event::Kicked`** — Server sends `:op!u@h KICK #room victim :spam`. Client must emit `Event::Kicked { channel: "#room", nick: "victim", by: "op", reason: "spam" }`.

4. **`AWAY` → `Event::AwayChanged`** — Two sub-cases: (a) `:user!u@h AWAY :be right back` → `away_msg: Some("be right back")`, and (b) `:user!u@h AWAY` (no param) → `away_msg: None` (user returning).

5. **`TOPIC` → `Event::TopicChanged`** — `:mod!u@h TOPIC #room :New topic` must emit `Event::TopicChanged { channel: "#room", topic: "New topic", set_by: Some("mod") }`.

6. **`ConnectConfig::validate()` invalid nick characters** — The validator rejects nicks containing `#`, `!`, `@`, `*`, `?`, `,`, space, or control chars. There are no tests for these individual rejection paths despite the function having 8 distinct invalid-char checks.

### Existing test convention to follow

The sibling test module `mod irc_loop_tests` at the bottom of `freeq-sdk/src/client.rs` (line 3247) provides the exact pattern: use `start_run_irc()` to spin up `run_irc` over a `tokio::io::duplex`, write raw IRC lines to the "server side", then `tokio::time::timeout`-wait for specific `Event` variants on the event channel. All new tests will follow this same in-process, no-network, no-server pattern.