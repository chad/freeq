#!/usr/bin/env node
/**
 * Streaming message demo — types out a message word-by-word using the
 * IRC edit-message hack. The same primitive LLM-powered bots use to
 * pipe Claude's streaming responses live into a channel.
 *
 * Wire pattern:
 *   1. Send a placeholder PRIVMSG with `+freeq.at/streaming=1` and capture
 *      the msgid via `sendAndAwaitEcho`.
 *   2. For each word, send a tagged PRIVMSG with `+draft/edit=<msgid>` and
 *      the streaming tag — server propagates the edit to all clients.
 *   3. Final edit drops the streaming tag, marking the message as settled.
 *
 * Run:
 *   npx tsx freeq-bot-kit-js/examples/streaming.ts \
 *     --owner did:plc:<your-did> --channel '#test' --message 'hello world'
 */

import { FreeqBot } from "../src/index.js";
import { parseArgs } from "node:util";
import { setTimeout as sleep } from "node:timers/promises";

const DEFAULT_MESSAGE =
  "Hello from the streaming message demo! This message is being typed out " +
  "word by word using the IRC edit-message hack. Each update edits the " +
  "same message in place. 🚀";

const { values } = parseArgs({
  options: {
    server: { type: "string", default: "wss://irc.freeq.at/irc" },
    channel: { type: "string", default: "#test" },
    nick: { type: "string", default: "stream-demo" },
    owner: { type: "string" },
    message: { type: "string", default: DEFAULT_MESSAGE },
    "word-delay-ms": { type: "string", default: "200" },
  },
  strict: true,
});

if (!values.owner) {
  console.error("Usage: streaming --owner did:plc:<your-did> [--channel #test] [--message '…']");
  process.exit(1);
}

const wordDelayMs = Number(values["word-delay-ms"]);

const bot = await FreeqBot.create({
  name: "stream-demo",
  ownerDid: values.owner,
  nick: values.nick!,
  url: values.server!,
  channels: [values.channel!],
});

await bot.start();
console.error(`[stream-demo] up as ${bot.client.nick}`);

// Let JOIN settle before editing.
await sleep(500);

// 1. Placeholder + capture msgid via echo.
const msgid = await bot.client.sendAndAwaitEcho(values.channel!, " ", {
  "+freeq.at/streaming": "1",
});
console.error(`[stream-demo] msgid=${msgid}`);

// 2. Word-by-word edits.
const words = values.message!.split(/\s+/).filter(Boolean);
let accumulated = "";
for (const [i, word] of words.entries()) {
  accumulated += (i > 0 ? " " : "") + word;
  bot.client.sendTagged(values.channel!, accumulated, {
    "+draft/edit": msgid,
    "+freeq.at/streaming": "1",
  });
  await sleep(wordDelayMs);
}

// 3. Final edit without the streaming flag — client renders as settled.
bot.client.sendEdit(values.channel!, msgid, accumulated);

await sleep(1000);
await bot.stop("stream demo complete");
process.exit(0);
