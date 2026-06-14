# Fix: idle web session silently degrades to guest

## Symptom

Leave the web app idle, come back, and the UI still looks "logged in" (verified
🔒 badge, real DID) — but you've actually become an anonymous guest. Messages
you send go out labeled `GuestNNNNN` while the badge still shows your identity.
(Reported 2026-06-14 with a screenshot of `Guest14953 🔒` posting "cool!".)

## Root cause

On an idle reconnect the SDK (`freeq-sdk-js`) re-mints the web session via a
broker `/session` refresh inside `onTransportStateChange('connected')`. Two
paths registered the connection as a **guest, client-side, without any server
904**:

1. **Safety-timer race** — the 8s registration safety timer raced the broker
   fetch's own 8s AbortController. If the timer won, it ran
   `this.sasl = null; sendRegistration()` → clean Guest 001.
2. **Broker-catch with no token** — broker refresh failed and there was no
   fallback token → same `this.sasl = null; sendRegistration()`.

Because neither path set `_saslFailed` or emitted `authenticated('')`:

- `_saslFailed` stayed false → outgoing PRIVMSGs were **not** blocked → guest
  messages leaked.
- The app store kept the **stale `authDid`** → the verified badge stayed lit
  next to the Guest nick, and `ReconnectBanner`'s `identityLost` guard
  (`connected && !authDid`) never fired.

The existing regression suite only covered the server-sent **904** path, not
these client-side guest fallbacks.

## Fix

### `freeq-sdk-js/src/client.ts`
- New `failReconnectAuth(reason)` mirrors the 904 teardown: drop creds, set
  `_saslFailed`, emit `authError` + `authenticated('')`, tear the socket down.
- Both silent-guest paths now route through it **when the user intended to be
  authenticated** (`sasl.did` set). Genuine guests (no DID) still register
  normally.
- `sendRegistration` no-ops if the transport was already torn down (guards a
  late broker resolution).
- Safety timer bumped 8s → 15s so the broker fetch's own 8s abort always wins
  the race and we get a clean attempt/failure instead of a guest slipping in.

### `freeq-app/src/irc/client.ts`
- `reconnect()` no longer replays the stale single-use web token for an
  authenticated, broker-backed session — it clears the in-memory token and
  `skipBrokerRefresh` so the SDK re-mints via `/session`. (Otherwise "Reconnect
  now" would 904 and bounce the user back to guest.)

## Tests
- `freeq-sdk-js/src/client.guest-fallback.test.ts`: +3 tests for the no-904
  broker-failure paths (no guest registration, authDid cleared, no PRIVMSG
  leak, genuine-guest unaffected).
- `freeq-app/src/irc/client-reconnect.test.ts`: new — reconnect forces broker
  refresh for authed sessions; guest reconnect unaffected.
- Full suites green: SDK 182, web app 697.
