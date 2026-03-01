# Freeq Native Windows App — Detailed Technical Plan

## 0) Executive summary

Build a **modern, stylish, high-performance native Windows desktop app** for Freeq by reusing the existing Rust client SDK (`freeq-sdk`) as the protocol/auth core, with a Windows-native UI shell and platform integrations.

**Recommended stack:**
- **Core engine:** Rust (`freeq-sdk`) + optional thin app-core crate (`freeq-windows-core`)
- **UI shell:** **WinUI 3 (Windows App SDK)** for native Fluent visuals and best Windows integration
- **Bridge:** C ABI / FFI boundary from Rust core to C#/WinRT host

This plan optimizes for:
1. Native look/feel (Fluent, acrylic/mica, system integrations)
2. Smooth performance on message-heavy channels
3. Reliable auth/session handling via AT Protocol web-token flow
4. Fast iteration with strict layering to preserve SDK reuse

---

## 1) Product goals, non-goals, and success metrics

## 1.1 Goals
- Native Windows desktop chat app with polished Fluent design.
- Full parity for core chat workflows:
  - connect/auth/guest
  - channel + DM messaging
  - history, reactions, edit/delete, typing
  - member/topic display
- Excellent runtime performance:
  - low-latency input/send
  - smooth scrolling with large histories
  - predictable memory behavior
- Stable reconnect behavior and robust session persistence.

## 1.2 Non-goals (v1)
- No Linux/macOS target from this codebase (Windows-first).
- No full plugin ecosystem in v1.
- No custom rendering engine; rely on native UI virtualization.

## 1.3 Success metrics
- Cold start to interactive UI: **< 1.5s** on mid-tier hardware.
- Time-to-first-message after connect: **< 400ms** average (excluding network).
- 60fps scrolling in 10k-message channels with virtualization enabled.
- Crash-free sessions: **> 99.5%**.

---

## 2) Architectural overview

## 2.1 High-level layers

1. **Protocol & transport layer (Rust SDK)**
   - Existing `freeq-sdk` handles connection, CAP/SASL, event stream, commands.

2. **Windows app core (new Rust crate: `freeq-windows-core`)**
   - Stateful reducer around SDK events
   - Domain stores for channels/DMs/unread/read pointers
   - Command API exposed over FFI
   - Background services (reconnect policy, cache writes, media pipeline)

3. **Interop layer**
   - C ABI exports from Rust (`cdylib`)
   - C# interop wrappers (`DllImport`, safe marshaling)
   - Event callback channel with batched payloads

4. **UI shell (WinUI 3)**
   - MVVM pattern (Views + ViewModels)
   - Virtualized message list
   - Fluent theming, animations, adaptive layout

5. **Platform services**
   - Windows Credential Locker / DPAPI for secrets
   - Toast notifications, jump list/deep links, tray integration

## 2.2 Why WinUI 3 for this app
- Best native Windows fidelity (Fluent controls/materials).
- Solid virtualization primitives for large message timelines.
- Strong integration with notifications, title bar, app lifecycle.

---

## 3) Repository and crate/project structure

## 3.1 New additions
- `freeq-windows-core/` (Rust)
- `freeq-windows-app/` (C# WinUI 3 solution)
- `docs/windows-native-app-technical-plan.md` (this plan)

## 3.2 `freeq-windows-core` modules
- `bridge/`
  - FFI-safe types, ABI exports, callback registry
- `state/`
  - app state model + reducers
- `services/`
  - reconnect, persistence, notifications adapter hooks
- `store/`
  - sqlite/json caches
- `commands/`
  - connect/send/join/part/history/reaction/edit/delete
- `metrics/`
  - perf counters + optional telemetry emitters

## 3.3 `freeq-windows-app` (WinUI) structure
- `App.xaml.cs` (startup/lifecycle)
- `Shell/` (navigation, title bar, command bar)
- `Features/`
  - `Auth/`
  - `Chats/`
  - `Settings/`
  - `Discover/` (optional phase 2)
- `Infrastructure/`
  - FFI wrappers, dispatcher bridge, DI config
- `Design/`
  - shared styles, theme resources, tokens

---

## 4) Data model and event flow

## 4.1 Core state entities
- `ConnectionState` (disconnected/connecting/connected/registered)
- `IdentityState` (nick, DID, auth mode)
- `ConversationState`
  - channel or DM metadata
  - message list model
  - members, topic, typing map
  - unread + last-read markers
- `SessionState`
  - server endpoint
  - auto-join list
  - persisted UI settings

## 4.2 Event pipeline
1. Rust SDK emits `Event` stream.
2. `freeq-windows-core` converts to internal domain events.
3. Reducer updates canonical state.
4. App-core emits **batched UI diffs** to WinUI bridge (avoid 1 event = 1 UI mutation).
5. ViewModels apply diffs on UI thread.

## 4.3 Outbound command pipeline
1. User action -> ViewModel command.
2. Command forwarded to app-core API.
3. app-core invokes SDK `ClientHandle`/raw command helpers.
4. Optimistic UI updates only where safe (input clear, pending send marker).

---

## 5) FFI and interop contract

## 5.1 ABI strategy
- Use stable C ABI exports from Rust.
- Pass JSON payloads for complex structures to reduce marshaling complexity.
- Keep primitive hot paths (connect/send/join) as direct typed calls.

## 5.2 Core exported API surface (v1)
- `freeq_win_create_client(config_json) -> handle`
- `freeq_win_connect(handle)`
- `freeq_win_disconnect(handle)`
- `freeq_win_send_message(handle, target, text)`
- `freeq_win_send_raw(handle, line)`
- `freeq_win_join(handle, channel)` / `part`
- `freeq_win_request_history(handle, target, mode_json)`
- `freeq_win_set_web_token(handle, token)`
- `freeq_win_subscribe_events(handle, callback, user_data)`
- `freeq_win_get_snapshot(handle) -> json`

## 5.3 Threading model
- Rust runtime owns network/event tasks.
- Callback invocations occur on background thread.
- C# bridge posts updates onto DispatcherQueue for UI-safe mutation.

## 5.4 Safety/reliability controls
- Versioned event envelope schema (`version`, `type`, `payload`).
- Backpressure: bounded event queue + coalescing.
- Strict ownership/lifetime rules for handles and callback teardown.

---

## 6) UI/UX plan (modern + stylish)

## 6.1 Design language
- Fluent 2 styling.
- Mica/Acrylic surfaces where appropriate.
- Dynamic theme sync (light/dark/system accent).
- Rounded cards, subtle depth, motion easing.

## 6.2 Primary shell
- Left rail: conversations + badges + quick filters.
- Main pane: message timeline + typing/footer composer.
- Optional right pane: members/topic/details.
- Adaptive collapse for smaller windows.

## 6.3 Message timeline
- Virtualized list (ItemsRepeater/ListView with recycling).
- Grouping by day and sender run.
- Inline chips for reactions, edited/deleted status.
- Reply context cards and jump-to-parent affordance.

## 6.4 Composer
- Markdown-lite formatting shortcuts.
- Enter to send / Shift+Enter newline.
- Upload/attach button placeholder for phase 2.
- Typing indicator debounce (3s guard).

## 6.5 Delight/polish
- Non-blocking subtle animations for navigation and state changes.
- Skeleton placeholders for history fetch.
- Toast banners for transient errors with action buttons.

---

## 7) Performance strategy

## 7.1 Rendering performance
- Virtualize conversation list and message list from day 1.
- Avoid full-collection resets; apply incremental diffs.
- Keep ViewModels immutable-ish for predictable updates.

## 7.2 Data performance
- Append-only message storage with capped in-memory windows.
- LRU for avatars/media thumbnails.
- Background persistence batching (e.g., flush every N events or T ms).

## 7.3 Network/runtime performance
- Reuse SDK async pipeline; avoid blocking calls on UI thread.
- Batch inbound events (e.g., 16ms frame-window) before UI dispatch.
- Reconnect backoff: 1,2,4,8,16,30s cap with jitter.

## 7.4 Instrumentation
- Track:
  - event-to-render latency
  - dropped/coalesced events
  - reconnect attempts
  - list virtualization metrics
- Debug perf overlay in dev builds.

---

## 8) Security and identity

## 8.1 Auth modes
- Primary: broker/web-token SASL flow (AT Protocol OAuth path).
- Secondary: guest mode.

## 8.2 Secret handling
- Store broker token / sensitive material via Windows secure storage (Credential Locker or DPAPI-protected blob).
- Never log raw tokens.
- Zeroize sensitive buffers when feasible in Rust.

## 8.3 Privacy defaults
- Minimize persisted plaintext in logs.
- User-controllable diagnostic logging toggle.

---

## 9) Persistence and offline behavior

## 9.1 Persisted artifacts
- Settings: theme, server, window/layout prefs, auto-join.
- Session: nick/handle and secure auth credential references.
- Cache: recent message windows, read pointers, avatar cache index.

## 9.2 Storage choice
- SQLite for message metadata/read pointers/unread counters.
- File cache directory for media thumbnails.

## 9.3 Offline strategy
- On startup with no network, load last cached conversations.
- Queue unsent drafts per conversation (optional v1.1).

---

## 10) Platform integrations (Windows-first)

- Windows toast notifications for mentions/DMs.
- Badge count on taskbar icon.
- Deep link handler for `freeq://` auth callbacks.
- System tray minimize-to-tray behavior (optional v1).
- Global keyboard shortcuts for quick switcher (Ctrl+K).

---

## 11) Delivery roadmap

## Phase 0 — Foundation (1–2 weeks)
- Create `freeq-windows-core` skeleton and FFI scaffolding.
- Build minimal WinUI shell + bridge health checks.
- Connect/disconnect happy path.

## Phase 1 — Core chat MVP (2–4 weeks)
- Auth (broker token + guest), channel list, message timeline, send message.
- Join/part, history latest/before, unread/read tracking.
- Basic settings and persistence.

## Phase 2 — Rich IRCv3 features (2–3 weeks)
- Typing, reactions, edit/delete, replies.
- Member list/topic/mode updates.
- Improved reconnect + failure UX.

## Phase 3 — Polish + performance (2 weeks)
- Virtualization tuning and batching improvements.
- UI polish, animations, accessibility pass.
- Crash/telemetry dashboards and release hardening.

## Phase 4 — Optional enhancements
- Media upload/preview pipeline.
- E2EE controls if server/client policy allows.
- Discover/community surfaces.

---

## 12) QA and testing strategy

## 12.1 Rust core tests
- Reducer unit tests for event->state transitions.
- Reconnect policy tests.
- Serialization/versioning tests for event envelopes.

## 12.2 Interop tests
- ABI smoke tests in CI (`create/connect/send/disconnect`).
- High-volume event flood test for callback stability.

## 12.3 UI tests
- Playwright/WinAppDriver-style smoke for:
  - login flow
  - open channel
  - send message
  - reconnect state transitions

## 12.4 Manual test matrix
- Windows 11 stable + Windows 10 compatibility where feasible.
- High DPI, multi-monitor, resize stress.
- Light/dark/accessibility contrast modes.

---

## 13) CI/CD and release plan

- Build pipeline:
  - Rust core checks (`fmt`, `clippy`, tests)
  - WinUI app build + signing verification
  - Integration smoke run
- Packaging:
  - MSIX for Store/sideload friendly distribution
  - Optional winget manifest updates
- Release channels:
  - Canary -> Beta -> Stable ring progression

---

## 14) Risks and mitigations

1. **FFI complexity / threading bugs**
   - Mitigation: strict event envelope, dedicated dispatcher bridge, heavy stress tests.

2. **UI perf regressions under heavy channels**
   - Mitigation: virtualization-first design, diff batching, perf budgets in CI.

3. **Auth flow edge cases on callback/deep linking**
   - Mitigation: explicit callback state machine + retry UX + diagnostic telemetry.

4. **SDK evolution drift vs app-core contract**
   - Mitigation: versioned adapter layer and compatibility tests pinned to SDK events.

---

## 15) Immediate next steps (implementation-ready)

1. Create `freeq-windows-core` crate with:
   - client bootstrap
   - event subscription callback
   - connect/disconnect/send/join primitives
2. Generate a WinUI 3 app shell with a basic chat window and status panel.
3. Implement first interop loop:
   - connect to server
   - render incoming messages in virtualized list
   - send message from composer
4. Add telemetry hooks for startup/connect/render timings.
5. Start Phase 1 feature branch with weekly milestone demos.

---

## 16) Notes on alignment with existing codebase

- This plan intentionally reuses the existing architecture already validated in:
  - `freeq-sdk` event-driven client model
  - `freeq-tui` state reducer/event loop shape
  - `freeq-ios` bridge + app-state pattern
- The Windows app should adopt the same **command handle + event stream + state reducer** pattern to maximize shared understanding and reduce protocol bugs.

