# Freeq Native Windows App — Detailed Technical Code Design

## 1) Scope and design intent

This document translates the Windows technical plan into an implementation-ready code design:

- concrete crate/project boundaries
- key structs/classes/interfaces
- FFI envelope formats and lifecycle rules
- reducer and state-store mechanics
- threading/concurrency model
- persistence schema
- UI ViewModel composition
- observability and failure handling paths

The design targets **WinUI 3 + Rust SDK** and reuses existing `freeq-sdk` command/event semantics.

---

## 2) Solution layout

```text
/workspace/freeq
  /freeq-windows-core        # New Rust crate (cdylib + internal modules)
  /freeq-windows-app         # New WinUI 3 solution (.sln)
  /docs/windows-native-app-code-design.md
```

### 2.1 Rust crate targets

`freeq-windows-core/Cargo.toml`:
- `crate-type = ["cdylib", "rlib"]`
- depends on `freeq-sdk`, `tokio`, `serde`, `serde_json`, `rusqlite`, `tracing`

### 2.2 C# solution projects

- `Freeq.Windows.App` (WinUI 3 front-end)
- `Freeq.Windows.Interop` (P/Invoke and marshaling)
- `Freeq.Windows.Domain` (DTOs + ViewModel contracts)

---

## 3) Rust core architecture (`freeq-windows-core`)

## 3.1 Module map

```text
src/
  lib.rs
  bridge/
    abi.rs               # extern "C" exports + handle map
    envelope.rs          # JSON event envelope + command payloads
    callback.rs          # callback registry and safe dispatch
  runtime/
    client_runtime.rs    # sdk connect/command/event tasks
    reconnect.rs         # reconnect policy state machine
  state/
    app_state.rs         # canonical in-memory state
    reducer.rs           # event -> mutation logic
    projections.rs       # snapshots/diffs for UI
  persistence/
    db.rs                # sqlite open/migrate/transactions
    message_store.rs     # message inserts/queries
    session_store.rs     # settings/session persistence
  services/
    typing.rs            # typing indicator timers/debouncing
    batching.rs          # event coalescing to UI frame window
  errors.rs
```

## 3.2 Root app container

```rust
pub struct AppCore {
    pub id: u64,
    pub runtime: tokio::runtime::Runtime,
    pub state: parking_lot::RwLock<AppState>,
    pub sdk_handle: parking_lot::Mutex<Option<freeq_sdk::client::ClientHandle>>,
    pub callback: CallbackSink,
    pub db: Db,
    pub reconnect: parking_lot::Mutex<ReconnectController>,
}
```

Responsibilities:
- own process-level resources per client instance
- coordinate SDK event ingestion and command dispatch
- persist/update state and emit UI deltas

## 3.3 Canonical state structures

```rust
pub struct AppState {
    pub connection: ConnectionState,
    pub identity: IdentityState,
    pub conversations: indexmap::IndexMap<String, ConversationState>,
    pub active_conversation: Option<String>,
    pub unread: std::collections::HashMap<String, u32>,
    pub read_markers: std::collections::HashMap<String, String>,
    pub motd: Vec<String>,
    pub ui_flags: UiFlags,
}

pub enum ConversationKind { Channel, Direct }

pub struct ConversationState {
    pub id: String,                     // #chan or nick
    pub kind: ConversationKind,
    pub title: String,
    pub topic: Option<String>,
    pub members: Vec<MemberState>,
    pub messages: std::collections::VecDeque<MessageState>,
    pub typing_users: std::collections::HashMap<String, i64>,
    pub last_activity_ms: i64,
}

pub struct MessageState {
    pub msgid: String,
    pub from: String,
    pub text: String,
    pub ts_ms: i64,
    pub reply_to: Option<String>,
    pub edited: bool,
    pub deleted: bool,
    pub reactions: std::collections::HashMap<String, std::collections::BTreeSet<String>>,
}
```

Rules:
- keep canonical state in Rust (single source of truth)
- UI consumes snapshots + incremental diffs
- message retention bounded per conversation (configurable cap)

## 3.4 Reducer design

`reduce(state: &mut AppState, event: DomainEvent) -> Vec<StateDiff>`

Reducer characteristics:
- deterministic, pure-ish mutation logic
- no blocking I/O inside reducer
- returns minimal typed diffs used by UI

`StateDiff` examples:
- `ConnectionChanged { old, new }`
- `ConversationUpserted { id }`
- `MessageInserted { conv_id, msgid, index }`
- `MessageUpdated { conv_id, msgid, fields }`
- `UnreadChanged { conv_id, unread }`

## 3.5 Domain event adapter

`freeq-sdk::event::Event` maps to internal `DomainEvent`:

```rust
pub enum DomainEvent {
    Connected,
    Registered { nick: String },
    Authenticated { did: String },
    Message(IncomingMessage),
    Tag(TagEvent),
    Joined { channel: String, nick: String },
    Parted { channel: String, nick: String },
    Names { channel: String, members: Vec<MemberState> },
    Topic { channel: String, topic: String },
    Mode { channel: String, mode: String, arg: Option<String> },
    Disconnected { reason: String },
}
```

Adapter responsibilities:
- normalize tags (`+draft/edit`, `+draft/delete`, `+react`, `+typing`)
- normalize DM target naming for self/peer direction
- stamp missing timestamps with monotonic fallback

---

## 4) FFI/API contract

## 4.1 Handle lifecycle

- `freeq_win_create_client(config_json)` allocates `AppCore`, returns handle (`u64`).
- Handle table is global `DashMap<u64, Arc<AppCore>>`.
- `freeq_win_destroy_client(handle)` unregisters callback, disconnects, drops runtime tasks.

## 4.2 Exported C ABI

```rust
#[no_mangle]
pub extern "C" fn freeq_win_create_client(config_json: *const c_char) -> u64;
#[no_mangle]
pub extern "C" fn freeq_win_destroy_client(handle: u64);
#[no_mangle]
pub extern "C" fn freeq_win_connect(handle: u64) -> i32;
#[no_mangle]
pub extern "C" fn freeq_win_disconnect(handle: u64) -> i32;
#[no_mangle]
pub extern "C" fn freeq_win_join(handle: u64, channel: *const c_char) -> i32;
#[no_mangle]
pub extern "C" fn freeq_win_send_message(handle: u64, target: *const c_char, text: *const c_char) -> i32;
#[no_mangle]
pub extern "C" fn freeq_win_send_raw(handle: u64, line: *const c_char) -> i32;
#[no_mangle]
pub extern "C" fn freeq_win_request_history(handle: u64, mode_json: *const c_char) -> i32;
#[no_mangle]
pub extern "C" fn freeq_win_get_snapshot_json(handle: u64) -> *mut c_char;
#[no_mangle]
pub extern "C" fn freeq_win_free_string(ptr: *mut c_char);
#[no_mangle]
pub extern "C" fn freeq_win_subscribe_events(handle: u64, cb: EventCallback, user_data: *mut c_void) -> i32;
```

Error codes (`i32`):
- `0` success
- `1` invalid handle
- `2` invalid argument
- `3` not connected
- `4` internal error

## 4.3 Event envelope

All callbacks send UTF-8 JSON:

```json
{
  "version": 1,
  "seq": 1024,
  "timestamp_ms": 1730000000000,
  "type": "state_diffs",
  "payload": {
    "diffs": [
      {"kind": "message_inserted", "conv_id": "#general", "msgid": "abc", "index": 442}
    ]
  }
}
```

Envelope types:
- `state_diffs`
- `error`
- `log` (debug builds only)
- `stats` (optional)

## 4.4 Callback contract

```c
typedef void (*EventCallback)(const char* json, void* user_data);
```

Constraints:
- callback may be invoked from non-UI thread
- payload memory owned by Rust for duration of call only
- callback must return quickly; UI side should enqueue and return

---

## 5) Concurrency and task model

## 5.1 Rust task topology

Per `AppCore` instance:
1. SDK connection task (`connect_with_stream` + event rx loop)
2. Command executor task (bounded mpsc from ABI calls)
3. Diff batching task (collect diffs for 16ms window)
4. Persistence task (async write queue)
5. Reconnect supervisor task

## 5.2 Queue sizing and backpressure

- command queue: 512
- raw event queue: 4096
- diff queue: 2048
- persistence queue: 2048

Policy:
- prefer coalescing over dropping for `typing`/`unread` churn
- allow drop of `log`/debug stats under pressure
- never drop connect/disconnect/auth/message events silently (emit pressure warning)

## 5.3 UI thread safety

- C# interop receives callback on worker thread
- pushes envelope onto `Channel<EventEnvelope>`
- `DispatcherQueue.TryEnqueue` applies diffs on main thread

---

## 6) Persistence design

## 6.1 SQLite schema (initial)

```sql
CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  topic TEXT,
  last_activity_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
  msgid TEXT PRIMARY KEY,
  conv_id TEXT NOT NULL,
  from_nick TEXT NOT NULL,
  text TEXT NOT NULL,
  ts_ms INTEGER NOT NULL,
  reply_to TEXT,
  edited INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_messages_conv_ts ON messages(conv_id, ts_ms);

CREATE TABLE IF NOT EXISTS reactions (
  msgid TEXT NOT NULL,
  emoji TEXT NOT NULL,
  nick TEXT NOT NULL,
  PRIMARY KEY (msgid, emoji, nick)
);

CREATE TABLE IF NOT EXISTS app_kv (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
```

`app_kv` keys:
- `server_addr`
- `nick`
- `auto_join_json`
- `read_markers_json`
- `theme`

## 6.2 Write model

- append new messages in batched transaction every 100ms or 100 events
- upsert conversation metadata on activity
- reaction/edit/delete mutate existing rows

## 6.3 Snapshot load model

Startup sequence:
1. load session config from `app_kv`
2. load recent conversations sorted by activity
3. load last N messages per conversation (config default 300)
4. build initial `AppState`

---

## 7) Security design

## 7.1 Secret boundaries

Rust core never persists broker token in plaintext DB.

Windows app stores secrets via platform API:
- Credential Locker (preferred) or DPAPI-protected secure file

Rust receives token only when needed:
- `freeq_win_set_web_token(handle, token)`

## 7.2 Logging hygiene

- redact tokens and DIDs where configured
- production log level defaults to `warn`
- debug logs opt-in in settings

---

## 8) WinUI 3 application code design

## 8.1 Composition root

`App.xaml.cs`:
- configure DI container
- initialize interop service
- open main `ShellWindow`
- register toast and deep-link handlers

## 8.2 ViewModel tree

```text
ShellViewModel
  ├─ ConnectionViewModel
  ├─ ConversationListViewModel
  ├─ ActiveConversationViewModel
  │    ├─ MessageListViewModel
  │    ├─ ComposerViewModel
  │    └─ MemberListViewModel
  └─ SettingsViewModel
```

Interfaces:

```csharp
public interface ICoreBridge {
    ulong CreateClient(ClientConfig config);
    void Connect(ulong handle);
    void Disconnect(ulong handle);
    void SendMessage(ulong handle, string target, string text);
    void Join(ulong handle, string channel);
    void SendRaw(ulong handle, string line);
    AppSnapshot GetSnapshot(ulong handle);
    IAsyncEnumerable<EventEnvelope> Subscribe(ulong handle, CancellationToken ct);
}
```

## 8.3 Diff applier

`StateDiffApplier` service:
- accepts envelope diffs
- updates in-memory observable stores
- minimizes `ObservableCollection` churn
- preserves scroll anchor for active timeline

## 8.4 Virtualized timeline

Implementation details:
- `ItemsRepeater` with `RecyclePool`
- message row template selector:
  - normal
  - compact same-sender continuation
  - system event
  - deleted placeholder
- incremental load trigger when near top => request `CHATHISTORY BEFORE`

## 8.5 Input and command routing

`ComposerViewModel` commands:
- `SendMessageCommand`
- `SendTypingCommand` (debounced)
- `EditMessageCommand`
- `DeleteMessageCommand`
- `ReactCommand`

Slash command parser in UI layer for local commands:
- `/join`, `/part`, `/nick`, `/topic`, `/raw` (debug-gated)

---

## 9) Error handling and reconnect flow

## 9.1 Error taxonomy

- `RecoverableNetwork`
- `AuthExpired`
- `AuthRejected`
- `ProtocolViolation`
- `Internal`

## 9.2 Reconnect state machine

```text
Disconnected
  -> Connecting
  -> Registered
  -> (network loss) BackoffWaiting
  -> Connecting
```

Backoff schedule:
- base 1s, multiplier 2, max 30s, jitter ±20%

Rules:
- clear backoff after stable connection window (>= 60s)
- on auth rejection, force token refresh before reconnect attempt

## 9.3 User-facing status

- inline status banner in shell
- transient toast for recoverable disconnect
- blocking auth dialog when credentials invalid

---

## 10) Telemetry and diagnostics

## 10.1 Core metrics

- `startup_ms`
- `connect_ms`
- `event_queue_depth`
- `diff_batch_size`
- `event_to_ui_ms`
- `sqlite_flush_ms`
- `dropped_events_total{type}`

## 10.2 Diagnostics channels

- rotating local log files
- optional JSON performance trace dump
- in-app diagnostics pane (dev mode)

---

## 11) Test design

## 11.1 Rust tests

- reducer property tests:
  - edits must target existing or become no-op append policy
  - delete sets tombstone state
- queue-pressure tests for batching/coalescing
- reconnect policy unit tests with deterministic clock
- DB migration tests from schema v1->vN

## 11.2 Interop tests (C#)

- P/Invoke smoke:
  - create/destroy handle
  - connect/disconnect cycle
  - callback subscription and envelope parse
- string memory tests for `get_snapshot_json/free_string`

## 11.3 UI tests

- conversation switch preserves draft text
- typing indicator appears/disappears within timeout
- history prepend keeps viewport stable
- theme switch applies without restart

---

## 12) Incremental implementation plan (code-first)

## Milestone A: Core bridge loop
- create crate + ABI exports
- connect + inbound message callback
- C# shell renders plain list of messages

## Milestone B: Canonical state + diffs
- reducer and `StateDiff` model
- snapshot load + incremental diff application
- unread counts and active conversation

## Milestone C: Persistence and reconnect
- sqlite writes/reads
- reconnect supervisor
- session restore on relaunch

## Milestone D: Rich IRC features
- reply/edit/delete/reaction
- member/topic/mode views
- typing indicators

## Milestone E: polish/perf hardening
- virtualization tuning
- queue telemetry + perf budgets
- accessibility + keyboard navigation pass

---

## 13) Open design decisions

1. **Interop payload format**
   - default JSON for flexibility
   - optional binary codec (MessagePack) only if profiling proves needed

2. **Single vs multi-window**
   - v1 single window for complexity control
   - design state store to support multi-window projection later

3. **Media pipeline timing**
   - defer uploads/previews until core text workflows are stable

4. **E2EE integration path**
   - keep adapter seam in `services/crypto_adapter.rs`
   - enable only with clear UX and key management policy

---

## 14) Definition of done (v1)

- Authenticated and guest connect flows stable
- Channel/DM chat with history, reactions, edit/delete, typing
- Session persistence + reconnect behavior validated
- 60fps scroll maintained on large history datasets
- Crash-free and memory budget targets met
- Installer (MSIX) and update channel working

