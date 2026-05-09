// Agent did:key identity, persisted across runs.
//
// Wraps the SDK's `generateDidKey` / `importDidKey` with a 32-byte ed25519
// seed file at ~/.freeqcc/agent.key (mode 0600). First run generates and
// writes; subsequent runs load and re-derive the same did:key.
import { generateDidKey, importDidKey, type DidKey } from "@freeq/sdk";
import { chmod, readFile, writeFile } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";

export interface AgentIdentity {
  /** `did:key:z…` — agent's cryptographic principal. */
  did: string;
  /** SDK key object for SASL signing. */
  didKey: DidKey;
  /** True when this run generated the key (first launch). */
  isFresh: boolean;
}

export async function loadOrCreateIdentity(): Promise<AgentIdentity> {
  await ensureDir();

  const seed = await readSeedIfPresent();
  if (seed) {
    const didKey = await importDidKey(seed);
    return { did: didKey.did, didKey, isFresh: false };
  }

  const didKey = await generateDidKey();
  const newSeed = await didKey.exportSeed();
  await writeFile(paths.agentKey, newSeed, { mode: 0o600 });
  // Belt and suspenders: enforce 0600 even if file existed with looser mode.
  await chmod(paths.agentKey, 0o600);
  return { did: didKey.did, didKey, isFresh: true };
}

async function readSeedIfPresent(): Promise<Uint8Array | null> {
  let buf: Buffer;
  try {
    buf = await readFile(paths.agentKey);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw err;
  }
  if (buf.length !== 32) {
    throw new Error(
      `${paths.agentKey} is ${buf.length} bytes, expected 32 ` +
        `(ed25519 seed). Delete it to regenerate the agent key, or restore the original.`,
    );
  }
  return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
}
