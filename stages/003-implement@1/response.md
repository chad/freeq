Here's a summary of what was done:

## Summary

**File added:** `freeq-app/src/irc/client-wiring.test.ts`

**22 new tests** across 7 `describe` blocks, all passing:

### Section A — Exported action functions (store side-effects)
| Test | What it pins |
|------|-------------|
| `sendMessage` × 4 | DM buffer created for nick targets; not created for `#channel`; works with null client |
| `sendReply` × 2 | Same buffer logic for reply targets |
| `joinChannel` × 2 | Channel added to store; set as active channel |
| `partChannel` × 2 | Channel removed from store; `saveJoinedChannels` writes to localStorage |

### Section B — wireEvents real dispatch chain (via `__wireEventsForTests`)
| Test | What it pins |
|------|-------------|
| `'message'` × 4 | DM auto-creates buffer; increments `mentionCount`; self-DMs don't increment; message added to buffer |
| `'error'` × 2 | Exact "same identity reconnected" string → `fullReset`; other errors do nothing |
| `'pinAdded'`/`'pinRemoved'` × 3 | Pin appended; pin removed; idempotent on duplicate add |
| `'motdStart'`/`'motd'` × 2 | Buffer reset + `motdDismissed` cleared; lines appended in order |
| `'historyBatch'` × 1 | Messages merged into channel |

**Convention followed:** matches `client-av.test.ts` exactly — `makeEventStub()` with `on`/`emit`, `vi.mock('@freeq/sdk')`, `vi.mock('../lib/notifications')`, same `globalThis` setup, `__setClientForTests` / `__wireEventsForTests` seams, no jsdom directive (runs in node environment like the passing `client-state.test.ts`).