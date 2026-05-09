// Owner-only DM gate.
//
// State persisted at ~/.freeqcc/gate.json so cooldowns + the rolling
// hourly turn cap survive daemon restarts. Refusal-cooldowns are
// per-sender and also persisted.
//
// Tunables live in DEFAULT_LIMITS; the CLI may surface them later.

export const DEFAULT_LIMITS = {
  /** Min ms between successful dispatches. */
  cooldownMs: 60_000,
  /** Max successful dispatches in any rolling 60-minute window. */
  hourlyTurnCap: 30,
  /** Min ms between refusal NOTICEs to the same sender. */
  refusalCooldownMs: 60 * 60 * 1000, // 1 hour
  /**
   * Bot↔bot cycle detection: if more than N rapid back-and-forths happen
   * with the same counterparty inside the window, force a backoff. Tuned
   * for "two agents productively troubleshooting together" while catching
   * runaway loops.
   */
  cycleWindowMs: 5 * 60 * 1000, // 5 minutes
  cycleTurnCap: 10,
  cycleBackoffMs: 10 * 60 * 1000, // 10 minutes silent after a cycle trip
};

export type GateDecision =
  | { kind: "dispatch" }
  | { kind: "refuse"; reason: string }
  | { kind: "silent" }; // refusal already sent within window, or cooldown / turn cap hit

export interface GateState {
  lastRefusalAt: Map<string, number>; // sender DID (or "unknown:" + nick) → last refusal ts (ms)
  lastDispatchAt: number; // ms
  dispatchTimestamps: number[]; // ms timestamps within the rolling hour window
  /** sender DID → recent dispatch timestamps within cycleWindowMs. */
  perPeerDispatches: Map<string, number[]>;
  /** sender DID → backoff-until timestamp (cycle trip). */
  cycleBackoffUntil: Map<string, number>;
}

export function newGateState(): GateState {
  return {
    lastRefusalAt: new Map(),
    lastDispatchAt: 0,
    dispatchTimestamps: [],
    perPeerDispatches: new Map(),
    cycleBackoffUntil: new Map(),
  };
}

interface SerializedGateState {
  lastRefusalAt: Array<[string, number]>;
  lastDispatchAt: number;
  dispatchTimestamps: number[];
  perPeerDispatches?: Array<[string, number[]]>;
  cycleBackoffUntil?: Array<[string, number]>;
}

import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { paths, ensureDir } from "./paths.js";

const GATE_FILE = join(paths.dir, "gate.json");

export async function loadGateState(): Promise<GateState> {
  let raw: string;
  try {
    raw = await readFile(GATE_FILE, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return newGateState();
    throw err;
  }
  const parsed = JSON.parse(raw) as SerializedGateState;
  return {
    lastRefusalAt: new Map(parsed.lastRefusalAt ?? []),
    lastDispatchAt: parsed.lastDispatchAt ?? 0,
    dispatchTimestamps: Array.isArray(parsed.dispatchTimestamps)
      ? parsed.dispatchTimestamps
      : [],
    perPeerDispatches: new Map(parsed.perPeerDispatches ?? []),
    cycleBackoffUntil: new Map(parsed.cycleBackoffUntil ?? []),
  };
}

export async function saveGateState(state: GateState): Promise<void> {
  await ensureDir();
  const serialized: SerializedGateState = {
    lastRefusalAt: Array.from(state.lastRefusalAt.entries()),
    lastDispatchAt: state.lastDispatchAt,
    dispatchTimestamps: state.dispatchTimestamps,
    perPeerDispatches: Array.from(state.perPeerDispatches.entries()),
    cycleBackoffUntil: Array.from(state.cycleBackoffUntil.entries()),
  };
  await writeFile(GATE_FILE, JSON.stringify(serialized) + "\n", { mode: 0o600 });
}

export interface EvaluateArgs {
  state: GateState;
  /** Sender's DID, or null if not authenticated / unknown. */
  senderDid: string | null;
  /** Sender's nick, used as a refusal-cooldown key when DID is unknown. */
  senderNick: string;
  ownerDid: string;
  /** Optional extra allowed DIDs (multi-DID allowlist). Owner is always allowed. */
  allowedDids?: string[];
  now?: number;
  cooldownMs?: number;
  hourlyTurnCap?: number;
  refusalCooldownMs?: number;
}

export function evaluate(args: EvaluateArgs): GateDecision {
  const now = args.now ?? Date.now();
  const cooldownMs = args.cooldownMs ?? DEFAULT_LIMITS.cooldownMs;
  const hourlyTurnCap = args.hourlyTurnCap ?? DEFAULT_LIMITS.hourlyTurnCap;
  const refusalCooldownMs = args.refusalCooldownMs ?? DEFAULT_LIMITS.refusalCooldownMs;
  const { state, senderDid, senderNick, ownerDid } = args;
  const allowedDids = args.allowedDids ?? [];

  const refusalKey = senderDid ?? `unknown:${senderNick.toLowerCase()}`;

  const isAllowed =
    senderDid !== null &&
    (senderDid === ownerDid || allowedDids.includes(senderDid));

  // Non-allowed — refuse once per refusalCooldownMs, silent thereafter.
  if (!isAllowed) {
    const last = state.lastRefusalAt.get(refusalKey);
    if (last !== undefined && now - last < refusalCooldownMs) {
      return { kind: "silent" };
    }
    state.lastRefusalAt.set(refusalKey, now);
    const reason = senderDid
      ? "non-owner sender"
      : "could not verify your identity";
    return { kind: "refuse", reason };
  }

  // Allowed sender — check rate limits. The cooldown only applies when
  // there's a previous dispatch to cool down from; first dispatch always
  // goes.
  if (state.lastDispatchAt > 0 && now - state.lastDispatchAt < cooldownMs) {
    return { kind: "silent" };
  }

  // Trim and check the hourly window.
  const oneHourAgo = now - 60 * 60 * 1000;
  state.dispatchTimestamps = state.dispatchTimestamps.filter((t) => t > oneHourAgo);
  if (state.dispatchTimestamps.length >= hourlyTurnCap) {
    return { kind: "silent" };
  }

  // Bot↔bot cycle detection: per-counterparty.
  if (senderDid !== null && senderDid !== ownerDid) {
    const backoffUntil = state.cycleBackoffUntil.get(senderDid) ?? 0;
    if (now < backoffUntil) {
      return { kind: "silent" };
    }
    const windowMs = DEFAULT_LIMITS.cycleWindowMs;
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    const cutoff = now - windowMs;
    const recent = (state.perPeerDispatches.get(senderDid) ?? []).filter((t) => t > cutoff);
    if (recent.length >= cap) {
      // Trip — silence this peer for cycleBackoffMs
      state.cycleBackoffUntil.set(senderDid, now + DEFAULT_LIMITS.cycleBackoffMs);
      state.perPeerDispatches.set(senderDid, []); // reset
      return { kind: "silent" };
    }
    recent.push(now);
    state.perPeerDispatches.set(senderDid, recent);
  }

  // Reserve the slot now so concurrent DMs don't race past the cap.
  state.lastDispatchAt = now;
  state.dispatchTimestamps.push(now);
  return { kind: "dispatch" };
}
