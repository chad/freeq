// Owner config (Bluesky handle + AT Protocol DID) persisted at ~/.freeqcc/owner.json.
//
// First launch: prompt for handle, resolve via Bluesky's public API
// (fetchProfile, re-exported by @freeq/bot-kit, handles both handle and
// DID inputs), save {handle, did}. Subsequent launches: load and return.
//
// Failure modes:
// - Handle doesn't resolve → throw with the original input echoed
// - File exists but malformed → throw, don't silently regenerate
import { fetchProfile } from "@freeq/bot-kit";
import { readFile, writeFile } from "node:fs/promises";
import prompts from "prompts";
import { paths, ensureDir } from "./paths.js";

export interface Owner {
  /** Canonical AT Protocol handle, lowercase, no leading @ (e.g. "chadfowler.com"). */
  handle: string;
  /** Resolved DID, typically did:plc:… */
  did: string;
}

export async function loadOwner(): Promise<Owner | null> {
  let raw: string;
  try {
    raw = await readFile(paths.owner, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw err;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    throw new Error(
      `${paths.owner} is not valid JSON. Delete it and re-run \`freeqcc launch\` to re-prompt.`,
    );
  }
  const o = parsed as { handle?: unknown; did?: unknown };
  if (typeof o.handle !== "string" || typeof o.did !== "string") {
    throw new Error(
      `${paths.owner} is missing 'handle' or 'did'. Delete it and re-run \`freeqcc launch\`.`,
    );
  }
  return { handle: o.handle, did: o.did };
}

export async function resolveHandle(input: string): Promise<Owner> {
  const trimmed = input.trim().replace(/^@/, "").toLowerCase();
  if (!trimmed) {
    throw new Error("Handle is empty.");
  }
  const profile = await fetchProfile(trimmed);
  if (!profile) {
    throw new Error(
      `Couldn't resolve "${trimmed}" via Bluesky's public API. ` +
        `Check the handle (e.g. "chadfowler.com" or "yourname.bsky.social"), ` +
        `or pass a DID directly (did:plc:…).`,
    );
  }
  return { handle: profile.handle, did: profile.did };
}

export async function promptAndStoreOwner(): Promise<Owner> {
  const response = await prompts(
    {
      type: "text",
      name: "handle",
      message: "Your AT Protocol handle (e.g. chadfowler.com)",
      validate: (v: string) => (v.trim().length > 0 ? true : "Required"),
    },
    {
      onCancel: () => {
        throw new Error("Cancelled.");
      },
    },
  );

  const owner = await resolveHandle(response.handle);
  await ensureDir();
  await writeFile(paths.owner, JSON.stringify(owner, null, 2) + "\n", { mode: 0o600 });
  return owner;
}

export async function loadOrPromptOwner(): Promise<Owner> {
  const existing = await loadOwner();
  if (existing) return existing;
  return await promptAndStoreOwner();
}
