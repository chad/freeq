import { appendFile, stat, rename, unlink } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";

export interface RefusedEntry {
  ts: string; // ISO 8601
  fromNick: string;
  fromDid: string | null;
  text: string;
  reason: string;
}

// Rotate refused.log when it grows past this size. One previous file is
// kept at refused.log.1 — a noisy attacker spamming the bot with bad DIDs
// can't fill the disk, but recent context for the operator is preserved.
const ROTATE_BYTES = 1_048_576; // 1 MiB

async function maybeRotate(): Promise<void> {
  let size = 0;
  try {
    const st = await stat(paths.refusedLog);
    size = st.size;
  } catch {
    return; // file doesn't exist yet — nothing to rotate
  }
  if (size < ROTATE_BYTES) return;
  const backup = `${paths.refusedLog}.1`;
  // Best-effort: drop any older backup, then rename current → .1.
  try {
    await unlink(backup);
  } catch {
    // ignore
  }
  try {
    await rename(paths.refusedLog, backup);
  } catch {
    // ignore — if rename fails we keep appending to the current file
  }
}

export async function logRefused(entry: RefusedEntry): Promise<void> {
  await ensureDir();
  await maybeRotate();
  await appendFile(paths.refusedLog, JSON.stringify(entry) + "\n", { mode: 0o600 });
}
