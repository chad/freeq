#!/usr/bin/env node
/**
 * Echo bot — the canonical "is bot-kit working?" example.
 *
 * Connects, joins a channel, echoes any non-self message back, and
 * responds to `!ping` with `pong`.
 *
 * Run from the repo root after `npm run build` in this package:
 *   npx tsx freeq-bot-kit-js/examples/echo-bot.ts \
 *     --owner did:plc:<your-did> --channel '#test'
 */

import { FreeqBot } from "../src/index.js";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  options: {
    server: { type: "string", default: "wss://irc.freeq.at/irc" },
    channel: { type: "string", default: "#test" },
    nick: { type: "string", default: "echo-bot" },
    owner: { type: "string" },
  },
  strict: true,
});

if (!values.owner) {
  console.error("Usage: echo-bot --owner did:plc:<your-did> [--channel #test] [--nick echo-bot] [--server wss://…/irc]");
  process.exit(1);
}

const bot = await FreeqBot.create({
  name: "echo-bot",
  ownerDid: values.owner,
  nick: values.nick!,
  url: values.server!,
  channels: [values.channel!],
});

bot.on("message", (channel, msg) => {
  if (msg.isSelf) return;
  if (msg.text === "!ping") {
    bot.client.sendMessage(channel, "pong");
    return;
  }
  bot.client.sendMessage(channel, `echo: ${msg.text}`);
});

await bot.start();
console.error(`[echo-bot] up as ${bot.client.nick} (${bot.identity.did})`);

process.once("SIGINT",  () => bot.stop("SIGINT").then(()  => process.exit(0)));
process.once("SIGTERM", () => bot.stop("SIGTERM").then(() => process.exit(0)));
