// Per-bot on-disk directory: ~/.freeq/bots/<name>/
//
// Internal to bot-kit — not exported. FreeqBot uses this to scope a bot's
// state (seed file, delegation cert) by name.

import { homedir } from "node:os";
import { join } from "node:path";
import { mkdir, chmod } from "node:fs/promises";

/** Default root: `~/.freeq/bots/`. Override via `botDir({root})` for tests. */
export const FREEQ_BOTS_ROOT = join(homedir(), ".freeq", "bots");

export interface BotDirOptions {
  /** Override the parent directory. Defaults to `FREEQ_BOTS_ROOT`. */
  root?: string;
}

/** Return `<root>/<name>/`, creating it with mode 0700. */
export async function botDir(name: string, opts: BotDirOptions = {}): Promise<string> {
  if (!name || /[/\\]/.test(name)) {
    throw new Error(`bot name must be non-empty and contain no path separators (got ${JSON.stringify(name)})`);
  }
  const root = opts.root ?? FREEQ_BOTS_ROOT;
  const dir = join(root, name);
  await mkdir(dir, { recursive: true, mode: 0o700 });
  // mkdir's `mode` is only honored when the directory is created; if it already
  // existed under a looser umask, force it back to owner-only. Best-effort.
  try {
    await chmod(dir, 0o700);
  } catch {
    // ignore — filesystems that ignore POSIX modes (e.g. some FAT mounts)
  }
  return dir;
}
