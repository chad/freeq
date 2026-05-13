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

## Daemon CLI scaffold

For long-running bot daemons, `createDaemonCLI` wires the universal commands (`launch`, `stop`, `status`, `doctor`, `tail`) over a [Commander](https://www.npmjs.com/package/commander) program. The bot supplies a `runDaemon` callback; bot-kit handles pid files, `--detach` forking, signal wiring, and the built-in doctor checks (identity, delegation, server actor record).

```ts
import { createDaemonCLI } from '@freeq/bot-kit';

const cli = createDaemonCLI({
  name: 'mybot',
  paths: {
    dir:        '~/.mybot/',
    daemonPid:  '~/.mybot/daemon.pid',
    daemonLog:  '~/.mybot/daemon.log',
    agentKey:   '~/.mybot/agent.key',
    delegation: '~/.mybot/delegation.json',
  },
  // Pre-launch hook (prompts, config persistence). Runs in BOTH the
  // foreground and the detached child — must be idempotent.
  preflight: async (parsed) => {
    const owner = await loadOrPromptOwner();
    return { ownerDid: owner.did, nick: parsed.nick ?? 'mybot' };
  },
  // The daemon entry point. Only runs in the daemon process.
  runDaemon: async (opts) => {
    const bot = await FreeqBot.create({ ...opts, url: 'wss://irc.freeq.at/irc' });
    bot.on('message', (ch, msg) => bot.client.sendMessage(ch, `echo: ${msg.text}`));
    await bot.start();
    return { stop: (reason) => bot.stop(reason) };
  },
  // Extra `launch` flags. Caller reads via parsed.<flag> inside preflight.
  launchOptions: [
    { flags: '--nick <nick>', description: 'Override the bot nick' },
  ],
  // Server actor URL — enables provenance check in `status` + `doctor`.
  actorStatusUrl: (did) => `https://irc.freeq.at/api/v1/actors/${encodeURIComponent(did)}`,
  // Optional bot-specific doctor checks, appended after built-ins.
  doctorChecks: [
    { name: 'claude binary', run: async () => {
        try { execSync('which claude', { stdio: 'pipe' }); return { ok: true }; }
        catch { return { ok: false, reason: 'claude not on PATH' }; }
    }},
  ],
});

// Add custom subcommands on top.
cli.command('grant <did> <action>').description('Grant access').action(/* ... */);

await cli.parseAsync(process.argv);
```

**Built-in `doctor` checks:** identity file (32-byte ed25519 seed → did:key), delegation cert (parses + `bot_did === agent.did`), server actor record (if `actorStatusUrl` provided, queries `online` + `provenance.verified`). Each `doctorChecks` entry runs after, in registration order, with `{ ok: true, detail? } | { ok: 'warn', reason } | { ok: false, reason }`. Doctor exits 1 if any check fails (warnings don't fail).

**Two-callback launch model:** `preflight` runs in foreground (prompts ok) and re-runs idempotently in the detached child after fork. `runDaemon` only runs in the daemon process and receives `preflight`'s return value. Signal handlers (SIGINT/SIGTERM) are wired by the scaffold; the returned handle's `stop(reason)` is invoked on shutdown.

## What this package does NOT do

Deliberately out of scope:

- **Owner config / handle resolution.** Caller provides `ownerDid`. Bot-kit doesn't prompt or persist owner.json.
- **Signal handlers (when not using `createDaemonCLI`).** `process.on('SIGINT', ...)` is process-global; if you're using `FreeqBot` directly without the CLI scaffold, the application owns it. The README's quick example shows the snippet.
- **Reconnect logic.** Already in the SDK transport (`@freeq/sdk`'s `Transport` does exponential-backoff auto-reconnect and re-emits `'ready'` on resume). Bot-kit's announce loop is already bound to every `'ready'` so reconnects re-announce automatically.
- **DM dispatch / capability gating / ACLs.** Application logic. Use `bot.on('message', ...)` and the SDK directly.
- **did:key rotation.** Bot-specific (e.g. freeqcc also wipes per-DID claude sessions). Write a `rotate-key` subcommand on the returned `Command`.
- **Manifest building.** Bot-kit takes a pre-built TOML string. Compose your manifest however you like.

## Status

v0.1 — extracted from `freeqcc` and `freeq-swarm`. Both projects will migrate onto this package.
