import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { generateDidKey } from "@freeq/sdk";
import { createDaemonCLI, readPidIfAlive, type DaemonPaths } from "./daemon-cli.js";

async function tmp(): Promise<string> {
  return await mkdtemp(join(tmpdir(), "bot-kit-cli-test-"));
}

function pathsIn(dir: string): DaemonPaths {
  return {
    dir,
    daemonPid: join(dir, "daemon.pid"),
    daemonLog: join(dir, "daemon.log"),
    agentKey: join(dir, "agent.key"),
    delegation: join(dir, "delegation.json"),
  };
}

describe("readPidIfAlive", () => {
  let dir: string;
  beforeEach(async () => {
    dir = await tmp();
  });
  afterEach(async () => {
    await rm(dir, { recursive: true, force: true });
  });

  it("returns null when pid file is missing", async () => {
    expect(await readPidIfAlive(join(dir, "missing.pid"))).toBeNull();
  });

  it("returns null when pid file is malformed", async () => {
    const path = join(dir, "malformed.pid");
    await writeFile(path, "not-a-number\n");
    expect(await readPidIfAlive(path)).toBeNull();
  });

  it("returns null for a pid that doesn't exist", async () => {
    const path = join(dir, "dead.pid");
    // pid 999999 is overwhelmingly likely to be free
    await writeFile(path, "999999\n");
    expect(await readPidIfAlive(path)).toBeNull();
  });

  it("returns the pid for a live process", async () => {
    const path = join(dir, "live.pid");
    await writeFile(path, `${process.pid}\n`);
    expect(await readPidIfAlive(path)).toBe(process.pid);
  });

  it("tolerates surrounding whitespace", async () => {
    const path = join(dir, "ws.pid");
    await writeFile(path, `  ${process.pid}  \n\n`);
    expect(await readPidIfAlive(path)).toBe(process.pid);
  });
});

describe("createDaemonCLI", () => {
  let dir: string;
  beforeEach(async () => {
    dir = await tmp();
  });
  afterEach(async () => {
    await rm(dir, { recursive: true, force: true });
  });

  it("registers the v1 commands (launch, stop, status, doctor, tail)", () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
    });
    const names = cli.commands.map((c) => c.name()).sort();
    expect(names).toEqual(["doctor", "launch", "status", "stop", "tail"]);
  });

  it("exposes launchOptions as extra `launch` flags", () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
      launchOptions: [
        { flags: "--server <url>", description: "Server URL override" },
      ],
    });
    const launch = cli.commands.find((c) => c.name() === "launch")!;
    const flags = launch.options.map((o) => o.flags);
    expect(flags).toContain("--server <url>");
    expect(flags).toContain("--detach");
  });

  it("allows caller to add custom subcommands", () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
    });
    cli.command("grant <did>").description("Custom").action(() => {});
    const names = cli.commands.map((c) => c.name());
    expect(names).toContain("grant");
  });
});

describe("createDaemonCLI: doctor", () => {
  let dir: string;
  let exitSpy: ReturnType<typeof vi.spyOn>;
  let logSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(async () => {
    dir = await tmp();
    // commander's .parseAsync calls process.exit(1) when actions throw
    // and our doctor action exits(1) on failure; stub it so the test
    // doesn't kill the runner.
    exitSpy = vi
      .spyOn(process, "exit")
      .mockImplementation((() => undefined) as never);
    logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
  });
  afterEach(async () => {
    exitSpy.mockRestore();
    logSpy.mockRestore();
    await rm(dir, { recursive: true, force: true });
  });

  function output(): string {
    return logSpy.mock.calls.map((c) => c.join(" ")).join("\n");
  }

  it("flags missing identity and missing delegation as problems", async () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
    });
    await cli.parseAsync(["node", "testbot", "doctor"]);
    const out = output();
    expect(out).toContain("✗ agent identity");
    expect(out).toContain("✗ delegation");
    expect(out).toContain("problem(s)");
    expect(exitSpy).toHaveBeenCalledWith(1);
  });

  it("reports identity ok when agent.key is present + valid", async () => {
    const paths = pathsIn(dir);
    const k = await generateDidKey();
    const seed = await k.exportSeed();
    await mkdir(paths.dir, { recursive: true });
    await writeFile(paths.agentKey, seed);

    const cli = createDaemonCLI({
      name: "testbot",
      paths,
      runDaemon: async () => ({ stop: async () => {} }),
    });
    await cli.parseAsync(["node", "testbot", "doctor"]);
    const out = output();
    expect(out).toContain("✓ agent identity");
    expect(out).toContain(k.did);
    // delegation is still missing, so still exits 1
    expect(exitSpy).toHaveBeenCalledWith(1);
  });

  it("includes caller doctor checks after built-ins, in order", async () => {
    const calls: string[] = [];
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
      doctorChecks: [
        {
          name: "custom-a",
          run: async () => {
            calls.push("a");
            return { ok: true, detail: "fine" };
          },
        },
        {
          name: "custom-b",
          run: async () => {
            calls.push("b");
            return { ok: "warn", reason: "soft warning" };
          },
        },
      ],
    });
    await cli.parseAsync(["node", "testbot", "doctor"]);
    expect(calls).toEqual(["a", "b"]);
    const out = output();
    expect(out).toContain("✓ custom-a: fine");
    expect(out).toContain("⚠ custom-b: soft warning");
  });

  it("treats a caller check that throws as a failure (doesn't crash doctor)", async () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
      doctorChecks: [
        {
          name: "throws",
          run: async () => {
            throw new Error("boom");
          },
        },
      ],
    });
    await cli.parseAsync(["node", "testbot", "doctor"]);
    const out = output();
    expect(out).toContain("✗ throws: boom");
    expect(exitSpy).toHaveBeenCalledWith(1);
  });

  it("skips the server actor check when actorStatusUrl is omitted", async () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
    });
    await cli.parseAsync(["node", "testbot", "doctor"]);
    const out = output();
    expect(out).not.toContain("server actor record");
  });

  it("runs the server actor check when actorStatusUrl is provided + identity exists", async () => {
    const paths = pathsIn(dir);
    const k = await generateDidKey();
    const seed = await k.exportSeed();
    await mkdir(paths.dir, { recursive: true });
    await writeFile(paths.agentKey, seed);

    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          online: true,
          nick: "testbot",
          provenance: { verified: false, reason: "Cert has no signature" },
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );

    try {
      const cli = createDaemonCLI({
        name: "testbot",
        paths,
        runDaemon: async () => ({ stop: async () => {} }),
        actorStatusUrl: (did) => `https://example.test/actors/${did}`,
      });
      await cli.parseAsync(["node", "testbot", "doctor"]);
      const out = output();
      expect(out).toContain("server actor record");
      expect(out).toContain("online");
      expect(out).toContain("unverified");
    } finally {
      fetchSpy.mockRestore();
    }
  });
});

describe("createDaemonCLI: stop", () => {
  let dir: string;
  let logSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(async () => {
    dir = await tmp();
    logSpy = vi.spyOn(console, "log").mockImplementation(() => {});
  });
  afterEach(async () => {
    logSpy.mockRestore();
    await rm(dir, { recursive: true, force: true });
  });

  it("reports 'no daemon' when no pid file exists", async () => {
    const cli = createDaemonCLI({
      name: "testbot",
      paths: pathsIn(dir),
      runDaemon: async () => ({ stop: async () => {} }),
    });
    await cli.parseAsync(["node", "testbot", "stop"]);
    expect(logSpy.mock.calls.flat().join(" ")).toContain("No daemon is running");
  });

  it("cleans up a stale pid file (process gone)", async () => {
    const paths = pathsIn(dir);
    await mkdir(paths.dir, { recursive: true });
    // Write a pid we know is dead
    await writeFile(paths.daemonPid, "999999\n");
    const cli = createDaemonCLI({
      name: "testbot",
      paths,
      runDaemon: async () => ({ stop: async () => {} }),
    });
    await cli.parseAsync(["node", "testbot", "stop"]);
    expect(await readPidIfAlive(paths.daemonPid)).toBeNull();
    // Stale-file cleanup means the file should be gone too.
    const { stat } = await import("node:fs/promises");
    await expect(stat(paths.daemonPid)).rejects.toThrow();
  });
});
