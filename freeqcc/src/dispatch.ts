// Phase 6 — claude -p subprocess dispatch with persistent --resume session.
// Stub until phase 6.

export interface DispatchResult {
  reply: string;
  sessionId: string;
  durationMs: number;
}

export async function dispatchToClaude(_args: {
  text: string;
  sessionId: string | null; // null on first dispatch
}): Promise<DispatchResult> {
  throw new Error("not implemented (phase 6)");
}
