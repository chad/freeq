# Architecture Map: SDK + iOS + TUI

## 1) Shared SDK (`freeq-sdk`)

### Purpose
`freeq-sdk` is the protocol/client core used by multiple clients. It exposes IRC connectivity, AT Protocol auth flows, event streaming, and higher-level helpers (media tags, bot framework, E2EE primitives). The crate is explicitly structured as reusable modules under `src/lib.rs`. 

### Core layers

1. **Connection + transport (`client.rs`)**
   - `ConnectConfig` defines server/nick/TLS/web-token inputs.
   - Establishes **TCP** or **TLS** via `establish_connection`.
   - Optional **iroh QUIC** transport path behind feature flag (`iroh-transport`).
   - Main lifecycle: `connect` / `connect_with_stream` spawns async runtime task that:
     - negotiates `CAP LS`
     - does registration (`NICK`/`USER`)
     - performs SASL (web-token or challenge signer)
     - runs read/write loop
     - emits typed `Event` values.

2. **Application command surface (`ClientHandle`)**
   - Channel joins, messaging, raw IRC command passthrough.
   - IRCv3-tag helpers: replies, edit/delete, typing indicators, reactions, CHATHISTORY requests, pin/topic/mode helpers.

3. **Auth subsystem (`auth.rs`, `oauth.rs`, `pds.rs`)**
   - `ChallengeSigner` trait abstracts signing backend.
   - Implementations include key-based signer (`KeySigner`) and PDS/OAuth-backed signers.
   - OAuth module handles DPoP key/proof flow and session caching.
   - PDS module provides session creation/verification for app-password and related flows.

4. **Event contract (`event.rs`)**
   - SDK emits a single `Event` enum consumed by UIs/bots.
   - Includes connection/auth lifecycle, chat messages, tag messages, membership/mode/topic updates, batch/history events, raw lines, disconnect notices.

5. **Extra capability modules**
   - `bot.rs`: command router framework (permissions, rate limiting, helpers).
   - `media.rs`: rich media/message tag encoding helpers.
   - `ratchet.rs`, `x3dh.rs`, `e2ee.rs`, `e2ee_did.rs`: E2EE building blocks.
   - `p2p.rs`: optional p2p features.

### Dataflow (SDK)
1. Caller supplies `ConnectConfig` + optional signer.
2. SDK opens transport, negotiates capabilities/auth.
3. Caller sends outbound `Command`s through `ClientHandle`.
4. SDK parses inbound IRC lines into high-level `Event`s over channel receiver.

---

## 2) TUI Client (`freeq-tui`)

### High-level architecture

`freeq-tui` is a Rust terminal app that composes:

- **Core protocol/auth/media logic** from `freeq-sdk`
- **UI state machine** in `app.rs`
- **Rendering** in `ui.rs` (ratatui)
- **Input editing layer** in `editor.rs`
- **Config/session persistence** in `config.rs`

### Runtime flow

1. **Startup / identity resolution (`main.rs`)**
   - Parse CLI flags (server, nick, auth options, iroh, channels, vi mode).
   - Merge CLI + persisted config + last session.
   - Build signer via one of:
     - OAuth cached/interactive flow
     - app-password PDS session
     - crypto key auth
     - guest mode.

2. **Transport selection (`main.rs`)**
   - If `--iroh-addr` given, use iroh.
   - Else probe server capabilities for iroh and auto-upgrade when available.
   - Else use TCP/TLS connect path.

3. **Client bind (`main.rs`)**
   - Calls SDK `connect_with_stream` to get `(ClientHandle, EventReceiver)`.
   - Auto-joins saved/resolved channels.

4. **Event loop (`run_app`)**
   - Multiplexes terminal key events + SDK events + background task results.
   - Updates `App` state in memory (buffers, users, unread counts, transport status, reconnect info).
   - Renders each frame via `ui::draw`.

5. **Reconnect + persistence**
   - Tracks reconnect backoff state in `App`/`ReconnectInfo`.
   - Persists channel/session info on exit.

### TUI state model (`app.rs`)

- Central `App` struct holds:
  - buffer map (`status`, channels, DMs)
  - current editor state/history
  - connection + transport metadata
  - auth DID, media uploader, optional p2p handles
  - image cache / async background result channels
- Buffers are append-only deques with capped retention (`MAX_MESSAGES`).
- Batch history (CHATHISTORY BATCH) is staged and merged in-order.

### TUI boundaries

- **northbound**: user keyboard commands / slash commands / editor
- **southbound**: SDK `ClientHandle` commands
- **inbound domain events**: SDK `Event` stream
- **render boundary**: pure-ish draw pass from `App` state to terminal frame.

---

## 3) iOS Client (`freeq-ios`)

### High-level architecture

The iOS app is a SwiftUI shell around the Rust SDK through a UniFFI-generated bridge.

- **SwiftUI composition root**: `freeqApp.swift` + `ContentView.swift`
- **Global app store/state**: `Models/AppState.swift`
- **UI feature surfaces**: `Views/*`
- **Rust bridge artifact**: `FreeqSDK.xcframework` + generated Swift bindings (`Generated/freeq.swift`)
- **Rust FFI implementation**: `freeq-sdk-ffi`

### Rust↔Swift bridge design

1. `freeq-sdk-ffi` wraps SDK types and exposes:
   - `FreeqClient` class for connect/join/send/raw operations.
   - `EventHandler` callback trait and FFI-safe event/value structs.
   - Event conversion from SDK `Event` -> FFI `FreeqEvent`.
2. UniFFI generates Swift bindings.
3. iOS app links `FreeqSDK.xcframework` and calls these generated APIs.

### iOS runtime flow

1. **App launch (`freeqApp.swift`)**
   - Instantiates `AppState` + `NetworkMonitor` as environment objects.
   - Handles OAuth callback URL (`freeq://auth?...`) and stores broker/session tokens.

2. **Root routing (`ContentView.swift`)**
   - Chooses between connect screen, reconnecting screen, and chat tabs based on `connectionState` and saved-session state.

3. **Session + connection orchestration (`AppState`)**
   - Loads persistent values from `UserDefaults` + secrets from Keychain.
   - Maintains `FreeqClient` instance.
   - Supports broker-token session refresh (`/session`) to get fresh SASL web-token.
   - Connects/disconnects, joins/parts, sends IRC raw/tagged commands.
   - Tracks channels/DMs, unread counts, read markers, typing state, MOTD, reconnect backoff.

4. **Event ingestion (`SwiftEventHandler`)**
   - Implements FFI `EventHandler` and hops all events to main thread.
   - Reduces each `FreeqEvent` into `AppState` mutations (messages, names, topic, modes, reaction/delete/edit tags, disconnect handling).

### iOS state model

- `AppState` is effectively a **single store** (`ObservableObject`) for connection + chat state.
- `ChannelState` objects carry per-channel messages/members/topic/typing info.
- UI components subscribe to `@Published` properties and render declaratively.

---

## 4) Cross-project relationship map

- `freeq-sdk` is the protocol/runtime core.
- `freeq-tui` links it directly (native Rust consumer).
- `freeq-ios` consumes it indirectly through `freeq-sdk-ffi` + UniFFI-generated Swift layer.

### Shared event-driven pattern

Both clients follow the same model:

1. Connect and obtain command handle + event stream.
2. Send outbound commands through the handle.
3. Reduce inbound events into client-local state.
4. Render state (terminal widgets or SwiftUI views).

### Key architectural difference

- **TUI**: in-process Rust all the way down.
- **iOS**: SwiftUI state/store on top, Rust core over FFI boundary.

This means iOS has extra bridge concerns (thread hops, FFI-safe type mapping, artifact generation), while TUI can use SDK types directly.

