import { describe, it, expect, beforeEach, vi } from "vitest";
import { EventEmitter } from "node:events";
import { createDidResolver, type DidResolverClient } from "./did-resolver.js";

// Minimal mock of the FreeqClient surface the resolver actually uses.
// EventEmitter-backed so we can fire memberDid/userRenamed/userQuit at will,
// and `raw` is a spy so we can assert WHOIS dispatch + drive responses.
class MockClient implements DidResolverClient {
  private emitter = new EventEmitter();
  raw = vi.fn();
  on(event: "memberDid" | "userRenamed" | "userQuit", handler: (...a: never[]) => void): void {
    this.emitter.on(event, handler);
  }
  off(event: "memberDid" | "userRenamed" | "userQuit", handler: (...a: never[]) => void): void {
    this.emitter.off(event, handler);
  }
  // Test helpers
  fireMemberDid(nick: string, did: string): void {
    this.emitter.emit("memberDid", nick, did);
  }
  fireUserRenamed(from: string, newNick: string): void {
    this.emitter.emit("userRenamed", from, newNick);
  }
  fireUserQuit(from: string, reason: string): void {
    this.emitter.emit("userQuit", from, reason);
  }
  listenerCount(event: string): number {
    return this.emitter.listenerCount(event);
  }
}

function msg(from: string, tags: Record<string, string> = {}) {
  return { from, tags };
}

describe("createDidResolver: account-tag short-circuit", () => {
  let client: MockClient;
  beforeEach(() => {
    client = new MockClient();
  });

  it("returns the account-tag DID immediately, no WHOIS", async () => {
    const r = createDidResolver(client);
    const did = await r.resolve(msg("alice", { account: "did:plc:alice" }));
    expect(did).toBe("did:plc:alice");
    expect(client.raw).not.toHaveBeenCalled();
  });

  it("ignores account-tag values that aren't DIDs", async () => {
    const r = createDidResolver(client, { timeoutMs: 50 });
    const p = r.resolve(msg("alice", { account: "not-a-did" }));
    // Falls through to WHOIS path
    expect(client.raw).toHaveBeenCalledWith("WHOIS alice");
    await expect(p).resolves.toBeNull();
  });
});

describe("createDidResolver: cache", () => {
  let client: MockClient;
  beforeEach(() => {
    client = new MockClient();
  });

  it("populates the cache from memberDid events; subsequent lookups skip WHOIS", async () => {
    const r = createDidResolver(client);
    client.fireMemberDid("alice", "did:plc:alice");
    const did = await r.resolve(msg("alice"));
    expect(did).toBe("did:plc:alice");
    expect(client.raw).not.toHaveBeenCalled();
  });

  it("is case-insensitive on lookup", async () => {
    const r = createDidResolver(client);
    client.fireMemberDid("Alice", "did:plc:alice");
    expect(await r.resolve(msg("alice"))).toBe("did:plc:alice");
    expect(await r.resolve(msg("ALICE"))).toBe("did:plc:alice");
    expect(client.raw).not.toHaveBeenCalled();
  });

  it("invalidates cache on userRenamed (drops the old nick)", async () => {
    const r = createDidResolver(client, { timeoutMs: 50 });
    client.fireMemberDid("alice", "did:plc:alice");
    expect(await r.resolve(msg("alice"))).toBe("did:plc:alice");
    client.fireUserRenamed("alice", "alice2");
    // Subsequent lookup for "alice" should NOT return the stale DID
    const p = r.resolve(msg("alice"));
    expect(client.raw).toHaveBeenCalledWith("WHOIS alice");
    await expect(p).resolves.toBeNull();
  });

  it("invalidates cache on userQuit", async () => {
    const r = createDidResolver(client, { timeoutMs: 50 });
    client.fireMemberDid("alice", "did:plc:alice");
    expect(await r.resolve(msg("alice"))).toBe("did:plc:alice");
    client.fireUserQuit("alice", "Leaving");
    const p = r.resolve(msg("alice"));
    expect(client.raw).toHaveBeenCalledWith("WHOIS alice");
    await expect(p).resolves.toBeNull();
  });

  it("expires entries after cacheTtlMs (returns null + re-WHOISes)", async () => {
    vi.useFakeTimers();
    try {
      const r = createDidResolver(client, { cacheTtlMs: 1000, timeoutMs: 50 });
      client.fireMemberDid("alice", "did:plc:alice");
      // Just before expiry: hit
      vi.advanceTimersByTime(999);
      expect(await r.resolve(msg("alice"))).toBe("did:plc:alice");
      expect(client.raw).not.toHaveBeenCalled();
      // After expiry: miss → WHOIS
      vi.advanceTimersByTime(2);
      const p = r.resolve(msg("alice"));
      expect(client.raw).toHaveBeenCalledWith("WHOIS alice");
      await vi.advanceTimersByTimeAsync(60);
      await expect(p).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("createDidResolver: WHOIS race", () => {
  let client: MockClient;
  beforeEach(() => {
    client = new MockClient();
  });

  it("resolves to the DID when memberDid fires before timeout", async () => {
    const r = createDidResolver(client, { timeoutMs: 1000 });
    const p = r.resolve(msg("alice"));
    expect(client.raw).toHaveBeenCalledWith("WHOIS alice");
    // Server responds — memberDid fires
    client.fireMemberDid("alice", "did:plc:alice");
    await expect(p).resolves.toBe("did:plc:alice");
  });

  it("returns null on timeout when memberDid never fires", async () => {
    const r = createDidResolver(client, { timeoutMs: 30 });
    const p = r.resolve(msg("rando"));
    expect(client.raw).toHaveBeenCalledWith("WHOIS rando");
    await expect(p).resolves.toBeNull();
  });

  it("dedupes concurrent WHOIS for the same nick", async () => {
    const r = createDidResolver(client, { timeoutMs: 1000 });
    const p1 = r.resolve(msg("alice"));
    const p2 = r.resolve(msg("alice"));
    const p3 = r.resolve(msg("Alice")); // case-insensitive dedupe
    expect(client.raw).toHaveBeenCalledTimes(1);
    client.fireMemberDid("alice", "did:plc:alice");
    expect(await p1).toBe("did:plc:alice");
    expect(await p2).toBe("did:plc:alice");
    expect(await p3).toBe("did:plc:alice");
  });

  it("dispatches separate WHOIS for different nicks", async () => {
    const r = createDidResolver(client, { timeoutMs: 1000 });
    const p1 = r.resolve(msg("alice"));
    const p2 = r.resolve(msg("bob"));
    expect(client.raw).toHaveBeenCalledTimes(2);
    expect(client.raw).toHaveBeenCalledWith("WHOIS alice");
    expect(client.raw).toHaveBeenCalledWith("WHOIS bob");
    client.fireMemberDid("alice", "did:plc:alice");
    client.fireMemberDid("bob", "did:plc:bob");
    expect(await p1).toBe("did:plc:alice");
    expect(await p2).toBe("did:plc:bob");
  });

  it("respects per-call timeoutMs over the resolver default", async () => {
    vi.useFakeTimers();
    try {
      const r = createDidResolver(client, { timeoutMs: 10_000 });
      const p = r.resolve(msg("rando"), { timeoutMs: 100 });
      await vi.advanceTimersByTimeAsync(150);
      await expect(p).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("createDidResolver: cache + whois option knobs", () => {
  let client: MockClient;
  beforeEach(() => {
    client = new MockClient();
  });

  it("cache:false bypasses the cache (still WHOISes)", async () => {
    const r = createDidResolver(client, { timeoutMs: 1000 });
    client.fireMemberDid("alice", "did:plc:alice"); // cache populated
    const p = r.resolve(msg("alice"), { cache: false });
    expect(client.raw).toHaveBeenCalledWith("WHOIS alice"); // didn't use cache
    client.fireMemberDid("alice", "did:plc:alice");
    expect(await p).toBe("did:plc:alice");
  });

  it("whois:false returns null on cache miss (no WHOIS round-trip)", async () => {
    const r = createDidResolver(client);
    const did = await r.resolve(msg("alice"), { whois: false });
    expect(did).toBeNull();
    expect(client.raw).not.toHaveBeenCalled();
  });

  it("whois:false still returns from cache when hit", async () => {
    const r = createDidResolver(client);
    client.fireMemberDid("alice", "did:plc:alice");
    const did = await r.resolve(msg("alice"), { whois: false });
    expect(did).toBe("did:plc:alice");
    expect(client.raw).not.toHaveBeenCalled();
  });

  it("cache:false + whois:false → account-tag only (strict)", async () => {
    const r = createDidResolver(client);
    // With tag: returned
    expect(
      await r.resolve(msg("alice", { account: "did:plc:alice" }), {
        cache: false,
        whois: false,
      }),
    ).toBe("did:plc:alice");
    // Without tag, even if cache has alice: null (cache bypassed)
    client.fireMemberDid("alice", "did:plc:alice");
    expect(await r.resolve(msg("alice"), { cache: false, whois: false })).toBeNull();
    expect(client.raw).not.toHaveBeenCalled();
  });

  it("account-tag wins regardless of cache/whois settings", async () => {
    const r = createDidResolver(client);
    expect(
      await r.resolve(msg("alice", { account: "did:plc:alice" }), {
        cache: false,
        whois: false,
      }),
    ).toBe("did:plc:alice");
    expect(client.raw).not.toHaveBeenCalled();
  });
});

describe("createDidResolver: lifecycle", () => {
  it("close() detaches all listeners", () => {
    const client = new MockClient();
    const r = createDidResolver(client);
    // Three listeners were added (memberDid, userRenamed, userQuit)
    expect(client.listenerCount("memberDid")).toBe(1);
    expect(client.listenerCount("userRenamed")).toBe(1);
    expect(client.listenerCount("userQuit")).toBe(1);
    r.close();
    expect(client.listenerCount("memberDid")).toBe(0);
    expect(client.listenerCount("userRenamed")).toBe(0);
    expect(client.listenerCount("userQuit")).toBe(0);
  });

  it("close() clears the cache", async () => {
    const client = new MockClient();
    const r = createDidResolver(client, { timeoutMs: 50 });
    client.fireMemberDid("alice", "did:plc:alice");
    expect(await r.resolve(msg("alice"))).toBe("did:plc:alice");
    r.close();
    // After close, the cache is empty. The instance is also no longer wired,
    // so subsequent .resolve() won't get help from new events — but a
    // synchronous cache check would also miss. With whois:false, a post-close
    // lookup returns null cleanly.
    expect(await r.resolve(msg("alice"), { whois: false })).toBeNull();
  });
});
