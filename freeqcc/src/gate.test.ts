/** Tests for the owner-only DM gate.
 *
 * The gate is security-critical: it decides whether to dispatch a DM to
 * Claude Code on this machine. A regression that lets non-owner DMs through
 * is a privilege-escalation bug. Cover the core decision paths. */

import { describe, it, expect, beforeEach } from "vitest";
import { evaluate, newGateState, DEFAULT_LIMITS, type GateState } from "./gate.js";

const OWNER = "did:plc:owner";
const STRANGER = "did:plc:stranger";
const ALLOWED = "did:plc:friend";

describe("gate.evaluate — owner-only behavior", () => {
  let state: GateState;
  beforeEach(() => { state = newGateState(); });

  it("dispatches owner DMs", () => {
    const d = evaluate({ state, senderDid: OWNER, senderNick: "owner", ownerDid: OWNER, now: 1_000 });
    expect(d.kind).toBe("dispatch");
  });

  it("refuses non-owner DMs (first time, with reason)", () => {
    const d = evaluate({ state, senderDid: STRANGER, senderNick: "stranger", ownerDid: OWNER, now: 1_000 });
    expect(d.kind).toBe("refuse");
    expect(d).toMatchObject({ kind: "refuse", reason: expect.stringContaining("non-owner") });
  });

  it("goes silent after refusing the same non-owner within the cooldown", () => {
    evaluate({ state, senderDid: STRANGER, senderNick: "stranger", ownerDid: OWNER, now: 1_000 });
    const second = evaluate({ state, senderDid: STRANGER, senderNick: "stranger", ownerDid: OWNER, now: 2_000 });
    expect(second.kind).toBe("silent");
  });

  it("refuses again after the refusal cooldown expires", () => {
    evaluate({ state, senderDid: STRANGER, senderNick: "stranger", ownerDid: OWNER, now: 1_000 });
    const past = 1_000 + DEFAULT_LIMITS.refusalCooldownMs + 1;
    const again = evaluate({ state, senderDid: STRANGER, senderNick: "stranger", ownerDid: OWNER, now: past });
    expect(again.kind).toBe("refuse");
  });

  it("refuses null-DID senders with an identity-verification reason", () => {
    const d = evaluate({ state, senderDid: null, senderNick: "anon", ownerDid: OWNER, now: 1_000 });
    expect(d).toMatchObject({ kind: "refuse", reason: expect.stringContaining("verify") });
  });

  it("uses nick-based refusal-cooldown key when DID is null (so different anon nicks don't collide)", () => {
    evaluate({ state, senderDid: null, senderNick: "anon1", ownerDid: OWNER, now: 1_000 });
    const d2 = evaluate({ state, senderDid: null, senderNick: "anon2", ownerDid: OWNER, now: 2_000 });
    // Different nick → fresh refusal, not silent
    expect(d2.kind).toBe("refuse");
  });
});

describe("gate.evaluate — allowlisted DIDs", () => {
  let state: GateState;
  beforeEach(() => { state = newGateState(); });

  it("dispatches DMs from a DID in allowedDids", () => {
    const d = evaluate({
      state, senderDid: ALLOWED, senderNick: "friend",
      ownerDid: OWNER, allowedDids: [ALLOWED], now: 1_000,
    });
    expect(d.kind).toBe("dispatch");
  });

  it("refuses DIDs not on the allowlist", () => {
    const d = evaluate({
      state, senderDid: STRANGER, senderNick: "stranger",
      ownerDid: OWNER, allowedDids: [ALLOWED], now: 1_000,
    });
    expect(d.kind).toBe("refuse");
  });
});

describe("gate.evaluate — bot-bot cycle detection", () => {
  let state: GateState;
  beforeEach(() => { state = newGateState(); });

  // Cycle detection only applies to non-owner allowlisted peers.
  // Owner is never subject to it.
  const PEER = ALLOWED;
  const opts = (now: number) => ({
    state, senderDid: PEER, senderNick: "peer",
    ownerDid: OWNER, allowedDids: [PEER], now,
  });

  it("dispatches up to cycleTurnCap rapid messages from same peer", () => {
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    for (let i = 0; i < cap; i++) {
      const d = evaluate(opts(1_000 + i * 100));
      expect(d.kind).toBe("dispatch");
    }
  });

  it("trips backoff (silent) when cycleTurnCap is exceeded inside the window", () => {
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    for (let i = 0; i < cap; i++) {
      evaluate(opts(1_000 + i * 100));
    }
    // (cap+1)th call within the same window should trip cycle backoff
    const tripped = evaluate(opts(1_000 + cap * 100));
    expect(tripped.kind).toBe("silent");
    // Backoff window registered
    expect(state.cycleBackoffUntil.get(PEER)).toBeDefined();
  });

  it("stays silent during the cycle backoff window", () => {
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    for (let i = 0; i < cap + 1; i++) {
      evaluate(opts(1_000 + i * 100));
    }
    // Try again partway through the backoff
    const stillBackoff = evaluate(opts(1_000 + cap * 100 + DEFAULT_LIMITS.cycleBackoffMs / 2));
    expect(stillBackoff.kind).toBe("silent");
  });

  it("resumes dispatching after backoff expires", () => {
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    for (let i = 0; i < cap + 1; i++) {
      evaluate(opts(1_000 + i * 100));
    }
    const after = 1_000 + cap * 100 + DEFAULT_LIMITS.cycleBackoffMs + 1;
    const resumed = evaluate(opts(after));
    expect(resumed.kind).toBe("dispatch");
  });

  it("does NOT subject the owner to cycle detection", () => {
    // Owner dispatching very rapidly — should never trip cycle backoff.
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    // Use a higher hourly cap so the hourly limit doesn't intercept.
    for (let i = 0; i < cap + 5; i++) {
      const d = evaluate({
        state, senderDid: OWNER, senderNick: "owner",
        ownerDid: OWNER, hourlyTurnCap: 1000, now: 1_000 + i * 100,
      });
      expect(d.kind).toBe("dispatch");
    }
    // perPeerDispatches must NOT have been populated for the owner
    expect(state.perPeerDispatches.has(OWNER)).toBe(false);
  });

  it("tracks cycle state per-peer (one peer tripping doesn't silence others)", () => {
    const OTHER_PEER = "did:plc:other-peer";
    const cap = DEFAULT_LIMITS.cycleTurnCap;
    // Trip PEER's cycle
    for (let i = 0; i < cap + 1; i++) {
      evaluate({
        state, senderDid: PEER, senderNick: "peer",
        ownerDid: OWNER, allowedDids: [PEER, OTHER_PEER], now: 1_000 + i * 100,
      });
    }
    // OTHER_PEER should still dispatch normally
    const other = evaluate({
      state, senderDid: OTHER_PEER, senderNick: "other",
      ownerDid: OWNER, allowedDids: [PEER, OTHER_PEER], now: 1_000 + cap * 100,
    });
    expect(other.kind).toBe("dispatch");
  });
});

describe("gate.evaluate — hourly turn cap", () => {
  let state: GateState;
  beforeEach(() => { state = newGateState(); });

  it("dispatches up to the cap, then goes silent", () => {
    const cap = 3;
    for (let i = 0; i < cap; i++) {
      const d = evaluate({
        state, senderDid: OWNER, senderNick: "owner",
        ownerDid: OWNER, hourlyTurnCap: cap, now: 1_000 + i * 1_000,
      });
      expect(d.kind).toBe("dispatch");
    }
    const over = evaluate({
      state, senderDid: OWNER, senderNick: "owner",
      ownerDid: OWNER, hourlyTurnCap: cap, now: 1_000 + cap * 1_000,
    });
    expect(over.kind).toBe("silent");
  });

  it("trims old dispatches outside the hour window so the cap resets over time", () => {
    const cap = 2;
    // Two old dispatches outside the window
    state.dispatchTimestamps = [1_000, 2_000];
    const oneHourPlus = 1_000 + 60 * 60 * 1000 + 1;
    const d = evaluate({
      state, senderDid: OWNER, senderNick: "owner",
      ownerDid: OWNER, hourlyTurnCap: cap, now: oneHourPlus,
    });
    expect(d.kind).toBe("dispatch");
  });
});
