#!/usr/bin/env node
// freeqcc CLI: launch | status | stop | doctor.
import { Command } from "commander";
import prompts from "prompts";
import { execSync } from "node:child_process";
import { readFile, writeFile, unlink, stat } from "node:fs/promises";
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
  .action(async (opts: { nick?: string; server?: string }) => {
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
  .description("Stop the running daemon (clean QUIT).")
  .action(async () => {
    const pid = await readPid();
    if (!pid) {
      console.log("No daemon is running (no pid file or process is dead).");
      return;
    }
    try {
      process.kill(pid, "SIGTERM");
      console.log(`Sent SIGTERM to pid ${pid}.`);
    } catch (err) {
      const code = (err as NodeJS.ErrnoException).code;
      if (code === "ESRCH") {
        console.log(`Pid ${pid} is gone; cleaning up stale pid file.`);
        await unlink(paths.daemonPid).catch(() => {});
      } else {
        throw err;
      }
    }
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

    // Server reachability
    try {
      const r = await fetch("https://irc.freeq.at/api/v1/health");
      if (r.ok) ok(`server reachable: ${r.status}`);
      else fail(`server health: ${r.status}`);
    } catch (e) {
      fail(`server unreachable: ${(e as Error).message}`);
    }

    console.log(problems === 0 ? "\nAll checks passed." : `\n${problems} problem(s) found.`);
    process.exitCode = problems === 0 ? 0 : 1;
  });

// ── helpers ──────────────────────────────────────────────────────────

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
