// FreeqBotDelegation/v1 cert. v1.0 ships unsigned (signature: null) because
// the owner's MSGSIG private key lives in their browser/iOS keystore, not on
// the daemon's machine. The server's verifier (connection/provenance.rs)
// recognizes this case and stores the cert as _verified: false with reason
// "Cert has no signature; declarative only" — exactly what we want.
//
// The cert *format* matches the Rust BotDelegation struct in
// freeq-bot-id/src/main.rs:69-87 and the freeq-server verifier
// connection/provenance.rs::verify_provenance. v1.1 adds an optional
// signature field; nothing else in this file changes.
import { readFile, writeFile } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";
import type { AgentIdentity } from "./identity.js";
import type { Owner } from "./owner.js";

export interface DelegationCert {
  /** Always "FreeqBotDelegation/v1". Server's verifier dispatches on this. */
  type: "FreeqBotDelegation/v1";
  /** Agent's DID — must match the SASL-authenticated session DID, server checks. */
  bot_did: string;
  /** Multibase ed25519 pubkey of the agent (the part after "did:key:"). */
  bot_public_key: string;
  /** Owner's DID, typically did:plc:… */
  creator_did: string;
  /** ISO-8601 timestamp the cert was minted. */
  created_at: string;
  /** Who can revoke this binding. Same as creator_did in v1.0. */
  revocation_authority: string;
  /**
   * Base64url ed25519 signature over the JCS-canonical form of the cert
   * with this field omitted. Null in v1.0 — server treats unsigned certs as
   * declarative metadata. v1.1 plugs in a signing flow.
   */
  signature: string | null;
}

export async function loadDelegation(): Promise<DelegationCert | null> {
  let raw: string;
  try {
    raw = await readFile(paths.delegation, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw err;
  }
  let parsed: DelegationCert;
  try {
    parsed = JSON.parse(raw) as DelegationCert;
  } catch {
    throw new Error(
      `${paths.delegation} is not valid JSON. Delete it to regenerate, or fix the file.`,
    );
  }
  if (parsed.type !== "FreeqBotDelegation/v1") {
    throw new Error(
      `${paths.delegation} has type ${parsed.type}; expected FreeqBotDelegation/v1.`,
    );
  }
  return parsed;
}

/** Build a fresh cert from agent + owner state. Does NOT persist. */
export function buildDelegation(args: {
  agent: AgentIdentity;
  owner: Owner;
}): DelegationCert {
  const { agent, owner } = args;
  const bot_public_key = agent.did.replace(/^did:key:/, "");
  if (bot_public_key === agent.did) {
    throw new Error(
      `Agent DID does not start with did:key: — got ${agent.did}. v1.0 only supports did:key agents.`,
    );
  }
  return {
    type: "FreeqBotDelegation/v1",
    bot_did: agent.did,
    bot_public_key,
    creator_did: owner.did,
    created_at: new Date().toISOString(),
    revocation_authority: owner.did,
    signature: null, // v1.0: unsigned
  };
}

/** Mint a new cert if none exists, otherwise return the existing one. */
export async function loadOrMintDelegation(args: {
  agent: AgentIdentity;
  owner: Owner;
}): Promise<DelegationCert> {
  const existing = await loadDelegation();
  if (existing) {
    // Sanity: existing cert must match current agent + owner identity.
    // Mismatch usually means the user changed handle or regenerated agent.key.
    if (existing.bot_did !== args.agent.did) {
      throw new Error(
        `Stored delegation bot_did (${existing.bot_did}) does not match current agent (${args.agent.did}). Delete ${paths.delegation} to regenerate.`,
      );
    }
    if (existing.creator_did !== args.owner.did) {
      throw new Error(
        `Stored delegation creator_did (${existing.creator_did}) does not match current owner (${args.owner.did}). Delete ${paths.delegation} to regenerate.`,
      );
    }
    return existing;
  }

  const cert = buildDelegation(args);
  await ensureDir();
  await writeFile(paths.delegation, JSON.stringify(cert, null, 2) + "\n", { mode: 0o600 });
  return cert;
}
