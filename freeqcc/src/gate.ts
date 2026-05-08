// Phase 6 — owner-only gate, refusal rate limit, cooldown, turn cap.
// Stub until phase 6.

export type GateDecision =
  | { kind: "dispatch" }
  | { kind: "refuse"; reason: string }
  | { kind: "silent" }; // refusal already sent within the hour, or cooldown / turn cap hit

export interface GateState {
  // per-sender DID → ISO ts of last refusal sent
  lastRefusalAt: Map<string, number>;
  // ms timestamp of last successful dispatch
  lastDispatchAt: number;
  // dispatches in the current hour window (ms timestamps)
  dispatchTimestamps: number[];
}

export function newGateState(): GateState {
  return {
    lastRefusalAt: new Map(),
    lastDispatchAt: 0,
    dispatchTimestamps: [],
  };
}

export function evaluate(_args: {
  state: GateState;
  senderDid: string | null;
  ownerDid: string;
  now?: number;
  cooldownMs?: number;
  hourlyTurnCap?: number;
  refusalCooldownMs?: number;
}): GateDecision {
  throw new Error("not implemented (phase 6)");
}
