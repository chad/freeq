// Phase 5+6 — long-lived process: connect, listen, gate, dispatch.
// Stub until phase 5.

export interface DaemonOptions {
  nick: string;
  serverUrl?: string;
}

export async function runDaemon(_opts: DaemonOptions): Promise<void> {
  throw new Error("not implemented (phase 5)");
}
