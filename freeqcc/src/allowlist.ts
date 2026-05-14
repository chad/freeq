// Optional multi-DID allowlist with per-DID capability scopes.
//
// File format at ~/.freeqcc/allowlist.json:
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
//
// Storage / live-reload is delegated to @freeq/bot-kit's createDidMap.
// This module owns the freeqcc-specific bits: the {allowed:[…]} wrapping,
// the legacy-action migration on parse, the atomic-write semantics on save,
// and the per-action authorization logic (isAllowed/actionsFor) used at
// dispatch time.
import { createDidMap, type DidMapMutable } from "@freeq/bot-kit";
import writeFileAtomic from "write-file-atomic";

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

function parseAllowlistJson(raw: string): AllowlistEntry[] {
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
}

async function saveAllowlistJson(path: string, entries: AllowlistEntry[]): Promise<void> {
  // Atomic write: writeFileAtomic uses tmp+fsync+rename under the hood, so
  // the file on disk is never half-truncated. A crash mid-write previously
  // could leave allowlist.json partial, which the daemon's mtime-poll loop
  // would re-read and parse-fail (silently dropping all grants until the
  // file was hand-fixed). createDidMap retains previous state on parse
  // error now, but the atomic write still prevents an operator-visible
  // "no grants?!" window.
  const data: AllowlistFile = { allowed: entries };
  await writeFileAtomic(path, JSON.stringify(data, null, 2) + "\n", { mode: 0o600 });
}

/** Build a hot-reloadable, atomically-persisted access map for the freeqcc
 *  allowlist file. Wraps bot-kit's createDidMap with this module's specific
 *  JSON shape and migration rules. */
export async function createAccessMap(
  path: string,
): Promise<DidMapMutable<AllowlistEntry>> {
  return createDidMap<AllowlistEntry>({
    load: { path, parse: parseAllowlistJson },
    save: (entries) => saveAllowlistJson(path, entries),
  });
}

/** True if this DID is the owner OR appears in the allowlist. */
export function isAllowed(
  senderDid: string,
  ownerDid: string,
  allowlist: AllowlistEntry[],
): boolean {
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
