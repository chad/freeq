/**
 * @freeq/bot-kit — on-disk persistence + announce-sequence orchestration for
 * freeq bots, on top of {@link https://www.npmjs.com/package/@freeq/sdk @freeq/sdk}.
 *
 * Usage:
 *   const bot = await FreeqBot.create({ name, ownerDid, nick, url });
 *   bot.on('message', (ch, msg) => bot.client.sendMessage(ch, `echo: ${msg.text}`));
 *   await bot.start();
 *
 *   process.once('SIGINT',  () => bot.stop('SIGINT').then(()  => process.exit(0)));
 *   process.once('SIGTERM', () => bot.stop('SIGTERM').then(() => process.exit(0)));
 */

export { FreeqBot } from "./bot.js";
export type {
  ActorClass,
  FreeqBotCreateOptions,
  FreeqBotStartOptions,
  FreeqBotStopOptions,
} from "./bot.js";

// AgentIdentity and DelegationCert are surfaced as `bot.identity` /
// `bot.delegation` properties, so consumers may need the types.
export type { AgentIdentity } from "./identity.js";
export type { DelegationCert } from "./delegation.js";
