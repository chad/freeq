# @freeq/bot-kit

High-level wrapper over [`@freeq/sdk`](../freeq-sdk-js/) for building freeq bots. Owns the boilerplate every bot needs:

- did:key identity persistence (32-byte ed25519 seed at `~/.freeq/bots/<name>/agent.key`, mode 0600)
- `FreeqBotDelegation/v1` cert minting and storage (`~/.freeq/bots/<name>/delegation.json`)
- `FreeqClient` construction with crypto SASL wired from the bot's did:key
- the announce sequence — PROVENANCE → AGENT REGISTER → (optional) AGENT MANIFEST → PRESENCE → HEARTBEAT — re-fired on every reconnect
- state-aware heartbeats: `bot.setState(...)` is the single source of truth for what your bot is doing
- hardened startup: rejects on SASL auth failure, pre-ready disconnect, or timeout

## Install

```bash
npm install @freeq/bot-kit @freeq/sdk
```

## Runnable examples

Four illustrative bots live under [`examples/`](examples/):

- [`echo-bot.ts`](examples/echo-bot.ts) — canonical smoke test; echoes messages, replies `pong` to `!ping`
- [`streaming.ts`](examples/streaming.ts) — types out a message word-by-word using the edit-message hack
- [`url-fetch-worker.ts`](examples/url-fetch-worker.ts) — the canonical agent pattern: claims `task_request` coordination events, fetches the URL, reports via `task_complete`, transitions state along the way
- [`fire-task.ts`](examples/fire-task.ts) — helper that fires a single `task_request` and exits; pairs with `url-fetch-worker` for end-to-end testing

Run any of them with `npm run example:<name> -- --owner did:plc:<your-did> --channel '#test'`. See [`examples/README.md`](examples/README.md) for the full worker walk-through.

## Quick example

```ts
import { FreeqBot } from '@freeq/bot-kit';

const bot = await FreeqBot.create({
  name: 'echo-bot',                    // → ~/.freeq/bots/echo-bot/
  ownerDid: 'did:plc:abc123',          // your AT Protocol DID
  nick: 'echo-bot',
  url: 'wss://irc.freeq.at/irc',
  channels: ['#bots'],                 // auto-join on connect
});

bot.on('message', (channel, msg) => {
  if (msg.isSelf) return;
  if (msg.text === '!ping') {
    bot.client.sendMessage(channel, 'pong');
  }
});

await bot.start();
console.log(`[${bot.identity.did}] up as ${bot.client.nick}`);

// Wire your own signal handlers — bot-kit doesn't install any.
process.once('SIGINT',  () => bot.stop('SIGINT').then(()  => process.exit(0)));
process.once('SIGTERM', () => bot.stop('SIGTERM').then(() => process.exit(0)));
```

That's it. On first run you'll get a fresh did:key under `~/.freeq/bots/echo-bot/`. Subsequent runs reuse the same identity.

## API

### `FreeqBot.create(options)`

Async factory. Loads/creates identity + delegation cert from disk, constructs a `FreeqClient` with crypto SASL, and returns a ready-to-`start()` bot.

```ts
await FreeqBot.create({
  // Required
  name: string,              // scopes state under ~/.freeq/bots/<name>/
  ownerDid: string,          // 'did:plc:…' — caller resolves Bluesky handles themselves
  nick: string,              // IRC nickname
  url: string,               // WebSocket URL, e.g. 'wss://irc.freeq.at/irc'

  // Optional
  root?: string,             // override parent dir (default: ~/.freeq/bots)
  actorClass?: 'agent' | 'external_agent' | 'human',  // default 'agent'
  initialState?: string,     // default 'active'; carried by heartbeats until setState
  initialStatus?: string,    // optional initial PRESENCE status string
  manifest?: string,         // TOML manifest. If set, announce includes AGENT MANIFEST.
  channels?: string[],       // auto-join on connect (forwarded to SDK)
  heartbeatMs?: number,      // default 30_000
  heartbeatTtlS?: number,    // default 60
  serverOrigin?: string,     // REST API origin (default: derived from url)
  onNickCollision?: 'refuse' | 'auto-suffix' | 'random-suffix',  // default 'refuse'
});
```

Caller resolves the `ownerDid`. If you have a Bluesky handle, use `fetchProfile` from `@freeq/sdk` (or any other resolver) before calling `FreeqBot.create`.

### `bot.start({ timeoutMs? })`

Connects, awaits `'ready'`, runs the announce sequence, starts the heartbeat loop. Resolves once announced.

Rejects with a typed error on:
- SASL auth failure → `SASL auth failed: …`
- Pre-ready disconnect → `disconnected before ready`
- Timeout (default 30s) → `timeout waiting for ready (Nms)`

### `bot.stop(reasonOrOptions?)`

Graceful shutdown. Stops the heartbeat loop, sends `PRESENCE :state=offline` and `QUIT :<reason>`, waits for the WebSocket send buffer to drain (default 2000ms), then disconnects. Idempotent.

```ts
await bot.stop();                                  // default reason
await bot.stop('SIGINT');                          // string shorthand
await bot.stop({ reason: 'redeploy', drainMs: 500 });
```

**Server-side ghost period (~30s).** The freeq server applies a 30-second grace window to DID-authenticated sessions on disconnect — channel membership is held briefly so a quick reconnect doesn't churn JOIN/QUIT messages. To other clients in the channel this looks like the bot lingering after shutdown. It's intentional. If you watch via the freeq-app web client, the bot will visibly disappear ~30s after `bot.stop()` resolves. Heartbeat-TTL-only cleanup (when the QUIT never reaches the server) extends this to ~60-90s.

### `bot.setState(state, status?, task?)`

Updates the bot's current state. Sends an immediate `PRESENCE` update; subsequent heartbeats carry the new state.

```ts
bot.setState('executing', 'reviewing PR #42');
// ... do work ...
bot.setState('idle');
```

Valid states: `online`, `idle`, `active`, `executing`, `waiting_for_input`, `blocked_on_permission`, `blocked_on_budget`, `degraded`, `paused`, `sandboxed`, `revoked`, `offline`.

Read current state via `bot.state` (the last value set).

### Events

`bot.on(event, handler)`, `bot.off(...)`, `bot.once(...)` are typed delegations to `bot.client.on/off/once`. Subscribe to any event from `FreeqEvents`:

```ts
bot.on('message',       (channel, msg) => { /* ... */ });
bot.on('memberJoined',  (channel, member) => { /* ... */ });
bot.on('governance',    (signal) => { /* react to pause/resume/revoke */ });
bot.on('coordinationEvent', (event) => { /* react to task_request etc */ });
```

### Properties

```ts
bot.client      // FreeqClient — escape hatch for anything not on the wrapper
bot.identity    // { did, didKey, isFresh } — your did:key identity
bot.delegation  // FreeqBotDelegation/v1 cert
bot.stateDir    // absolute path: ~/.freeq/bots/<name>/
bot.state       // current PRESENCE state (string)
```

`bot.client` is fully available for anything the wrapper doesn't surface: `bot.client.sendMessage(...)`, `bot.client.requestWhois(...)`, `bot.client.spawnAgent(...)`, etc. See the [SDK reference](../freeq-site/docs/typescript-sdk.md) for the full surface.

## What this package does NOT do

Deliberately out of scope:

- **Owner config / handle resolution.** Caller provides `ownerDid`. Bot-kit doesn't prompt or persist owner.json.
- **Signal handlers.** `process.on('SIGINT', ...)` is process-global; the application owns it. Bot-kit shows the snippet (above) but doesn't install handlers itself.
- **Reconnect logic.** Already in the SDK transport (`@freeq/sdk`'s `Transport` does exponential-backoff auto-reconnect and re-emits `'ready'` on resume). Bot-kit's announce loop is already bound to every `'ready'` so reconnects re-announce automatically.
- **DM dispatch / capability gating / ACLs.** Application logic. Use `bot.on('message', ...)` and the SDK directly.
- **Manifest building.** Bot-kit takes a pre-built TOML string. Compose your manifest however you like.

## Status

v0.1 — extracted from `freeqcc` and `freeq-swarm`. Both projects will migrate onto this package.
