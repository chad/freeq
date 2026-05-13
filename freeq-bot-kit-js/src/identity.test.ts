/** Unit tests for identity.ts. */
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, readFile, writeFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadOrCreateIdentity } from "./identity.js";

describe("loadOrCreateIdentity", () => {
  let dir: string;
  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "freeq-bot-kit-identity-"));
  });
  afterEach(async () => {
    await rm(dir, { recursive: true, force: true });
  });

  it("creates a fresh did:key on first run", async () => {
    const seedPath = join(dir, "agent.key");
    const id = await loadOrCreateIdentity({ seedPath });
    expect(id.isFresh).toBe(true);
    expect(id.did).toMatch(/^did:key:z/);
    expect(id.didKey.signer).toBeTypeOf("function");

    const buf = await readFile(seedPath);
    expect(buf.length).toBe(32);
  });

  it("rederives the same DID on subsequent runs", async () => {
    const seedPath = join(dir, "agent.key");
    const first = await loadOrCreateIdentity({ seedPath });
    const second = await loadOrCreateIdentity({ seedPath });
    expect(second.isFresh).toBe(false);
    expect(second.did).toBe(first.did);
  });

  it("creates parent directory if missing", async () => {
    const seedPath = join(dir, "nested", "deep", "agent.key");
    const id = await loadOrCreateIdentity({ seedPath });
    expect(id.isFresh).toBe(true);
    const buf = await readFile(seedPath);
    expect(buf.length).toBe(32);
  });

  it("writes the seed file with mode 0600", async () => {
    const seedPath = join(dir, "agent.key");
    await loadOrCreateIdentity({ seedPath });
    const s = await stat(seedPath);
    if (process.platform === "linux" || process.platform === "darwin") {
      expect(s.mode & 0o777).toBe(0o600);
    }
  });

  it("rejects a malformed seed file (wrong length)", async () => {
    const seedPath = join(dir, "agent.key");
    await writeFile(seedPath, new Uint8Array(16)); // wrong size
    await expect(loadOrCreateIdentity({ seedPath })).rejects.toThrow(
      /expected 32/,
    );
  });

  it("re-derives identity from a hand-placed valid seed", async () => {
    const seedPath = join(dir, "agent.key");
    const first = await loadOrCreateIdentity({ seedPath });
    const seed = await readFile(seedPath);

    // Wipe and place the same seed: should produce the same DID.
    await rm(seedPath);
    await writeFile(seedPath, seed, { mode: 0o600 });
    const second = await loadOrCreateIdentity({ seedPath });
    expect(second.isFresh).toBe(false);
    expect(second.did).toBe(first.did);
  });
});
