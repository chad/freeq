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
    for (const path of [paths.agentKey, paths.delegation, paths.session]) {
      try {
        await unlink(path);
        console.log(`removed ${path}`);
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
          throw err;
        }
      }
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
 *  orphans from previous sessions). Excludes the current process. */
function findOrphanFreeqccPids(): number[] {
  try {
    // -f matches the full command line. Each line is a pid.
    const out = execSync("pgrep -f 'freeqcc launch'", { encoding: "utf8" });
    return out
      .split("\n")
      .map((s) => parseInt(s.trim(), 10))
      .filter((n) => Number.isFinite(n) && n !== process.pid && n !== process.ppid);
  } catch {
    // pgrep returns exit 1 when there are no matches.
    return [];
  }
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
