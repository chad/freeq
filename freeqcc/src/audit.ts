import { appendFile } from "node:fs/promises";
import { paths, ensureDir } from "./paths.js";

export interface RefusedEntry {
  ts: string; // ISO 8601
  fromNick: string;
  fromDid: string | null;
  text: string;
  reason: string;
}

export async function logRefused(entry: RefusedEntry): Promise<void> {
  await ensureDir();
  await appendFile(paths.refusedLog, JSON.stringify(entry) + "\n", { mode: 0o600 });
}
