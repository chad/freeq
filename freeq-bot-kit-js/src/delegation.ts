// FreeqBotDelegation/v1 cert. v1.0 ships unsigned (signature: null) because
// the owner's MSGSIG private key typically lives in their browser/iOS keystore,
// not on the bot's machine. The server's verifier (connection/provenance.rs)
// recognizes this case and stores the cert as _verified: false with reason
// "Cert has no signature; declarative only".
//
// The cert *format* matches the Rust BotDelegation struct in
// freeq-bot-id/src/main.rs and the freeq-server verifier in
// connection/provenance.rs::verify_provenance.

import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname } from "node:path";

export interface DelegationCert {
  /** Always "FreeqBotDelegation/v1". Server's verifier dispatches on this. */
  type: "FreeqBotDelegation/v1";
  /** Bot's DID — must match the SASL-authenticated session DID, server checks. */
  bot_did: string;
  /** Multibase ed25519 pubkey of the bot (the part after "did:key:"). */
  bot_public_key: string;
  /** Owner's DID, typically did:plc:… */
  creator_did: string;
  /** ISO-8601 timestamp the cert was minted. */
  created_at: string;
  /** Who can revoke this binding. Same as creator_did in v1.0. */
  revocation_authority: string;
  /**
   * Base64url ed25519 signature over the JCS-canonical form of the cert with
   * this field omitted. Null in v1.0 — server treats unsigned certs as
   * declarative metadata. A future v1.1 plugs in a signing flow.
   */
  signature: string | null;
}

export interface BuildDelegationOptions {
  /** Bot's did:key — must start with "did:key:". */
  agentDid: string;
  /** Owner's DID, typically did:plc:… */
  ownerDid: string;
}

/** Build a fresh cert. Does NOT persist. */
export function buildDelegation(opts: BuildDelegationOptions): DelegationCert {
  const { agentDid, ownerDid } = opts;
  const bot_public_key = agentDid.replace(/^did:key:/, "");
  if (bot_public_key === agentDid) {
    throw new Error(
      `agentDid does not start with did:key: — got ${agentDid}. v1.0 only supports did:key bots.`,
    );
  }
  return {
    type: "FreeqBotDelegation/v1",
    bot_did: agentDid,
    bot_public_key,
    creator_did: ownerDid,
    created_at: new Date().toISOString(),
    revocation_authority: ownerDid,
    signature: null,
  };
}

export interface LoadDelegationOptions {
  /** Absolute path to the cert file. */
  certPath: string;
}

export async function loadDelegation(
  opts: LoadDelegationOptions,
): Promise<DelegationCert | null> {
  const { certPath } = opts;
  let raw: string;
  try {
    raw = await readFile(certPath, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw err;
  }
  let parsed: DelegationCert;
  try {
    parsed = JSON.parse(raw) as DelegationCert;
  } catch {
    throw new Error(
      `${certPath} is not valid JSON. Delete it to regenerate, or fix the file.`,
    );
  }
  if (parsed.type !== "FreeqBotDelegation/v1") {
    throw new Error(
      `${certPath} has type ${parsed.type}; expected FreeqBotDelegation/v1.`,
    );
  }
  return parsed;
}

export interface LoadOrMintDelegationOptions {
  /** Absolute path to the cert file. The parent directory is created if needed. */
  certPath: string;
  agentDid: string;
  ownerDid: string;
}

/** Mint a new cert if none exists, otherwise return the existing one. */
export async function loadOrMintDelegation(
  opts: LoadOrMintDelegationOptions,
): Promise<DelegationCert> {
  const { certPath, agentDid, ownerDid } = opts;
  const existing = await loadDelegation({ certPath });
  if (existing) {
    // Sanity: an existing cert must match the current agent + owner identity.
    // Mismatch usually means the user changed handle or regenerated the seed.
    if (existing.bot_did !== agentDid) {
      throw new Error(
        `Stored delegation bot_did (${existing.bot_did}) does not match current agent (${agentDid}). Delete ${certPath} to regenerate.`,
      );
    }
    if (existing.creator_did !== ownerDid) {
      throw new Error(
        `Stored delegation creator_did (${existing.creator_did}) does not match current owner (${ownerDid}). Delete ${certPath} to regenerate.`,
      );
    }
    return existing;
  }

  const cert = buildDelegation({ agentDid, ownerDid });
  await mkdir(dirname(certPath), { recursive: true, mode: 0o700 });
  await writeFile(certPath, JSON.stringify(cert, null, 2) + "\n", { mode: 0o600 });
  return cert;
}
