// createDidResolver — resolve "who sent this message?" to a DID.
//
// Three sources, in priority order:
//
//   1. `msg.tags.account` — server attaches via account-tag cap when the
//      sender is SASL-authed. Authoritative for that exact message; never
//      stale. Always preferred.
//   2. nick→DID cache — populated by the SDK's `memberDid` events (which
//      fire from WHOIS replies that include a DID). Invalidated on
//      `userRenamed` and `userQuit`, and TTL-expired.
//   3. WHOIS round-trip — `WHOIS <nick>` and race the reply against
//      `timeoutMs`. Concurrent calls for the same nick share one in-flight
//      WHOIS.
//
// `cache` / `whois` knobs let the caller skip stages: strict mode is
// `{cache:false, whois:false}` (account-tag only, return null otherwise);
// fresh mode is `{cache:false}` (always WHOIS, never use stored DIDs);
// no-network mode is `{whois:false}` (cache only, no round-trips).
//
// The cache is best-effort. IRC only broadcasts NICK/QUIT to clients
// sharing a channel with the user; entries for users known only via DM
// won't be invalidated by events. TTL is the safety net. For
// security-sensitive paths, use strict mode.
import type { FreeqEvents } from "@freeq/sdk";

/** Minimal subset of the FreeqClient surface this primitive needs. The
 *  full client satisfies this interface. */
export interface DidResolverClient {
  raw(line: string): void;
  on<K extends "memberDid" | "userRenamed" | "userQuit">(
    event: K,
    handler: FreeqEvents[K],
  ): void;
  off<K extends "memberDid" | "userRenamed" | "userQuit">(
    event: K,
    handler: FreeqEvents[K],
  ): void;
}

export interface DidResolverOptions {
  /** WHOIS race timeout in ms. Default 3000. Per-call override available
   *  via `ResolveOpts.timeoutMs`. */
  timeoutMs?: number;
  /** Cache entries expire this many ms after insert. Default 300_000 (5
   *  min). The cache can miss invalidation events for users not in shared
   *  channels; TTL bounds the staleness window regardless. */
  cacheTtlMs?: number;
}

export interface ResolveOpts {
  /** Override the resolver's default WHOIS timeout for this call. */
  timeoutMs?: number;
  /** Consult/store the nick→DID cache. Default true. Set false for fresh
   *  lookups every time (no stale-cache risk; pays a WHOIS round-trip). */
  cache?: boolean;
  /** Fall back to WHOIS on cache miss. Default true. Set false to
   *  short-circuit: account-tag → cache → null, no round-trip. */
  whois?: boolean;
}

export interface DidResolver {
  /** Resolve the sender's DID. Returns null if the message has no
   *  account-tag, the cache doesn't know, and WHOIS times out (or is
   *  disabled). */
  resolve(
    msg: { from: string; tags?: Record<string, string> },
    opts?: ResolveOpts,
  ): Promise<string | null>;
  /** Detach all SDK event listeners and clear the cache. */
  close(): void;
}

interface CacheEntry {
  did: string;
  expiresAt: number;
}

export function createDidResolver(
  client: DidResolverClient,
  opts: DidResolverOptions = {},
): DidResolver {
  const defaultTimeoutMs = opts.timeoutMs ?? 3000;
  const cacheTtlMs = opts.cacheTtlMs ?? 5 * 60_000;
  const cache = new Map<string, CacheEntry>();
  const pending = new Map<string, Promise<string | null>>();

  const onMemberDid = (nick: string, did: string): void => {
    cache.set(nick.toLowerCase(), { did, expiresAt: Date.now() + cacheTtlMs });
  };
  const onUserRenamed = (from: string): void => {
    cache.delete(from.toLowerCase());
  };
  const onUserQuit = (from: string): void => {
    cache.delete(from.toLowerCase());
  };

  client.on("memberDid", onMemberDid);
  client.on("userRenamed", onUserRenamed);
  client.on("userQuit", onUserQuit);

  function getCached(nick: string): string | null {
    const key = nick.toLowerCase();
    const e = cache.get(key);
    if (!e) return null;
    if (e.expiresAt <= Date.now()) {
      cache.delete(key);
      return null;
    }
    return e.did;
  }

  function whoisAndWait(nick: string, timeoutMs: number): Promise<string | null> {
    const key = nick.toLowerCase();
    const existing = pending.get(key);
    if (existing) return existing;

    const promise = new Promise<string | null>((resolve) => {
      let settled = false;
      const settle = (value: string | null): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        client.off("memberDid", listener);
        pending.delete(key);
        resolve(value);
      };
      const listener = (gotNick: string, did: string): void => {
        if (gotNick.toLowerCase() !== key) return;
        settle(did);
      };
      const timer = setTimeout(() => settle(null), timeoutMs);
      // Don't keep the event loop alive just for this timer.
      timer.unref?.();
      client.on("memberDid", listener);
      client.raw(`WHOIS ${nick}`);
    });
    pending.set(key, promise);
    return promise;
  }

  return {
    async resolve(msg, callOpts = {}) {
      // 1. account-tag (always preferred; authoritative for the message)
      const tag = msg.tags?.account;
      if (tag && tag.startsWith("did:")) return tag;

      const useCache = callOpts.cache ?? true;
      const useWhois = callOpts.whois ?? true;
      const timeoutMs = callOpts.timeoutMs ?? defaultTimeoutMs;

      // 2. cache (unless disabled)
      if (useCache) {
        const cached = getCached(msg.from);
        if (cached) return cached;
      }

      // 3. WHOIS (unless disabled)
      if (useWhois) {
        return whoisAndWait(msg.from, timeoutMs);
      }

      return null;
    },
    close() {
      client.off("memberDid", onMemberDid);
      client.off("userRenamed", onUserRenamed);
      client.off("userQuit", onUserQuit);
      cache.clear();
      pending.clear();
    },
  };
}
