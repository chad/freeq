#!/usr/bin/env node
/**
 * Helper: fire a single task_request at a channel, then exit.
 *
 * Used to test url-fetch-worker. Run url-fetch-worker in one terminal,
 * then run this in another:
 *
 *   npm run example:fire-task -- --owner did:plc:<your-did> \
 *     --channel '#tasks' --url 'https://httpbin.org/delay/3'
 *
 * Worker should claim the task, transition to executing, fetch, then
 * emit task_complete and transition back to idle.
 */

import { FreeqBot } from "../src/index.js";
import { parseArgs } from "node:util";
import { setTimeout as sleep } from "node:timers/promises";

const { values } = parseArgs({
  options: {
    server: { type: "string", default: "wss://irc.freeq.at/irc" },
    channel: { type: "string", default: "#tasks" },
    nick: { type: "string", default: "tasker" },
    owner: { type: "string" },
    capability: { type: "string", default: "url_fetch" },
    url: { type: "string", default: "https://httpbin.org/delay/3" },
  },
  strict: true,
});

if (!values.owner) {
  console.error("Usage: fire-task --owner did:plc:<your-did> [--channel #tasks] [--url <url>]");
  process.exit(1);
}

const bot = await FreeqBot.create({
  name: "tasker",
  ownerDid: values.owner,
  nick: values.nick!,
  url: values.server!,
  channels: [values.channel!],
});

await bot.start();
console.error(`[tasker] up as ${bot.client.nick}`);

const taskId = bot.client.emitEvent(values.channel!, "task_request", {
  capability: values.capability!,
  url: values.url!,
});
console.error(`[tasker] fired task_request id=${taskId} for ${values.url}`);

// Give the wire a moment so the task_request and our QUIT don't collide.
await sleep(500);
await bot.stop("done");
process.exit(0);
