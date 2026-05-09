#!/usr/bin/env node
// freeqcc CLI: launch | status | stop | doctor.

// (E2EE polyfill removed: it advanced past 'indexedDB is not defined'
// but tripped over the SDK's exportKey('raw', x25519PrivateKey) which
// Node 22 doesn't support, sending the daemon into a reconnect loop.
// E2EE in Node remains v1.1 work — needs SDK refactor to JWK round-
// trips. v1.0 keeps plaintext DMs; SDK fallback handles it gracefully.)

import { Command } from "commander";
import prompts from "prompts";
import { execSync, spawn } from "node:child_process";
import { open, readFile, writeFile, unlink, stat } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";
import { loadConfig, saveConfig } from "./config.js";
import { loadOrPromptOwner } from "./owner.js";
import { loadOrCreateIdentity } from "./identity.js";
import { loadDelegation } from "./delegation.js";
import { runDaemon } from "./daemon.js";

const program = new Command();
program
  .name("freeqcc")
  .description("freeq + Claude Code: launch a freeq-DM-controllable Claude Code agent.")
  .version("0.1.0");

// ── launch ───────────────────────────────────────────────────────────

program
  .command("launch")
  .description("Launch the freeqcc agent (first run prompts for handle + bot nick).")
  .option("--nick <nick>", "Override the bot nick (otherwise loaded from config or prompted)")
  .option("--server <url>", "Override the freeq WebSocket URL")
  .option(
    "--detach",
    "Fork into the background (logs to ~/.freeqcc/daemon.log). Prompts complete in the foreground first.",
  )
  .action(async (opts: { nick?: string; server?: string; detach?: boolean }) => {
    const owner = await loadOrPromptOwner();
    const config = await loadConfig();
    const cliOverride = opts.nick;
    const stored = config?.nick;
    let nick = cliOverride ?? stored;

    if (!nick) {
      const suggested = `${owner.handle.replace(/[^a-zA-Z0-9.-]/g, "").toLowerCase()}-agent`;
      console.log("");
      console.log(`What should your agent be called?`);
      console.log(`  Default would be \`${suggested}\`, but a name you pick is more`);
      console.log(`  memorable — e.g. dev-buddy, code-helper, sourdough-bot, copilot-jr.`);
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
      // User passed --nick that differs from stored config — persist the new one.
      await saveConfig({ nick, serverUrl: opts.server ?? config?.serverUrl });
    }

    if (opts.detach) {
      // Fork a fresh `freeqcc launch` subprocess (without --detach), wired
      // to the log file. Parent exits after printing the child pid; child
      // writes its own pid file inside the spawned action.
      await ensureDir();
      const logFh = await open(paths.daemonLog, "a", 0o600);
      const args = process.argv.slice(2).filter((a) => a !== "--detach");
      const child = spawn(process.argv0, [process.argv[1], ...args], {
        detached: true,
        stdio: ["ignore", logFh.fd, logFh.fd],
        env: { ...process.env, FREEQCC_DETACHED: "1" },
      });
      child.unref();
      await logFh.close();
      console.log(`freeqcc launched (pid ${child.pid}); logs → ${paths.daemonLog}`);
      console.log(`  freeqcc status   — show live state`);
      console.log(`  freeqcc stop     — clean shutdown`);
      return;
    }

    // Write our pid before connecting so `freeqcc status` and `stop` work.
    await ensureDir();
    await writeFile(paths.daemonPid, String(process.pid) + "\n", { mode: 0o600 });

    try {
      const conn = await runDaemon({ nick, serverUrl: opts.server ?? config?.serverUrl });
      // Block forever — runDaemon installed SIGINT/SIGTERM handlers.
      await new Promise(() => {
        void conn;
      });
    } finally {
      // Clean up pid file on exit (best-effort)
      try {
        await unlink(paths.daemonPid);
      } catch {
        // ignore
      }
    }
  });

// ── status ───────────────────────────────────────────────────────────

program
  .command("status")
  .description("Show daemon status: connected, owner verified, bot DID/nick, last DM.")
  .action(async () => {
    const config = await loadConfig();
    const owner = await safeLoadOwner();
    const agent = await safeLoadIdentity();
    const cert = await loadDelegation().catch(() => null);
    const pidAlive = await readPid();

    console.log("─── freeqcc status ───");
    console.log(`pid file:       ${paths.daemonPid}`);
    console.log(`daemon:         ${pidAlive ? `running (pid ${pidAlive})` : "not running"}`);
    console.log(`bot nick:       ${config?.nick ?? "(not configured)"}`);
    console.log(`owner:          ${owner ? `@${owner.handle} (${owner.did})` : "(not configured)"}`);
    console.log(`agent DID:      ${agent?.did ?? "(no agent.key)"}`);
    console.log(
      `delegation:     ${cert ? (cert.signature ? "signed" : "unsigned (v1.0)") : "(none)"}`,
    );
    console.log(`server:         ${config?.serverUrl ?? "wss://irc.freeq.at/irc (default)"}`);

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
      console.log(`dispatches:     ${telemetry.dispatchCount ?? 0} total ($${cost} cumulative, $${last} last)`);
      if (telemetry.lastDispatchAt) {
        console.log(`last dispatch:  ${telemetry.lastDispatchAt}`);
      }
    }

    // If running, query the actor endpoint for live state.
    if (pidAlive && agent) {
      const url = `https://irc.freeq.at/api/v1/actors/${encodeURIComponent(agent.did)}`;
      try {
        const resp = await fetch(url);
        if (resp.ok) {
          const json = (await resp.json()) as Record<string, unknown>;
          const provenance = json.provenance as Record<string, unknown> | undefined;
          console.log(`actor.online:   ${json.online}`);
          console.log(`actor.nick:     ${json.nick ?? "(none)"}`);
          // Surface a 433 collision: configured nick != server-confirmed nick.
          // Common case: SDK appended `_` because the requested nick was
          // taken; user should rotate (`freeqcc stop && freeqcc launch
          // --nick something-else --detach`).
          if (
            config?.nick &&
            json.nick &&
            json.nick !== config.nick
          ) {
            console.log(
              `⚠ NICK MISMATCH: config wants '${config.nick}' but server registered us as '${json.nick}'.\n` +
                `  Likely a 433 collision (your preferred nick is taken). Stop the daemon, rerun launch\n` +
                `  with --nick <something-else>, or check who owns it via 'curl https://irc.freeq.at/api/v1/users/${encodeURIComponent(config.nick)}/whois'.`,
            );
          }
          if (provenance) {
            console.log(
              `provenance:     verified=${provenance._verified} (${provenance._verification_reason})`,
            );
          }
        } else {
          console.log(`actor api:      ${resp.status} ${resp.statusText}`);
        }
      } catch (e) {
        console.log(`actor api:      error: ${(e as Error).message}`);
      }
    }
  });

// ── stop ─────────────────────────────────────────────────────────────

program
  .command("stop")
  .description("Stop the running daemon (clean QUIT). Also kills any orphan freeqcc processes.")
  .action(async () => {
    const pidFromFile = await readPid();
    const orphans = findOrphanFreeqccPids().filter((p) => p !== pidFromFile);

    if (!pidFromFile && orphans.length === 0) {
      console.log("No daemon is running.");
      return;
    }

    if (pidFromFile) {
      try {
        process.kill(pidFromFile, "SIGTERM");
        console.log(`Sent SIGTERM to pid ${pidFromFile} (from pid file).`);
      } catch (err) {
        const code = (err as NodeJS.ErrnoException).code;
        if (code === "ESRCH") {
          console.log(`Pid ${pidFromFile} is gone; cleaning up stale pid file.`);
          await unlink(paths.daemonPid).catch(() => {});
        } else {
          throw err;
        }
      }
    }

    if (orphans.length > 0) {
      console.log(
        `Found ${orphans.length} orphan freeqcc process(es) not in the pid file: ${orphans.join(", ")}. SIGTERM'ing.`,
      );
      for (const opid of orphans) {
        try {
          process.kill(opid, "SIGTERM");
        } catch {
          // already gone — fine
        }
      }
    }
  });

// ── grants / grant / revoke ──────────────────────────────────────────
//
// Edits ~/.freeqcc/allowlist.json. The running daemon fs.watches this file
// and reloads on change — no restart needed.

program
  .command("grants")
  .description("List allowlisted DIDs and the actions each is granted.")
  .action(async () => {
    const { loadAllowlist, OWNER_ACTIONS } = await import("./allowlist.js");
    const owner = await safeLoadOwner();
    if (owner) {
      console.log(`owner:    ${owner.did}  [${OWNER_ACTIONS.join(", ")}]`);
    }
    const al = await loadAllowlist();
    if (al.length === 0) {
      console.log("(no extra DIDs in allowlist)");
      return;
    }
    for (const e of al) {
      const acts = e.actions && e.actions.length > 0 ? e.actions.join(", ") : "chat-only";
      console.log(`${e.did}${e.label ? `  (${e.label})` : ""}  [${acts}]`);
    }
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
    const { loadAllowlist, saveAllowlist, ALL_ACTIONS } = await import(
      "./allowlist.js"
    );
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
    const al = await loadAllowlist();
    let entry = al.find((e) => e.did === did);
    if (!entry) {
      entry = { did, label: opts.label, actions: [] };
      al.push(entry);
    } else if (opts.label && !entry.label) {
      entry.label = opts.label;
    }
    entry.actions = entry.actions ?? [];
    if (!entry.actions.includes(action)) entry.actions.push(action);
    await saveAllowlist(al);
    console.log(
      `granted ${action} to ${did}${entry.label ? ` (${entry.label})` : ""}`,
    );
    console.log("(running daemon reloads the allowlist automatically)");
  });

program
  .command("revoke <did> [action]")
  .description(
    "Revoke a single <action> from <did>, or remove the entry entirely if no action is given.",
  )
  .action(async (did: string, action: string | undefined) => {
    const { loadAllowlist, saveAllowlist } = await import("./allowlist.js");
    const al = await loadAllowlist();
    const idx = al.findIndex((e) => e.did === did);
    if (idx === -1) {
      console.log(`no allowlist entry for ${did}`);
      return;
    }
    if (!action) {
      al.splice(idx, 1);
      await saveAllowlist(al);
      console.log(`removed ${did} from allowlist entirely`);
    } else {
      const entry = al[idx];
      const before = entry.actions?.length ?? 0;
      entry.actions = (entry.actions ?? []).filter((a) => a !== action);
      const after = entry.actions.length;
      await saveAllowlist(al);
      if (before === after) console.log(`${did} did not have '${action}'; nothing changed`);
      else console.log(`revoked ${action} from ${did}`);
    }
  });

// ── send (capability-token IRC actions) ──────────────────────────────

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

// ── rotate-key ───────────────────────────────────────────────────────

program
  .command("rotate-key")
  .description("Rotate the agent's did:key identity. Daemon must be stopped first.")
  .option("--force", "Skip the confirmation prompt")
  .action(async (opts: { force?: boolean }) => {
    const pid = await readPid();
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

// ── tail ─────────────────────────────────────────────────────────────

program
  .command("tail")
  .description("Stream the daemon log (~/.freeqcc/daemon.log).")
  .option("-n, --lines <n>", "show the last N lines first", "40")
  .action(async (opts: { lines?: string }) => {
    const { spawn } = await import("node:child_process");
    const lines = opts.lines ?? "40";
    const proc = spawn("tail", ["-F", "-n", lines, paths.daemonLog], {
      stdio: ["ignore", "inherit", "inherit"],
    });
    proc.on("error", (err) => {
      console.error(`tail failed: ${(err as Error).message}`);
      process.exit(1);
    });
    process.once("SIGINT", () => proc.kill("SIGTERM"));
  });

// ── doctor ───────────────────────────────────────────────────────────

program
  .command("doctor")
  .description("Sanity-check config, identity, delegation, owner resolution.")
  .action(async () => {
    let problems = 0;
    const fail = (msg: string): void => {
      console.log(`  ✗ ${msg}`);
      problems++;
    };
    const ok = (msg: string): void => console.log(`  ✓ ${msg}`);

    console.log("─── freeqcc doctor ───");

    // Identity
    const agent = await safeLoadIdentity();
    if (agent) ok(`agent identity: ${agent.did}`);
    else fail(`no agent.key — run 'freeqcc launch' to generate`);

    // Owner
    const owner = await safeLoadOwner();
    if (owner) ok(`owner: @${owner.handle} → ${owner.did}`);
    else fail(`no owner.json — run 'freeqcc launch' to set`);

    // Config
    const config = await loadConfig();
    if (config?.nick) ok(`config: bot nick = ${config.nick}`);
    else fail(`no config.json or missing nick`);

    // Delegation
    const cert = await loadDelegation().catch((e) => {
      fail(`delegation cert is malformed: ${(e as Error).message}`);
      return null;
    });
    if (cert) {
      ok(
        `delegation: ${cert.signature ? "signed" : "unsigned (v1.0)"} ` +
          `(bot=${cert.bot_did}, creator=${cert.creator_did})`,
      );
    } else {
      fail(`no delegation.json — run 'freeqcc launch' to mint`);
    }

    // Cross-check delegation matches identity + owner
    if (cert && agent && cert.bot_did !== agent.did) {
      fail(`delegation.bot_did ≠ agent.did (${cert.bot_did} vs ${agent.did})`);
    }
    if (cert && owner && cert.creator_did !== owner.did) {
      fail(`delegation.creator_did ≠ owner.did (${cert.creator_did} vs ${owner.did})`);
    }

    // claude binary on PATH
    try {
      const path = execSync("command -v claude", { encoding: "utf8" }).trim();
      ok(`claude binary: ${path}`);
    } catch {
      fail(`claude is not on PATH — install Claude Code from https://claude.ai/code`);
    }

    // Server reachability + PKI round-trip check
    try {
      const r = await fetch("https://irc.freeq.at/api/v1/health");
      if (r.ok) ok(`server reachable: ${r.status}`);
      else fail(`server health: ${r.status}`);
    } catch (e) {
      fail(`server unreachable: ${(e as Error).message}`);
    }

    if (agent) {
      try {
        const url = `https://irc.freeq.at/api/v1/actors/${encodeURIComponent(agent.did)}`;
        const r = await fetch(url);
        if (r.status === 404) {
          ok(`actor endpoint: 404 (agent has never connected — that's fine if you haven't launched yet)`);
        } else if (r.ok) {
          const data = (await r.json()) as { online?: boolean; provenance?: { _verified?: boolean; _verification_reason?: string } };
          if (data.online) ok(`server sees agent online`);
          else ok(`server has agent record (currently offline)`);
          if (data.provenance) {
            const v = data.provenance._verified;
            const reason = data.provenance._verification_reason ?? "(no reason)";
            if (v) ok(`provenance verified server-side: ${reason}`);
            else ok(`provenance stored unverified (v1.0 expected): ${reason}`);
          } else {
            ok(`agent has no provenance record (not yet submitted)`);
          }
        } else {
          fail(`actor endpoint returned ${r.status}`);
        }
      } catch (e) {
        fail(`actor endpoint check failed: ${(e as Error).message}`);
      }
    }

    console.log(problems === 0 ? "\nAll checks passed." : `\n${problems} problem(s) found.`);
    process.exitCode = problems === 0 ? 0 : 1;
  });

// ── helpers ──────────────────────────────────────────────────────────

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

async function safeLoadIdentity(): Promise<{ did: string } | null> {
  try {
    await stat(paths.agentKey);
    return await loadOrCreateIdentity();
  } catch {
    return null;
  }
}

/** Find any running `freeqcc launch` processes (across pids — covers
 *  orphans from previous sessions). Excludes the current process and ppid.
 *
 *  We then re-read each candidate's argv via `ps` and require that its argv0
 *  is a `node` (or `freeqcc`) binary AND that `launch` appears as a separate
 *  argv element. Without that, a user running e.g. `vim CHANGELOG.md` that
 *  contains the words "freeqcc launch" — or a forgotten `cat`/`grep` — would
 *  match on -f, and we'd SIGTERM the wrong process.
 */
function findOrphanFreeqccPids(): number[] {
  let candidates: number[];
  try {
    // -f matches the full command line. Each line is a pid.
    const out = execSync("pgrep -f 'freeqcc launch'", { encoding: "utf8" });
    candidates = out
      .split("\n")
      .map((s) => parseInt(s.trim(), 10))
      .filter((n) => Number.isFinite(n) && n !== process.pid && n !== process.ppid);
  } catch {
    // pgrep returns exit 1 when there are no matches.
    return [];
  }
  return candidates.filter((pid) => {
    let argv: string;
    try {
      // -ww disables width truncation; -o args= prints the full command.
      argv = execSync(`ps -ww -o args= -p ${pid}`, { encoding: "utf8" }).trim();
    } catch {
      return false;
    }
    // Tokenize on whitespace. We don't need a full shell parser — just enough
    // to confirm "launch" is its own argv token, not a substring of e.g. a
    // file path inside an editor.
    const tokens = argv.split(/\s+/);
    if (tokens.length < 2) return false;
    const argv0 = tokens[0];
    if (
      !/(^|\/)node$/.test(argv0) &&
      !/(^|\/)freeqcc$/.test(argv0)
    ) {
      // Skip processes whose argv0 isn't a node/freeqcc binary (e.g. an
      // editor opened on a file with "freeqcc launch" in the buffer).
      return false;
    }
    if (!tokens.includes("launch")) return false;
    return true;
  });
}

/** Returns the pid if the daemon is running, else null. */
async function readPid(): Promise<number | null> {
  let raw: string;
  try {
    raw = await readFile(paths.daemonPid, "utf8");
  } catch {
    return null;
  }
  const pid = parseInt(raw.trim(), 10);
  if (!pid) return null;
  try {
    process.kill(pid, 0); // signal 0 = test if alive
    return pid;
  } catch {
    return null;
  }
}

program.parseAsync(process.argv).catch((err) => {
  console.error(err);
  process.exit(1);
});
