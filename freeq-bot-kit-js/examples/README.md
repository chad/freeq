# bot-kit examples

Runnable examples of `@freeq/bot-kit`. None require any external service beyond a freeq server and your AT Protocol DID.

| Example | What it shows |
|---|---|
| [`echo-bot.ts`](echo-bot.ts) | The canonical "is bot-kit working?" smoke test. Echoes messages, replies `pong` to `!ping`. Minimal — `FreeqBot.create` + `bot.start` + manual SIGINT. |
| [`daemon.ts`](daemon.ts) | The same echo bot wrapped in `createDaemonCLI`. Ships `launch [--detach] / stop / status / doctor / tail` out of the box. The shape long-running bot daemons should adopt. |
| [`gated-bot.ts`](gated-bot.ts) | The full pattern: owner-only gate with allowlist (`createDidMap`), per-sender refusal cooldown + cycle detection (`createTurnGate`), `@`-mention parsing in non-bot channels (`bot.checkMention`), live DID resolution from incoming messages (`bot.resolveSenderDid`), all under `createDaemonCLI`. The four message-handling primitives composed in one realistic daemon. |
| [`streaming.ts`](streaming.ts) | Types out a message word-by-word using the edit-message hack. Same primitive LLM-powered bots use to pipe streaming responses. |
| [`url-fetch-worker.ts`](url-fetch-worker.ts) | The canonical agent pattern on freeq — listens for `task_request` coordination events, claims them, fetches the URL, reports results via `task_complete`. |
| [`fire-task.ts`](fire-task.ts) | Helper for testing `url-fetch-worker`. Fires a single `task_request` and exits. |

## Running

From the repo root:

```bash
# Build the package once (the examples import from src/ via `tsx`,
# but @freeq/sdk's compiled output is needed):
npm --prefix freeq-sdk-js run build

# Run any example with tsx:
npx tsx freeq-bot-kit-js/examples/echo-bot.ts --owner did:plc:<your-did> --channel '#test'
```

All examples take an `--owner did:plc:…` flag. You can find your DID at <https://bsky.app/profile/your.handle> or by calling `fetchProfile` from `@freeq/sdk`.

Each example creates its own did:key under `~/.freeq/bots/<example-name>/` on first run and reuses it on subsequent runs. Delete that directory to start fresh.

## Demoing the worker

**Terminal 1** — run the worker (it'll sit in the channel listening):

```bash
npm run example:url-worker -- \
  --owner did:plc:<your-did> --channel '#tasks'
```

**Terminal 2** — fire a single task at it:

```bash
npm run example:fire-task -- \
  --owner did:plc:<your-did> --channel '#tasks' \
  --url 'https://httpbin.org/delay/3'
```

What you should see (watch the channel in freeq-app or whatever client):

1. `tasker` joins and posts a `task_request` (rendered as `📋 New task: …`).
2. `url-fetch-worker` immediately posts a `task_claim` (`🙋 Claiming task: …`).
3. Worker transitions to `state=executing`. `WHOIS url-fetch-worker` during this window shows the live state.
4. After ~3 seconds (`httpbin.org/delay/3`), worker posts `task_complete` (`🎉 Task complete: 200 OK — <size>B in <ms>ms`) and transitions back to `state=idle`.
5. Tasker quits; worker keeps running until you Ctrl-C it.

`fire-task.ts` takes `--url <any-url>` so you can vary the work. Try `https://example.com` (instant) or `https://httpbin.org/delay/8` (longer) to see different `executing` durations.
