// Long-lived freeqcc daemon: load identity + owner + delegation,
// connect to freeq, listen for DMs, dispatch owner DMs to claude.
//
// Phase 5 wires connect + announce + heartbeat. The DM gate + claude
// dispatch (phase 6) hangs off the returned client.
import { loadOrCreateIdentity } from "./identity.js";
import { loadOrPromptOwner } from "./owner.js";
import { loadOrMintDelegation } from "./delegation.js";
import { connect, type Connected } from "./connect.js";
import { evaluate, loadGateState, saveGateState, type GateState } from "./gate.js";
import { dispatchToClaudeStreaming } from "./dispatch.js";
import { logRefused } from "./audit.js";
import { loadAllowlist, type AllowlistEntry } from "./allowlist.js";
import { paths, ensureDir } from "./paths.js";
import { writeFile } from "node:fs/promises";

interface DispatchTelemetry {
  dispatchCount: number;
  totalCostUsd: number;
  lastDispatchCostUsd: number;
  lastDispatchAt: string;
}

async function persistDispatchTelemetry(t: DispatchTelemetry): Promise<void> {
  try {
    await ensureDir();
    await writeFile(
      paths.dir + "/telemetry.json",
      JSON.stringify(t, null, 2) + "\n",
      { mode: 0o600 },
    );
  } catch {
    // best-effort
  }
}

export interface DaemonOptions {
  /** IRC nick. If omitted, derives from owner handle: `<owner>-agent` (truncated). */
  nick?: string;
  serverUrl?: string;
}

/** Default nick: `<owner-handle>-agent`, truncated to fit IRC nick limits. */
function deriveDefaultNick(handle: string): string {
  const base = handle.replace(/[^a-zA-Z0-9.-]/g, "").toLowerCase();
  const proposed = `${base}-agent`;
  // Most IRC servers cap nicks at 32; freeq is permissive but keep it sane.
  return proposed.length > 30 ? proposed.slice(0, 30) : proposed;
}

export async function runDaemon(opts: DaemonOptions = {}): Promise<Connected> {
  const agent = await loadOrCreateIdentity();
  const owner = await loadOrPromptOwner();
  const delegation = await loadOrMintDelegation({ agent, owner });
  const nick = opts.nick ?? deriveDefaultNick(owner.handle);

  console.log("─── freeqcc daemon ───");
  console.log(`agent DID:      ${agent.did}${agent.isFresh ? " (fresh)" : ""}`);
  console.log(`owner:          @${owner.handle} (${owner.did})`);
  console.log(`delegation:     ${delegation.signature ? "signed" : "unsigned (v1.0)"}`);
  console.log(`server:         ${opts.serverUrl ?? "wss://irc.freeq.at/irc"}`);
  console.log(`nick:           ${nick}`);
  console.log("──────────────────────");

  const conn = await connect({
    identity: agent,
    owner,
    delegation,
    nick,
    serverUrl: opts.serverUrl,
  });

  console.log(`✓ connected as ${conn.nick}`);
  console.log(`  DM @${conn.nick} from @${owner.handle} to talk to your local Claude Code.`);

  // ── Bot's own channel ──
  // The freeq web client creates / uses #<bot-nick> as the conversation
  // surface for bots, so messages a user thinks they're DMing actually
  // land there. Auto-join so we receive them. Channel messages from the
  // owner are routed through the same gate as DMs.
  const botChannel = `#${conn.nick}`;
  conn.client.raw(`JOIN ${botChannel}`);
  console.log(`  Auto-joined ${botChannel} (alternate conversation surface).`);

  // ── Raw wire debug (FREEQCC_DEBUG_RAW=1) ──
  if (process.env.FREEQCC_DEBUG_RAW === "1") {
    conn.client.on("raw", (line: string) => {
      // Dump every incoming line. Noisy but invaluable when DMs go missing.
      console.log(`[raw] ${line}`);
    });
  }

  // ── Owner-DID gate + claude dispatch (phase 6) ──
  //
  // We track sender DIDs via the SDK's `memberDid` event (fires on WHOIS).
  // For an unknown sender we fire WHOIS, queue the message for up to 3s,
  // and dispatch (or refuse) once the DID resolves.
  const gateState: GateState = await loadGateState();
  const allowlist: AllowlistEntry[] = await loadAllowlist();
  if (allowlist.length > 0) {
    console.log(`allowlist:      ${allowlist.length} extra DID(s) allowed beyond owner`);
    for (const e of allowlist) {
      console.log(`  - ${e.did}${e.label ? ` (${e.label})` : ""}`);
    }
  }
  const allowedDids = allowlist.map((e) => e.did);
  const persistGate = (): void => {
    saveGateState(gateState).catch((err) => {
      console.warn(`[gate] persist failed: ${(err as Error).message}`);
    });
  };
  const nickToDid = new Map<string, string>(); // case-insensitive: stored lowercase
  const pendingByNick = new Map<string, Array<() => void>>();
  let totalCostUsd = 0;
  let dispatchCount = 0;

  conn.client.on("memberDid", (nick: string, did: string) => {
    nickToDid.set(nick.toLowerCase(), did);
    const queued = pendingByNick.get(nick.toLowerCase());
    if (queued) {
      pendingByNick.delete(nick.toLowerCase());
      for (const cb of queued) cb();
    }
  });

  // Pre-warm the cache for the owner so the first owner DM doesn't pay
  // a WHOIS round-trip.
  conn.client.raw(`WHOIS ${owner.handle}`);

  const handleDm = async (
    fromNick: string,
    text: string,
    msgTags: Record<string, string>,
    replyTarget: string,
  ): Promise<void> => {
    // Try to resolve sender DID synchronously: account-tag, then cache.
    const didFromTag = msgTags["account"];
    let senderDid: string | null =
      (didFromTag && didFromTag.startsWith("did:") ? didFromTag : null) ??
      nickToDid.get(fromNick.toLowerCase()) ??
      null;

    if (!senderDid) {
      // Fire WHOIS, wait up to 3s, then re-dispatch.
      conn.client.raw(`WHOIS ${fromNick}`);
      const arrived = await new Promise<boolean>((resolve) => {
        const timer = setTimeout(() => resolve(false), 3000);
        const queue =
          pendingByNick.get(fromNick.toLowerCase()) ?? [];
        queue.push(() => {
          clearTimeout(timer);
          resolve(true);
        });
        pendingByNick.set(fromNick.toLowerCase(), queue);
      });
      if (arrived) {
        senderDid = nickToDid.get(fromNick.toLowerCase()) ?? null;
      }
    }

    const decision = evaluate({
      state: gateState,
      senderDid,
      senderNick: fromNick,
      ownerDid: owner.did,
      allowedDids,
    });

    if (decision.kind === "silent") {
      return;
    }
    if (decision.kind === "refuse") {
      const allowlistMode = allowedDids.length > 0;
      const refusalText = senderDid
        ? `I'm @${owner.handle}'s agent. I only respond to ${allowlistMode ? "owner + allowlisted DIDs" : "them"}.`
        : `I'm @${owner.handle}'s agent and I couldn't verify your identity. I only respond to authenticated users on @${owner.handle}'s allowlist.`;
      conn.client.sendMessage(replyTarget, refusalText);
      await logRefused({
        ts: new Date().toISOString(),
        fromNick,
        fromDid: senderDid,
        text: text.slice(0, 200),
        reason: decision.reason,
      });
      console.log(`[refused] ${fromNick} (${senderDid ?? "no-did"}): ${decision.reason}`);
      persistGate();
      return;
    }

    // Dispatch — set executing presence, stream claude into the chat as it
    // produces tokens, then mark final.
    conn.client.raw("PRESENCE :state=executing;status=responding to owner");
    console.log(`[dispatch] ${fromNick} → ${replyTarget}: ${text.slice(0, 80)}`);
    dispatchCount++;
    persistGate();

    // Streaming bookkeeping. The first PRIVMSG carries +freeq.at/streaming=1
    // and the SDK auto-includes msgid in tags. Server overwrites msgid with
    // its own; we recover the assigned msgid via echo-message (SDK emits a
    // self message event with msg.id once the server has acked).
    let streamMsgId: string | null = null;
    let streamSent = false;
    let pendingFlush: NodeJS.Timeout | null = null;
    let lastFlushedText = "";
    let latestText = "";
    const FLUSH_INTERVAL_MS = 500;

    // Listen for our own echo to capture the server-assigned msgid.
    const onEcho = (
      _channel: string,
      msg: { id?: string; isSelf?: boolean; tags?: Record<string, string> },
    ): void => {
      if (!msg.isSelf || streamMsgId) return;
      if (msg.tags?.["+freeq.at/streaming"] !== "1") return;
      if (msg.id) streamMsgId = msg.id;
    };
    conn.client.on("message", onEcho);

    const sendStreamingPrivmsg = (chunkText: string): void => {
      // Initial: tag streaming=1, no edit ref. SDK assigns msgid via echo.
      const safe = chunkText.replace(/[\r\n]/g, " ").slice(0, 1500) || "…";
      conn.client.raw(
        `@+freeq.at/streaming=1 PRIVMSG ${replyTarget} :${safe}`,
      );
      streamSent = true;
      lastFlushedText = chunkText;
    };

    const sendStreamingEdit = (chunkText: string, isFinal: boolean): void => {
      if (!streamMsgId) return; // can't edit without msgid yet
      const safe = chunkText.replace(/[\r\n]/g, " ").slice(0, 1500) || "…";
      const tag = isFinal
        ? `+draft/edit=${streamMsgId}`
        : `+draft/edit=${streamMsgId};+freeq.at/streaming=1`;
      conn.client.raw(`@${tag} PRIVMSG ${replyTarget} :${safe}`);
      lastFlushedText = chunkText;
    };

    const flush = (isFinal: boolean): void => {
      if (latestText === lastFlushedText && !isFinal) return;
      if (!streamSent) {
        sendStreamingPrivmsg(latestText);
      } else {
        sendStreamingEdit(latestText, isFinal);
      }
    };

    const scheduleFlush = (): void => {
      if (pendingFlush) return;
      pendingFlush = setTimeout(() => {
        pendingFlush = null;
        flush(false);
      }, FLUSH_INTERVAL_MS);
    };

    try {
      await dispatchToClaudeStreaming(text, {
        onChunk: (accumulated) => {
          latestText = accumulated;
          if (!streamSent) {
            // First chunk: send immediately so msgid lookup starts.
            sendStreamingPrivmsg(accumulated);
          } else {
            scheduleFlush();
          }
        },
        onComplete: async (final, _sessionId, durationMs, costUsd) => {
          if (typeof costUsd === "number" && Number.isFinite(costUsd)) {
            totalCostUsd += costUsd;
            // Record per-conversation cost so /freeqcc status can show it.
            void persistDispatchTelemetry({
              dispatchCount,
              totalCostUsd,
              lastDispatchCostUsd: costUsd,
              lastDispatchAt: new Date().toISOString(),
            });
          }
          if (pendingFlush) {
            clearTimeout(pendingFlush);
            pendingFlush = null;
          }
          latestText = final || latestText || "(claude returned empty)";
          if (!streamSent) {
            // Never streamed (model returned only the result event).
            // Fire one PRIVMSG with the final text, no streaming tag.
            const safe = latestText.replace(/[\r\n]/g, " ").slice(0, 1500);
            conn.client.raw(`PRIVMSG ${replyTarget} :${safe}`);
          } else {
            // We sent a streaming PRIVMSG but the echo with the server-
            // assigned msgid may not have arrived yet. Wait up to 2s; if
            // no msgid, fall back to a fresh PRIVMSG (the streaming-tagged
            // first chunk will look incomplete to clients, but at least
            // the user gets the final reply).
            const deadline = Date.now() + 2000;
            while (!streamMsgId && Date.now() < deadline) {
              await new Promise((r) => setTimeout(r, 50));
            }
            if (streamMsgId) {
              flush(true); // final edit clears the streaming tag
            } else {
              const safe = latestText.replace(/[\r\n]/g, " ").slice(0, 1500);
              conn.client.raw(`PRIVMSG ${replyTarget} :${safe}`);
              console.warn(
                `[stream] msgid never arrived via echo — sent fallback PRIVMSG`,
              );
            }
          }
          console.log(
            `[reply] ${durationMs}ms: ${latestText.slice(0, 80)}`,
          );
        },
        onError: (err) => {
          if (pendingFlush) {
            clearTimeout(pendingFlush);
            pendingFlush = null;
          }
          const message = (err as Error).message;
          if (streamSent && streamMsgId) {
            conn.client.raw(
              `@+draft/edit=${streamMsgId} PRIVMSG ${replyTarget} :(claude error: ${message.slice(0, 200)})`,
            );
          } else {
            conn.client.sendMessage(
              replyTarget,
              `(error invoking claude: ${message.slice(0, 200)})`,
            );
          }
          console.error(`[dispatch error]`, err);
        },
      });
    } finally {
      conn.client.off("message", onEcho);
      conn.client.raw("PRESENCE :state=online");
    }
  };

  // Per-channel cooldown for @mention replies in non-bot channels.
  const channelMentionCooldown = new Map<string, number>();
  const MENTION_COOLDOWN_MS = 60_000;

  const isMention = (text: string): boolean => {
    const lower = text.toLowerCase();
    const nickLower = conn.nick.toLowerCase();
    return (
      lower.includes(`@${nickLower}`) ||
      // Some clients omit the @; match a word-boundary nick so "yokota-bot" matches
      // but "yokota-bot-something" doesn't.
      new RegExp(`(^|\\s)${nickLower.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\\\$&")}(\\s|[,.!?:;]|$)`, "i").test(text)
    );
  };

  conn.client.on(
    "message",
    (
      channel: string,
      msg: {
        from: string;
        text: string;
        isSelf?: boolean;
        tags?: Record<string, string>;
      },
    ) => {
      if (msg.isSelf) return;
      const isChannel = channel.startsWith("#") || channel.startsWith("&");

      if (!isChannel) {
        // Direct DM — sender nick is the conversation partner.
        void handleDm(msg.from, msg.text, msg.tags ?? {}, msg.from).catch((e) => {
          console.error("[handleDm error]", e);
        });
        return;
      }

      // Channel message. The bot's own channel (#<bot-nick>) is treated as
      // a DM surface — every message goes through the gate as if it were a
      // DM. This mirrors how freeq web clients deliver "DMs to a bot".
      if (channel.toLowerCase() === botChannel.toLowerCase()) {
        void handleDm(msg.from, msg.text, msg.tags ?? {}, channel).catch((e) => {
          console.error("[handleDm error]", e);
        });
        return;
      }

      // Other channels: only respond when explicitly @mentioned. Per-channel
      // 60s cooldown to prevent the bot from replying to every message in
      // a busy channel during a thread it's involved in.
      if (!isMention(msg.text)) return;
      const lastReply = channelMentionCooldown.get(channel.toLowerCase()) ?? 0;
      if (Date.now() - lastReply < MENTION_COOLDOWN_MS) {
        console.log(`[mention cooldown] ${channel}: silent (last reply ${Math.round((Date.now() - lastReply) / 1000)}s ago)`);
        return;
      }
      channelMentionCooldown.set(channel.toLowerCase(), Date.now());
      // Strip the @<bot-nick> prefix from the text so claude doesn't see it
      const stripped = msg.text
        .replace(new RegExp(`@?${conn.nick.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\\\$&")}\\b[,:]?\\s*`, "i"), "")
        .trim();
      void handleDm(msg.from, stripped || msg.text, msg.tags ?? {}, channel).catch((e) => {
        console.error("[handleDm error]", e);
      });
    },
  );

  // Graceful shutdown on SIGINT/SIGTERM. Also wipes the pid file —
  // cli.ts can't rely on a finally-block here because process.exit(0)
  // bypasses outer try/finally.
  const shutdown = async (sig: string): Promise<void> => {
    console.log(`\n[${sig}] shutting down...`);
    await conn.stop(`signal ${sig}`);
    try {
      const { unlink } = await import("node:fs/promises");
      await unlink(paths.daemonPid);
    } catch {
      // already gone
    }
    process.exit(0);
  };
  process.once("SIGINT", () => void shutdown("SIGINT"));
  process.once("SIGTERM", () => void shutdown("SIGTERM"));

  return conn;
}
