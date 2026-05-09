import { homedir } from "node:os";
import { join } from "node:path";
import { mkdir, chmod } from "node:fs/promises";

export const FREEQCC_DIR = join(homedir(), ".freeqcc");

export const paths = {
  dir: FREEQCC_DIR,
  agentKey: join(FREEQCC_DIR, "agent.key"),
  owner: join(FREEQCC_DIR, "owner.json"),
  delegation: join(FREEQCC_DIR, "delegation.json"),
  daemonPid: join(FREEQCC_DIR, "daemon.pid"),
  daemonLog: join(FREEQCC_DIR, "daemon.log"),
  refusedLog: join(FREEQCC_DIR, "refused.log"),
  config: join(FREEQCC_DIR, "config.json"),
  allowlist: join(FREEQCC_DIR, "allowlist.json"),
  telemetry: join(FREEQCC_DIR, "telemetry.json"),
  gate: join(FREEQCC_DIR, "gate.json"),
  controlSock: join(FREEQCC_DIR, "control.sock"),
  /** Per-DID claude session id files live under sessions/. Owner uses
   *  sessions/__owner__.json; other DIDs use sessions/<sha256(did)>.json so
   *  the file name doesn't disclose the DID directly. */
  sessionsDir: join(FREEQCC_DIR, "sessions"),
} as const;

export async function ensureDir(): Promise<void> {
  await mkdir(FREEQCC_DIR, { recursive: true, mode: 0o700 });
  // mkdir's `mode` is only honored when the directory is created; if it
  // already exists from an earlier release that used a looser umask, force
  // the perms back to owner-only. Best-effort — if chmod fails (e.g. dir is
  // on a filesystem that ignores POSIX modes) we don't crash the daemon.
  try {
    await chmod(FREEQCC_DIR, 0o700);
  } catch {
    // ignore
  }
}
