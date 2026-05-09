import { homedir } from "node:os";
import { join } from "node:path";
import { mkdir } from "node:fs/promises";

export const FREEQCC_DIR = join(homedir(), ".freeqcc");

export const paths = {
  dir: FREEQCC_DIR,
  agentKey: join(FREEQCC_DIR, "agent.key"),
  owner: join(FREEQCC_DIR, "owner.json"),
  delegation: join(FREEQCC_DIR, "delegation.json"),
  session: join(FREEQCC_DIR, "session.json"),
  daemonPid: join(FREEQCC_DIR, "daemon.pid"),
  daemonLog: join(FREEQCC_DIR, "daemon.log"),
  refusedLog: join(FREEQCC_DIR, "refused.log"),
  config: join(FREEQCC_DIR, "config.json"),
  allowlist: join(FREEQCC_DIR, "allowlist.json"),
  telemetry: join(FREEQCC_DIR, "telemetry.json"),
  gate: join(FREEQCC_DIR, "gate.json"),
} as const;

export async function ensureDir(): Promise<void> {
  await mkdir(FREEQCC_DIR, { recursive: true, mode: 0o700 });
}
