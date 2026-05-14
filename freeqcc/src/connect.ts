// SDK wiring: delegate to @freeq/bot-kit's FreeqBot, which owns the
// did:key SASL → ready → PROVENANCE → AGENT REGISTER → PRESENCE →
// HEARTBEAT lifecycle and re-runs the announce on every reconnect.
//
// Adapter shape: returns the same `Connected` interface daemon.ts has
// always consumed ({client, agentDid, nick, stop}).
import {
  FreeqBot,
  type AgentIdentity,
  type DelegationCert,
  type FreeqClient,
  type MentionResult,
  type ResolveOpts,
} from "@freeq/bot-kit";
import { homedir } from "node:os";

export interface ConnectOptions {
  identity: AgentIdentity;
  /** Owner DID — passed through to bot-kit; only used if delegation.json
   *  is missing on disk and needs to be minted. */
  ownerDid: string;
  delegation: DelegationCert;
  nick: string;
  /** Defaults to wss://irc.freeq.at/irc */
  serverUrl?: string;
  /** Heartbeat interval in ms; default 30_000. */
  heartbeatMs?: number;
}

export interface Connected {
  client: FreeqClient;
  /** The DID we authenticated as. */
  agentDid: string;
  /** The nick the server registered us with. */
  nick: string;
  /** Stops the heartbeat and sends QUIT. */
  stop(reason?: string): Promise<void>;
  /** Resolve a PRIVMSG sender's DID via the bot's resolver (account-tag
   *  → cache → WHOIS-with-timeout). Returns null if unresolvable. */
  resolveSenderDid(
    msg: { from: string; tags?: Record<string, string> },
    opts?: ResolveOpts,
  ): Promise<string | null>;
  /** Classify a channel message as addressed-to-us with per-channel
   *  cooldown. Returns ignore / cooldown / respond. */
  checkMention(channel: string, text: string): MentionResult;
}

const DEFAULT_URL = "wss://irc.freeq.at/irc";

export async function connect(opts: ConnectOptions): Promise<Connected> {
  // Match freeqcc's historical on-disk layout (~/.freeqcc/agent.key,
  // ~/.freeqcc/delegation.json) by passing name=".freeqcc" + root=homedir.
  // bot-kit's stateDir then equals freeqcc's paths.dir, and the existing
  // seed + cert are reused without migration.
  const bot = await FreeqBot.create({
    name: ".freeqcc",
    root: homedir(),
    ownerDid: opts.ownerDid,
    nick: opts.nick,
    url: opts.serverUrl ?? DEFAULT_URL,
    heartbeatMs: opts.heartbeatMs,
  });

  // Sanity-check that bot-kit reused the same identity freeqcc resolved.
  if (bot.identity.did !== opts.identity.did) {
    throw new Error(
      `bot-kit identity (${bot.identity.did}) does not match ` +
        `freeqcc-loaded identity (${opts.identity.did}). State drift; investigate ~/.freeqcc/agent.key.`,
    );
  }

  await bot.start();

  // Reclaim-rename: freeq's ghost-session reclaim adopts the previous nick
  // for multi-device continuity (registration.rs "Adopt the ghost's nick").
  // If the user changed config and asked for a different nick this run,
  // force the rename. Runs once after initial ready, and again on each
  // subsequent ready (reconnects).
  const reclaimRename = (): void => {
    if (bot.client.nick && bot.client.nick !== opts.nick) {
      bot.client.raw(`NICK ${opts.nick}`);
    }
  };
  reclaimRename();
  bot.on("ready", reclaimRename);

  return {
    client: bot.client,
    agentDid: bot.identity.did,
    nick: bot.client.nick || opts.nick,
    stop: (reason?: string) => bot.stop(reason ?? "freeqcc stop"),
    resolveSenderDid: (msg, resolveOpts) => bot.resolveSenderDid(msg, resolveOpts),
    checkMention: (channel, text) => bot.checkMention(channel, text),
  };
}
