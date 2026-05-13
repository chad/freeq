/** Unit tests for delegation.ts. */
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, readFile, writeFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  buildDelegation,
  loadDelegation,
  loadOrMintDelegation,
  type DelegationCert,
} from "./delegation.js";

const FAKE_AGENT_DID = "did:key:zABCDEFGHIJKLMNOPQRSTUVWXYZ";
const FAKE_OWNER_DID = "did:plc:xyzowner";

describe("buildDelegation", () => {
  it("emits a v1 cert with expected fields", () => {
    const cert = buildDelegation({ agentDid: FAKE_AGENT_DID, ownerDid: FAKE_OWNER_DID });
    expect(cert.type).toBe("FreeqBotDelegation/v1");
    expect(cert.bot_did).toBe(FAKE_AGENT_DID);
    expect(cert.bot_public_key).toBe("zABCDEFGHIJKLMNOPQRSTUVWXYZ");
    expect(cert.creator_did).toBe(FAKE_OWNER_DID);
    expect(cert.revocation_authority).toBe(FAKE_OWNER_DID);
    expect(cert.signature).toBeNull();
    expect(new Date(cert.created_at).toString()).not.toBe("Invalid Date");
  });

  it("rejects an agentDid without did:key: prefix", () => {
    expect(() =>
      buildDelegation({ agentDid: "did:plc:notakey", ownerDid: FAKE_OWNER_DID }),
    ).toThrow(/did:key/);
  });
});

describe("loadDelegation", () => {
  let dir: string;
  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "freeq-bot-kit-delegation-"));
  });
  afterEach(async () => {
    await rm(dir, { recursive: true, force: true });
  });

  it("returns null when the file is absent", async () => {
    const result = await loadDelegation({ certPath: join(dir, "nope.json") });
    expect(result).toBeNull();
  });

  it("parses a well-formed cert", async () => {
    const certPath = join(dir, "delegation.json");
    const cert: DelegationCert = {
      type: "FreeqBotDelegation/v1",
      bot_did: FAKE_AGENT_DID,
      bot_public_key: "zABCDEFGHIJKLMNOPQRSTUVWXYZ",
      creator_did: FAKE_OWNER_DID,
      created_at: "2025-01-01T00:00:00.000Z",
      revocation_authority: FAKE_OWNER_DID,
      signature: null,
    };
    await writeFile(certPath, JSON.stringify(cert));
    const loaded = await loadDelegation({ certPath });
    expect(loaded).toEqual(cert);
  });

  it("rejects malformed JSON", async () => {
    const certPath = join(dir, "delegation.json");
    await writeFile(certPath, "{not json");
    await expect(loadDelegation({ certPath })).rejects.toThrow(/not valid JSON/);
  });

  it("rejects an unknown type tag", async () => {
    const certPath = join(dir, "delegation.json");
    await writeFile(certPath, JSON.stringify({ type: "SomethingElse/v9" }));
    await expect(loadDelegation({ certPath })).rejects.toThrow(/expected FreeqBotDelegation/);
  });
});

describe("loadOrMintDelegation", () => {
  let dir: string;
  beforeEach(async () => {
    dir = await mkdtemp(join(tmpdir(), "freeq-bot-kit-delegation-"));
  });
  afterEach(async () => {
    await rm(dir, { recursive: true, force: true });
  });

  it("mints a fresh cert when none exists", async () => {
    const certPath = join(dir, "delegation.json");
    const cert = await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    expect(cert.bot_did).toBe(FAKE_AGENT_DID);
    expect(cert.creator_did).toBe(FAKE_OWNER_DID);

    // Was persisted.
    const onDisk = JSON.parse(await readFile(certPath, "utf8"));
    expect(onDisk.bot_did).toBe(FAKE_AGENT_DID);
  });

  it("returns the existing cert when one is on disk and matches", async () => {
    const certPath = join(dir, "delegation.json");
    const first = await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    const second = await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    expect(second.created_at).toBe(first.created_at); // same instance loaded back
  });

  it("throws when the existing cert names a different agentDid", async () => {
    const certPath = join(dir, "delegation.json");
    await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    await expect(
      loadOrMintDelegation({
        certPath,
        agentDid: "did:key:zDIFFERENT",
        ownerDid: FAKE_OWNER_DID,
      }),
    ).rejects.toThrow(/bot_did/);
  });

  it("throws when the existing cert names a different ownerDid", async () => {
    const certPath = join(dir, "delegation.json");
    await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    await expect(
      loadOrMintDelegation({
        certPath,
        agentDid: FAKE_AGENT_DID,
        ownerDid: "did:plc:somebody-else",
      }),
    ).rejects.toThrow(/creator_did/);
  });

  it("creates parent directories if needed", async () => {
    const certPath = join(dir, "nested", "deep", "delegation.json");
    await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    const s = await stat(certPath);
    expect(s.isFile()).toBe(true);
  });

  it("writes the cert with mode 0600", async () => {
    const certPath = join(dir, "delegation.json");
    await loadOrMintDelegation({
      certPath,
      agentDid: FAKE_AGENT_DID,
      ownerDid: FAKE_OWNER_DID,
    });
    const s = await stat(certPath);
    if (process.platform === "linux" || process.platform === "darwin") {
      expect(s.mode & 0o777).toBe(0o600);
    }
  });
});
