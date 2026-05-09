// Long-lived freeqcc daemon: load identity + owner + delegation,
// connect to freeq, listen for DMs, dispatch owner DMs to claude.
//
// Phase 5 wires connect + announce + heartbeat. The DM gate + claude
// dispatch (phase 6) hangs off the returned client.
import { loadOrCreateIdentity } from "./identity.js";
import { loadOrPromptOwner } from "./owner.js";
import { loadOrMintDelegation } from "./delegation.js";
import { connect, type Connected } from "./connect.js";
import { evaluate, newGateState, type GateState } from "./gate.js";
import { dispatchToClaude } from "./dispatch.js";
import { logRefused } from "./audit.js";

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

  // ── Owner-DID gate + claude dispatch (phase 6) ──
  //
  // We track sender DIDs via the SDK's `memberDid` event (fires on WHOIS).
  // For an unknown sender we fire WHOIS, queue the message for up to 3s,
  // and dispatch (or refuse) once the DID resolves.
  const gateState: GateState = newGateState();
  const nickToDid = new Map<string, string>(); // case-insensitive: stored lowercase
  const pendingByNick = new Map<string, Array<() => void>>();

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
    });

    if (decision.kind === "silent") {
      return;
    }
    if (decision.kind === "refuse") {
      const refusalText = senderDid
        ? `I'm @${owner.handle}'s agent. I only respond to them.`
        : `I'm @${owner.handle}'s agent and I couldn't verify your identity. I only respond to authenticated users on @${owner.handle}'s allowlist.`;
      conn.client.sendMessage(fromNick, refusalText);
      await logRefused({
        ts: new Date().toISOString(),
        fromNick,
        fromDid: senderDid,
        text: text.slice(0, 200),
        reason: decision.reason,
      });
      console.log(`[refused] ${fromNick} (${senderDid ?? "no-did"}): ${decision.reason}`);
      return;
    }

    // Dispatch — set executing presence, run claude, send reply, set idle.
    conn.client.raw("PRESENCE :state=executing;status=responding to owner");
    try {
      console.log(`[dispatch] ${fromNick}: ${text.slice(0, 80)}`);
      const { reply, durationMs } = await dispatchToClaude(text);
      const safeReply = reply || "(claude returned empty)";
      conn.client.sendMessage(fromNick, safeReply);
      console.log(`[reply] ${durationMs}ms: ${safeReply.slice(0, 80)}`);
    } catch (err) {
      const message = (err as Error).message;
      conn.client.sendMessage(fromNick, `(error invoking claude: ${message.slice(0, 200)})`);
      console.error(`[dispatch error]`, err);
    } finally {
      conn.client.raw("PRESENCE :state=online");
    }
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
      if (channel.startsWith("#") || channel.startsWith("&")) return;
      // channel === fromNick on a DM. Use msg.from as the canonical sender.
      void handleDm(msg.from, msg.text, msg.tags ?? {}).catch((e) => {
        console.error("[handleDm error]", e);
      });
    },
  );

  // Graceful shutdown on SIGINT/SIGTERM
  const shutdown = async (sig: string): Promise<void> => {
    console.log(`\n[${sig}] shutting down...`);
    await conn.stop(`signal ${sig}`);
    process.exit(0);
  };
  process.once("SIGINT", () => void shutdown("SIGINT"));
  process.once("SIGTERM", () => void shutdown("SIGTERM"));

  return conn;
}
