// Tests for createTurnGate. Time is injected via `now` on each evaluate
// call, so we don't need fake timers — each test just walks the clock
// forward by passing different `now` values.
//
// Coverage:
//   - refusal-once-then-silent (with caller-provided refusalReason)
//   - dispatch-to-dispatch cooldown
//   - rolling hourly cap
//   - per-peer cycle detection (rate + backoff window)
//   - skipCycleDetection bypass
//   - keyed by senderDid (falls back to "unknown:<nick>" for null)
//   - persistence: load restores state, persist invokes save with
//     JSON-serializable snapshot

import { describe, it, expect, vi } from "vitest";
import { createTurnGate, type TurnGateState } from "./turn-gate.js";

// Tight defaults shorten test setup
function defaults() {
  return {
    cooldownMs: 0,
    hourlyCap: 30,
    refusalCooldownMs: 3600_000,
    cyclePolicy: {
      windowMs: 5 * 60_000,
      turnCap: 10,
      backoffMs: 10 * 60_000,
    },
  };
}

const t0 = 1_700_000_000_000; // arbitrary epoch ms for deterministic tests

describe("createTurnGate: refusal-once-then-silent", () => {
  it("first refusal returns refuse with the caller's reason", async () => {
    const gate = await createTurnGate(defaults());
    const d = gate.evaluate({
      senderDid: "did:plc:rando",
      senderNick: "rando",
      refusalReason: "not on allowlist",
      now: t0,
    });
    expect(d.kind).toBe("refuse");
    if (d.kind === "refuse") expect(d.reason).toBe("not on allowlist");
  });

  it("second refusal within cooldown is silent", async () => {
    const gate = await createTurnGate(defaults());
    gate.evaluate({
      senderDid: "did:plc:rando",
      senderNick: "rando",
      refusalReason: "no",
      now: t0,
    });
    const d2 = gate.evaluate({
      senderDid: "did:plc:rando",
      senderNick: "rando",
      refusalReason: "no",
      now: t0 + 1000, // 1s later, well within 1h cooldown
    });
    expect(d2.kind).toBe("silent");
  });

  it("refuses again after refusalCooldownMs elapses", async () => {
    const gate = await createTurnGate({ ...defaults(), refusalCooldownMs: 5000 });
    gate.evaluate({
      senderDid: "did:plc:rando",
      senderNick: "rando",
      refusalReason: "no",
      now: t0,
    });
    const d2 = gate.evaluate({
      senderDid: "did:plc:rando",
      senderNick: "rando",
      refusalReason: "no",
      now: t0 + 6000,
    });
    expect(d2.kind).toBe("refuse");
  });

  it("refusal keys per-sender — Alice refused doesn't silence Bob", async () => {
    const gate = await createTurnGate(defaults());
    gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      refusalReason: "no",
      now: t0,
    });
    const d = gate.evaluate({
      senderDid: "did:plc:bob",
      senderNick: "bob",
      refusalReason: "no",
      now: t0,
    });
    expect(d.kind).toBe("refuse");
  });

  it("senderDid=null falls back to nick-keyed refusal", async () => {
    const gate = await createTurnGate(defaults());
    gate.evaluate({
      senderDid: null,
      senderNick: "guest1",
      refusalReason: "no DID",
      now: t0,
    });
    const d2 = gate.evaluate({
      senderDid: null,
      senderNick: "guest1",
      refusalReason: "no DID",
      now: t0 + 500,
    });
    expect(d2.kind).toBe("silent");
    // Different nick is independent
    const d3 = gate.evaluate({
      senderDid: null,
      senderNick: "guest2",
      refusalReason: "no DID",
      now: t0 + 500,
    });
    expect(d3.kind).toBe("refuse");
  });
});

describe("createTurnGate: dispatch-to-dispatch cooldown", () => {
  it("first dispatch always passes regardless of cooldown setting", async () => {
    const gate = await createTurnGate({ ...defaults(), cooldownMs: 10_000 });
    const d = gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0,
    });
    expect(d.kind).toBe("dispatch");
  });

  it("second dispatch within cooldownMs is silent", async () => {
    const gate = await createTurnGate({ ...defaults(), cooldownMs: 1000 });
    gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0,
    });
    const d = gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0 + 500,
    });
    expect(d.kind).toBe("silent");
  });

  it("dispatch resumes after cooldownMs elapses", async () => {
    const gate = await createTurnGate({ ...defaults(), cooldownMs: 1000 });
    gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0,
    });
    const d = gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0 + 1500,
    });
    expect(d.kind).toBe("dispatch");
  });
});

describe("createTurnGate: rolling hourly cap", () => {
  it("allows up to hourlyCap dispatches in a 60-min window", async () => {
    const gate = await createTurnGate({ ...defaults(), hourlyCap: 3 });
    for (let i = 0; i < 3; i++) {
      const d = gate.evaluate({
        senderDid: "did:plc:alice",
        senderNick: "alice",
        now: t0 + i * 100,
      });
      expect(d.kind).toBe("dispatch");
    }
    const d4 = gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0 + 400,
    });
    expect(d4.kind).toBe("silent");
  });

  it("oldest dispatch slides out after 60 min and the cap resets accordingly", async () => {
    const gate = await createTurnGate({ ...defaults(), hourlyCap: 2 });
    gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0,
    });
    gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0 + 1000,
    });
    // Cap full
    expect(
      gate.evaluate({
        senderDid: "did:plc:alice",
        senderNick: "alice",
        now: t0 + 2000,
      }).kind,
    ).toBe("silent");
    // 61 min later — the t0 dispatch should have slid out
    const d = gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      now: t0 + 61 * 60_000,
    });
    expect(d.kind).toBe("dispatch");
  });
});

describe("createTurnGate: per-peer cycle detection", () => {
  it("a peer can dispatch up to cycleTurnCap times in cycleWindowMs", async () => {
    const gate = await createTurnGate({
      ...defaults(),
      hourlyCap: 1000, // don't let hourly cap interfere
      cyclePolicy: { windowMs: 60_000, turnCap: 3, backoffMs: 30_000 },
    });
    for (let i = 0; i < 3; i++) {
      const d = gate.evaluate({
        senderDid: "did:plc:peer",
        senderNick: "peer",
        now: t0 + i * 1000,
      });
      expect(d.kind).toBe("dispatch");
    }
    // Fourth in the window: cycle trip
    const d4 = gate.evaluate({
      senderDid: "did:plc:peer",
      senderNick: "peer",
      now: t0 + 4000,
    });
    expect(d4.kind).toBe("silent");
  });

  it("trip silences the peer until cycleBackoffMs elapses", async () => {
    const gate = await createTurnGate({
      ...defaults(),
      hourlyCap: 1000,
      cyclePolicy: { windowMs: 60_000, turnCap: 2, backoffMs: 30_000 },
    });
    // Trigger cycle: 2 allowed, 3rd trips
    gate.evaluate({
      senderDid: "did:plc:peer",
      senderNick: "peer",
      now: t0,
    });
    gate.evaluate({
      senderDid: "did:plc:peer",
      senderNick: "peer",
      now: t0 + 100,
    });
    gate.evaluate({
      senderDid: "did:plc:peer",
      senderNick: "peer",
      now: t0 + 200,
    });
    // During backoff: silent
    expect(
      gate.evaluate({
        senderDid: "did:plc:peer",
        senderNick: "peer",
        now: t0 + 1000,
      }).kind,
    ).toBe("silent");
    // After backoff: dispatch resumes. Trip happened at t0+200 with
    // backoffMs=30_000, so backoff ends at t0+30_200; pick a time
    // after that.
    const d = gate.evaluate({
      senderDid: "did:plc:peer",
      senderNick: "peer",
      now: t0 + 31_000,
    });
    expect(d.kind).toBe("dispatch");
  });

  it("cycle backoff is per-peer — other senders unaffected", async () => {
    const gate = await createTurnGate({
      ...defaults(),
      hourlyCap: 1000,
      cyclePolicy: { windowMs: 60_000, turnCap: 2, backoffMs: 30_000 },
    });
    // Trip peer-A
    for (let i = 0; i < 3; i++) {
      gate.evaluate({
        senderDid: "did:plc:peerA",
        senderNick: "peerA",
        now: t0 + i * 100,
      });
    }
    // peer-B is unaffected
    const d = gate.evaluate({
      senderDid: "did:plc:peerB",
      senderNick: "peerB",
      now: t0 + 500,
    });
    expect(d.kind).toBe("dispatch");
  });

  it("skipCycleDetection bypasses tracking entirely", async () => {
    const gate = await createTurnGate({
      ...defaults(),
      hourlyCap: 1000,
      cyclePolicy: { windowMs: 60_000, turnCap: 1, backoffMs: 30_000 },
    });
    // Without skip: 2nd would trip
    // With skip: every dispatch passes
    for (let i = 0; i < 20; i++) {
      const d = gate.evaluate({
        senderDid: "did:plc:owner",
        senderNick: "owner",
        skipCycleDetection: true,
        now: t0 + i * 100,
      });
      expect(d.kind).toBe("dispatch");
    }
  });
});

describe("createTurnGate: persistence", () => {
  it("load restores state — a sender refused before is still in cooldown", async () => {
    const seedState: TurnGateState = {
      lastRefusalAt: [["did:plc:alice", t0]],
      lastDispatchAt: 0,
      dispatchTimestamps: [],
      perPeerDispatches: [],
      cycleBackoffUntil: [],
    };
    const gate = await createTurnGate({
      ...defaults(),
      load: async () => seedState,
    });
    const d = gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      refusalReason: "still no",
      now: t0 + 1000, // within the 1h refusal cooldown
    });
    expect(d.kind).toBe("silent");
  });

  it("persist invokes save with a JSON-serializable snapshot", async () => {
    const save = vi.fn().mockResolvedValue(undefined);
    const gate = await createTurnGate({ ...defaults(), save });
    gate.evaluate({
      senderDid: "did:plc:alice",
      senderNick: "alice",
      refusalReason: "no",
      now: t0,
    });
    await gate.persist();
    expect(save).toHaveBeenCalledTimes(1);
    const snapshot = save.mock.calls[0]![0] as TurnGateState;
    // Round-trips through JSON unchanged (i.e. no Maps in the snapshot).
    expect(JSON.parse(JSON.stringify(snapshot))).toEqual(snapshot);
    expect(snapshot.lastRefusalAt).toEqual([["did:plc:alice", t0]]);
  });

  it("persist without save configured is a no-op (in-memory-only mode)", async () => {
    const gate = await createTurnGate(defaults());
    await expect(gate.persist()).resolves.toBeUndefined();
  });

  it("load failure throws (refuses to start with unknown state)", async () => {
    await expect(
      createTurnGate({
        ...defaults(),
        load: async () => {
          throw new Error("disk gone");
        },
      }),
    ).rejects.toThrow("disk gone");
  });

  it("round-trip: persist → reload → state intact", async () => {
    const stored: { snapshot: TurnGateState | null } = { snapshot: null };
    const g1 = await createTurnGate({
      ...defaults(),
      cyclePolicy: { windowMs: 60_000, turnCap: 2, backoffMs: 30_000 },
      save: async (s) => {
        stored.snapshot = s;
      },
    });
    // Trip cycle on peerA, refuse rando
    g1.evaluate({
      senderDid: "did:plc:peerA",
      senderNick: "peerA",
      now: t0,
    });
    g1.evaluate({
      senderDid: "did:plc:peerA",
      senderNick: "peerA",
      now: t0 + 100,
    });
    g1.evaluate({
      senderDid: "did:plc:peerA",
      senderNick: "peerA",
      now: t0 + 200,
    }); // trips
    g1.evaluate({
      senderDid: "did:plc:rando",
      senderNick: "rando",
      refusalReason: "no",
      now: t0 + 300,
    });
    await g1.persist();

    // New gate from the persisted snapshot
    const g2 = await createTurnGate({
      ...defaults(),
      cyclePolicy: { windowMs: 60_000, turnCap: 2, backoffMs: 30_000 },
      load: async () => stored.snapshot ?? emptyState(),
    });
    // peerA still in cycle backoff
    expect(
      g2.evaluate({
        senderDid: "did:plc:peerA",
        senderNick: "peerA",
        now: t0 + 1000,
      }).kind,
    ).toBe("silent");
    // rando still in refusal cooldown
    expect(
      g2.evaluate({
        senderDid: "did:plc:rando",
        senderNick: "rando",
        refusalReason: "no",
        now: t0 + 1000,
      }).kind,
    ).toBe("silent");
  });
});

function emptyState(): TurnGateState {
  return {
    lastRefusalAt: [],
    lastDispatchAt: 0,
    dispatchTimestamps: [],
    perPeerDispatches: [],
    cycleBackoffUntil: [],
  };
}
