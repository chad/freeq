// Optional multi-DID allowlist with per-DID capability scopes.
//
// Format: ~/.freeqcc/allowlist.json
//   {
//     "allowed": [
//       { "did": "did:plc:...", "label": "alice", "actions": ["join", "privmsg"] },
//       { "did": "did:key:...", "label": "peer agent" }   // no actions = chat only
//     ]
//   }
//
// Owner is ALWAYS allowed and gets the full action set (OWNER_ACTIONS below).
// A non-owner with no allowlist entry can't dispatch the bot at all. A non-
// owner with an entry but no `actions` can chat with the bot but cannot drive
// IRC actions. A non-owner with `actions: ["join"]` can ask the bot to join
// channels but nothing else.
import { readFile, writeFile, rename, unlink } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";

/** Default action set granted to the owner. Excludes:
 *   - "nick"  : changing the bot's IRC nick is too easy to weaponize via
 *               prompt-injection; owner can grant it explicitly if needed.
 *   - bare "privmsg" / "notice" : intentionally split into -user / -channel
 *               so an injected dispatch can't broadcast to arbitrary channels.
 *               Default grants user-targeted speech only. */
export const OWNER_ACTIONS: readonly string[] = [
  "join",
  "part",
  "privmsg-user",
  "notice-user",
];

/** Every action the control socket knows about. Used to validate that a
 *  granted action name in allowlist.json is real (so a typo doesn't get
 *  silently ignored). */
export const ALL_ACTIONS: readonly string[] = [
  "join",
  "part",
  "privmsg-user",
  "privmsg-channel",
  "notice-user",
  "notice-channel",
  "nick",
];

export interface AllowlistEntry {
  did: string;
  label?: string;
  /** Action names this DID is allowed to invoke via the control socket.
   *  Empty / undefined = chat-only (no IRC actions). */
  actions?: string[];
}

interface AllowlistFile {
  allowed?: AllowlistEntry[];
}

/** Map legacy action names to the new -user/-channel scoped form. Existing
 *  allowlist files written before the H-2 split can name "privmsg" /
 *  "notice"; loosen them to the SAFER -user variant only (broadcast to
 *  channels needs an explicit re-grant). */
export function migrateAction(a: string): string[] {
  if (a === "privmsg") return ["privmsg-user"];
  if (a === "notice") return ["notice-user"];
  return [a];
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
    return (parsed.allowed ?? [])
      .filter((e): e is AllowlistEntry => typeof e.did === "string" && e.did.length > 0)
      .map((e) => ({
        did: e.did,
        label: typeof e.label === "string" ? e.label : undefined,
        actions: Array.isArray(e.actions)
          ? e.actions
              .filter((a) => typeof a === "string")
              .flatMap(migrateAction)
          : [],
      }));
  } catch {
    return [];
  }
}

export async function saveAllowlist(entries: AllowlistEntry[]): Promise<void> {
  await ensureDir();
  const data: AllowlistFile = { allowed: entries };
  // Atomic write: serialize to a tmp file alongside the target, then rename.
  // A crash mid-write previously could leave allowlist.json half-truncated,
  // which the daemon's mtime-poll loop would re-read and parse-fail (silently
  // dropping all grants until the file was hand-fixed).
  const tmp = `${paths.allowlist}.${process.pid}.tmp`;
  try {
    await writeFile(tmp, JSON.stringify(data, null, 2) + "\n", { mode: 0o600 });
    await rename(tmp, paths.allowlist);
  } catch (err) {
    // Best-effort cleanup of the tmp file before bubbling the error.
    try {
      await unlink(tmp);
    } catch {
      // ignore
    }
    throw err;
  }
}

/** True if this DID is the owner OR appears in the allowlist. */
export function isAllowed(senderDid: string, ownerDid: string, allowlist: AllowlistEntry[]): boolean {
  if (senderDid === ownerDid) return true;
  return allowlist.some((e) => e.did === senderDid);
}

/** Action set for a sender. Owner: all. Allowlisted: their entry's `actions`.
 *  Anyone else: empty (the gate refuses them anyway, but useful for symmetry). */
export function actionsFor(
  senderDid: string,
  ownerDid: string,
  allowlist: AllowlistEntry[],
): string[] {
  if (senderDid === ownerDid) return [...OWNER_ACTIONS];
  const entry = allowlist.find((e) => e.did === senderDid);
  return entry?.actions ?? [];
}
