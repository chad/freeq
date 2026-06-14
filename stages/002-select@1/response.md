I now have a comprehensive picture. Here is my analysis and recommendation:

---

## Chosen File: `freeq-app/src/irc/client.ts`

**Language/crate:** TypeScript, `freeq-app` (Vite + Vitest)

---

### Why it's high-risk and undertested

CLAUDE.md explicitly flags it: **`irc/client.ts` (gamma 133) — UNDERTESTED — needs dedicated unit tests**.

The current hotspot run confirms it at gamma 5 (3rd after server-side files), making it the highest-risk file within the CI-covered TypeScript surface. The gap between what the file does and what tests cover is large:

- The file exposes ~30 exported functions (`sendMessage`, `sendReply`, `sendEdit`, `sendDelete`, `joinChannel`, `partChannel`, `reconnect`, `setSaslCredentials`, `pinMessage`, `unpinMessage`, etc.) that contain **store-mutation side effects beyond the SDK call** — e.g. `sendMessage` creates a DM buffer, `joinChannel` calls both `addChannel` and `setActiveChannel`, `partChannel` calls `saveJoinedChannels`.
- The `wireEvents` function (~230 lines, ~25 event handlers) routes every inbound SDK event to the Zustand store. The `__wireEventsForTests` escape hatch exists precisely to enable testing this, but most handlers remain uncovered.
- Existing tests (`client.test.ts` = parser tests, `client-state.test.ts` = store simulations that manually replicate what the handlers do, `client-reconnect.test.ts` = 96 lines on broker token), **none of them exercise `wireEvents` via `__wireEventsForTests`**, meaning the real dispatch chain is untested.
- `client-av.test.ts` uses `__wireEventsForTests` but only for the three `avSession*` event handlers (lines 377–460).

---

### 3–6 specific behaviors to pin

1. **`sendMessage` to a DM target creates the buffer** — when `target` is a nick (not `#channel`), the store should have a channel entry for that nick after the call, even if the SDK client is null. Currently there's no test that exercises this exported function at all.

2. **`sendReply` to a DM target also creates the buffer** — same logic branch exists in `sendReply`, equally untested.

3. **`wireEvents: 'message'` handler for a DM creates the buffer and increments mentions** — when a PRIVMSG arrives in a non-channel buffer that doesn't yet exist in the store, `wireEvents` must auto-create it via `addChannel`. The handler also calls `incrementMentions` for DMs. This path is *simulated* in `client-state.test.ts` but never run through the real handler.

4. **`wireEvents: 'error'` with "same identity reconnected" triggers `fullReset`** — this specific error string branches to `fullReset()` (which wipes all state) vs. doing nothing for other errors. One wrong `includes()` string breaks the entire re-auth recovery flow. Not tested anywhere.

5. **`wireEvents: 'pinAdded'` / `'pinRemoved'`** — these call `addPin`/`removePin` on the store. No pin-related events are tested in the `wireEvents` layer (the store methods themselves are tested separately).

6. **`joinChannel` sets the channel as active; `partChannel` persists to localStorage** — `joinChannel` calls `setActiveChannel` in addition to `addChannel`; `partChannel` calls `saveJoinedChannels` which writes to `localStorage`. Neither is covered.

---

### Test convention to follow

The existing sibling file **`freeq-app/src/irc/client-av.test.ts`** sets the pattern for `wireEvents` integration tests: it stubs a `FreeqClient`-shaped EventEmitter with `vi.fn()` methods, calls `__wireEventsForTests(stub)`, then emits events on the stub and asserts store state. The `__setClientForTests` export in `client.ts` enables injecting a stub client for the exported-function tests. Both patterns are established and the new tests should follow them exactly, importing from `vitest` and using the same `localStorage` mock setup seen in `client-state.test.ts`.