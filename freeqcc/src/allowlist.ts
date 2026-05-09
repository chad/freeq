// Optional multi-DID allowlist. The owner DID is ALWAYS allowed implicitly;
// this file extends the gate to accept additional DIDs (e.g. a friend, a
// peer agent for bot↔bot, etc.).
//
// Format: ~/.freeqcc/allowlist.json
//   {
//     "allowed": [
//       { "did": "did:plc:...", "label": "alice (collaborator)" },
//       { "did": "did:key:...", "label": "peer agent" }
//     ]
//   }
//
// Missing or unreadable file → no extra allowlisted DIDs (owner-only mode).
import { readFile } from "node:fs/promises";
import { paths } from "./paths.js";

export interface AllowlistEntry {
  did: string;
  label?: string;
}

interface AllowlistFile {
  allowed?: AllowlistEntry[];
}

export async function loadAllowlist(): Promise<AllowlistEntry[]> {
  let raw: string;
  try {
    raw = await readFile(paths.allowlist, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return [];
    return [];
  }
  try {
    const parsed = JSON.parse(raw) as AllowlistFile;
    return (parsed.allowed ?? []).filter((e) => typeof e.did === "string" && e.did.length > 0);
  } catch {
    return [];
  }
}

/** True if this DID is the owner OR appears in the allowlist. */
export function isAllowed(senderDid: string, ownerDid: string, allowlist: AllowlistEntry[]): boolean {
  if (senderDid === ownerDid) return true;
  return allowlist.some((e) => e.did === senderDid);
}
