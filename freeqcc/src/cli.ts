#!/usr/bin/env node
// freeqcc CLI: launch | stop | status | doctor | tail come from
// @freeq/bot-kit's createDaemonCLI scaffold. The freeqcc-specific
// subcommands — grants/grant/revoke/send/rotate-key — are layered on
// top of the returned Command.

// (E2EE polyfill removed: it advanced past 'indexedDB is not defined'
// but tripped over the SDK's exportKey('raw', x25519PrivateKey) which
// Node 22 doesn't support, sending the daemon into a reconnect loop.
// E2EE in Node remains v1.1 work — needs SDK refactor to JWK round-
// trips. v1.0 keeps plaintext DMs; SDK fallback handles it gracefully.)

import prompts from "prompts";
import { execSync } from "node:child_process";
import { readFile, unlink } from "node:fs/promises";
import {
  createDaemonCLI,
  loadDelegation,
  readPidIfAlive,
  type DoctorCheck,
} from "@freeq/bot-kit";
import { paths } from "./paths.js";
import { loadConfig, saveConfig } from "./config.js";
import { loadOrPromptOwner } from "./owner.js";
import { runDaemon } from "./daemon.js";

interface DaemonOpts {
  nick: string;
  serverUrl?: string;
}

const program = createDaemonCLI<DaemonOpts>({
  name: "freeqcc",
  paths: {
    dir: paths.dir,
    daemonPid: paths.daemonPid,
    daemonLog: paths.daemonLog,
    agentKey: paths.agentKey,
    delegation: paths.delegation,
  },
  launchOptions: [
    {
      flags: "--nick <nick>",
      description: "Override the bot nick (otherwise loaded from config or prompted)",
    },
    { flags: "--server <url>", description: "Override the freeq WebSocket URL" },
  ],
  preflight: async (parsed) => {
    const opts = parsed as { nick?: string; server?: string };
    const owner = await loadOrPromptOwner();
    const config = await loadConfig();
    const cliOverride = opts.nick;
    const stored = config?.nick;
    let nick = cliOverride ?? stored;

    if (!nick) {
      const suggested = `${owner.handle
        .replace(/[^a-zA-Z0-9.-]/g, "")
        .toLowerCase()}-agent`;
      console.log("");
      console.log("What should your agent be called?");
      console.log(
        `  Default would be \`${suggested}\`, but a name you pick is more`,
      );
      console.log(
        "  memorable — e.g. dev-buddy, code-helper, sourdough-bot, copilot-jr.",
      );
      console.log("");
      const resp = await prompts(
        {
          type: "text",
          name: "nick",
          message: "Bot nick",
          initial: suggested,
          validate: (v: string) =>
            /^[a-zA-Z0-9._-]+$/.test(v.trim())
              ? true
              : "Use letters, digits, dot, underscore, dash only.",
        },
        {
          onCancel: () => {
            throw new Error("Cancelled.");
          },
        },
      );
      nick = String(resp.nick).trim();
      await saveConfig({ nick, serverUrl: opts.server ?? config?.serverUrl });
    } else if (cliOverride && cliOverride !== stored) {
      // CLI --nick differs from stored config — persist the new one.
      await saveConfig({ nick, serverUrl: opts.server ?? config?.serverUrl });
    }

    return { nick, serverUrl: opts.server ?? config?.serverUrl };
  },
  runDaemon: async (opts) => {
    return await runDaemon({ nick: opts.nick, serverUrl: opts.serverUrl });
  },
  statusExtras: async () => {
    const lines: string[] = [];
    const config = await loadConfig();
    const owner = await safeLoadOwner();
    lines.push(`bot nick:       ${config?.nick ?? "(not configured)"}`);
    lines.push(
      `owner:          ${owner ? `@${owner.handle} (${owner.did})` : "(not configured)"}`,
    );
    lines.push(
      `server:         ${config?.serverUrl ?? "wss://irc.freeq.at/irc (default)"}`,
    );

    // Telemetry: how many dispatches, total claude API cost
    const telemetry = await safeReadJson<{
      dispatchCount?: number;
      totalCostUsd?: number;
      lastDispatchCostUsd?: number;
      lastDispatchAt?: string;
    }>(paths.dir + "/telemetry.json");
    if (telemetry) {
      const cost = (telemetry.totalCostUsd ?? 0).toFixed(4);
      const last = telemetry.lastDispatchCostUsd?.toFixed(4) ?? "—";
      lines.push(
        `dispatches:     ${telemetry.dispatchCount ?? 0} total ($${cost} cumulative, $${last} last)`,
      );
      if (telemetry.lastDispatchAt) {
        lines.push(`last dispatch:  ${telemetry.lastDispatchAt}`);
      }
    }
    return lines;
  },
  actorStatusUrl: (did) =>
    `https://irc.freeq.at/api/v1/actors/${encodeURIComponent(did)}`,
  doctorChecks: buildDoctorChecks(),
});

// ── bot-specific subcommands ─────────────────────────────────────────

program
  .command("grants")
  .description("List allowlisted DIDs and the actions each is granted.")
  .action(async () => {
    const { createAccessMap, OWNER_ACTIONS } = await import("./allowlist.js");
    const owner = await safeLoadOwner();
    if (owner) {
      console.log(`owner:    ${owner.did}  [${OWNER_ACTIONS.join(", ")}]`);
    }
    const access = await createAccessMap(paths.allowlist);
    const entries = access.list();
    if (entries.length === 0) {
      console.log("(no extra DIDs in allowlist)");
      access.close();
      return;
    }
    for (const e of entries) {
      const acts = e.actions && e.actions.length > 0 ? e.actions.join(", ") : "chat-only";
      console.log(`${e.did}${e.label ? `  (${e.label})` : ""}  [${acts}]`);
    }
    access.close();
  });

program
  .command("grant <did> <action>")
  .description(
    "Grant <did> the right to invoke <action>. Adds the entry if new. " +
      "Action must be one of: join, part, privmsg-user, privmsg-channel, " +
      "notice-user, notice-channel, nick.",
  )
  .option("--label <label>", "Optional human-readable label")
  .action(async (did: string, action: string, opts: { label?: string }) => {
    const { createAccessMap, ALL_ACTIONS } = await import("./allowlist.js");
    if (!ALL_ACTIONS.includes(action)) {
      console.error(
        `unknown action '${action}'. Known: ${ALL_ACTIONS.join(", ")}.`,
      );
      process.exit(1);
    }
    if (!did.startsWith("did:")) {
      console.error(`'${did}' doesn't look like a DID (expected did:plc:… or did:key:…)`);
      process.exit(1);
    }
    const access = await createAccessMap(paths.allowlist);
    const existing = access.get(did);
    const entry = existing
      ? {
          did,
          label: existing.label ?? opts.label,
          actions: existing.actions ?? [],
        }
      : { did, label: opts.label, actions: [] };
    if (!entry.actions!.includes(action)) entry.actions!.push(action);
    await access.set(entry);
    console.log(
      `granted ${action} to ${did}${entry.label ? ` (${entry.label})` : ""}`,
    );
    console.log("(running daemon reloads the allowlist automatically)");
    access.close();
  });

program
  .command("revoke <did> [action]")
  .description(
    "Revoke a single <action> from <did>, or remove the entry entirely if no action is given.",
  )
  .action(async (did: string, action: string | undefined) => {
    const { createAccessMap } = await import("./allowlist.js");
    const access = await createAccessMap(paths.allowlist);
    const entry = access.get(did);
    if (!entry) {
      console.log(`no allowlist entry for ${did}`);
      access.close();
      return;
    }
    if (action) {
      const before = entry.actions?.length ?? 0;
      const nextActions = (entry.actions ?? []).filter((a) => a !== action);
      if (nextActions.length === before) {
        console.log(`${did} didn't have action '${action}'`);
        access.close();
        return;
      }
      await access.set({ ...entry, actions: nextActions });
      console.log(`revoked ${action} from ${did}`);
    } else {
      await access.delete(did);
      console.log(`removed ${did} from allowlist`);
    }
    console.log("(running daemon reloads the allowlist automatically)");
    access.close();
  });

program
  .command("send <action> [args...]")
  .description(
    "Run an IRC action against the running daemon. Reads the dispatch capability " +
      "token from $FREEQCC_DISPATCH_TOKEN and the socket path from $FREEQCC_CONTROL_SOCK; " +
      "those are set automatically inside a claude -p subprocess spawned by the daemon. " +
      "Outside that context, this command exits 2.",
  )
  .action(async (action: string, args: string[]) => {
    const sock = process.env.FREEQCC_CONTROL_SOCK;
    const token = process.env.FREEQCC_DISPATCH_TOKEN;
    if (!sock || !token) {
      console.error(
        "freeqcc send: FREEQCC_CONTROL_SOCK and FREEQCC_DISPATCH_TOKEN must be set\n" +
          "(this command runs inside a daemon-spawned claude subprocess; not standalone).",
      );
      process.exit(2);
    }
    const { callControl } = await import("./control.js");
    let resp;
    try {
      resp = await callControl({ token, action, args }, sock);
    } catch (err) {
      console.error(`freeqcc send: ${(err as Error).message}`);
      process.exit(1);
    }
    if (resp.ok) {
      if (resp.result !== undefined) {
        console.log(JSON.stringify(resp.result));
      }
      process.exit(0);
    } else {
      console.error(`freeqcc send: ${resp.error ?? "unknown error"}`);
      process.exit(1);
    }
  });

program
  .command("rotate-key")
  .description("Rotate the agent's did:key identity. Daemon must be stopped first.")
  .option("--force", "Skip the confirmation prompt")
  .action(async (opts: { force?: boolean }) => {
    const pid = await readPidIfAlive(paths.daemonPid);
    if (pid) {
      console.error(`Daemon is running (pid ${pid}). Run 'freeqcc stop' first.`);
      process.exit(1);
    }
    if (!opts.force) {
      const r = await prompts({
        type: "confirm",
        name: "ok",
        message:
          "Rotate the agent's did:key identity? This regenerates agent.key " +
          "and re-mints delegation.json under a new DID. The bot loses any " +
          "existing channel ops, friends-list, etc. tied to the old DID.",
        initial: false,
      });
      if (!r.ok) {
        console.log("Cancelled.");
        return;
      }
    }
    for (const path of [paths.agentKey, paths.delegation]) {
      try {
        await unlink(path);
        console.log(`removed ${path}`);
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
          throw err;
        }
      }
    }
    // Wipe per-DID claude sessions — they're keyed to the old DID.
    try {
      const { rm } = await import("node:fs/promises");
      await rm(paths.sessionsDir, { recursive: true, force: true });
      console.log(`removed ${paths.sessionsDir}`);
    } catch {
      // best-effort
    }
    console.log(
      "\nDone. Next 'freeqcc launch' generates a fresh did:key and delegation cert.",
    );
  });

// ── doctor checks: freeqcc-specific layer atop the scaffold's built-ins ─

function buildDoctorChecks(): DoctorCheck[] {
  return [
    {
      name: "owner",
      run: async () => {
        const o = await safeLoadOwner();
        return o
          ? { ok: true, detail: `@${o.handle} → ${o.did}` }
          : { ok: false, reason: `no owner.json — run 'freeqcc launch' to set` };
      },
    },
    {
      name: "config: bot nick",
      run: async () => {
        const c = await loadConfig();
        return c?.nick
          ? { ok: true, detail: c.nick }
          : { ok: false, reason: "no config.json or missing nick" };
      },
    },
    {
      name: "owner ↔ delegation match",
      run: async () => {
        const o = await safeLoadOwner();
        const cert = await loadDelegation({ certPath: paths.delegation }).catch(
          () => null,
        );
        if (!o || !cert) {
          return {
            ok: "warn",
            reason: "cannot cross-check (missing owner or cert)",
          };
        }
        return cert.creator_did === o.did
          ? { ok: true }
          : {
              ok: false,
              reason: `delegation.creator_did (${cert.creator_did}) ≠ owner.did (${o.did})`,
            };
      },
    },
    {
      name: "claude binary",
      run: async () => {
        try {
          const path = execSync("command -v claude", { encoding: "utf8" }).trim();
          return { ok: true, detail: path };
        } catch {
          return {
            ok: false,
            reason: "claude is not on PATH — install Claude Code from https://claude.ai/code",
          };
        }
      },
    },
    {
      name: "server health endpoint",
      run: async () => {
        try {
          const r = await fetch("https://irc.freeq.at/api/v1/health");
          return r.ok
            ? { ok: true, detail: `${r.status}` }
            : { ok: false, reason: `${r.status}` };
        } catch (e) {
          return { ok: false, reason: `unreachable: ${(e as Error).message}` };
        }
      },
    },
  ];
}

// ── helpers used by statusExtras + doctorChecks ──────────────────────

async function safeReadJson<T>(path: string): Promise<T | null> {
  try {
    const raw = await readFile(path, "utf8");
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

async function safeLoadOwner(): Promise<{ handle: string; did: string } | null> {
  try {
    const raw = await readFile(paths.owner, "utf8");
    const o = JSON.parse(raw) as { handle: string; did: string };
    if (typeof o.handle === "string" && typeof o.did === "string") return o;
    return null;
  } catch {
    return null;
  }
}

program.parseAsync(process.argv).catch((err) => {
  console.error(err);
  process.exit(1);
});
