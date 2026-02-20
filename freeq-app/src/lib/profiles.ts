/**
 * AT Protocol profile fetcher.
 * Fetches avatar, display name, bio from PDS via public API.
 * Caches results in memory with TTL.
 */

export interface ATProfile {
  did: string;
  handle: string;
  displayName?: string;
  avatar?: string;
  description?: string;
}

const cache = new Map<string, { profile: ATProfile; fetchedAt: number }>();
const pending = new Map<string, Promise<ATProfile | null>>();
const CACHE_TTL = 10 * 60 * 1000; // 10 minutes
const PUBLIC_API = 'https://public.api.bsky.app';

export async function fetchProfile(didOrHandle: string): Promise<ATProfile | null> {
  // Check cache
  const cached = cache.get(didOrHandle);
  if (cached && Date.now() - cached.fetchedAt < CACHE_TTL) {
    return cached.profile;
  }

  // Deduplicate concurrent requests
  const existing = pending.get(didOrHandle);
  if (existing) return existing;

  const promise = doFetch(didOrHandle);
  pending.set(didOrHandle, promise);
  try {
    return await promise;
  } finally {
    pending.delete(didOrHandle);
  }
}

async function doFetch(didOrHandle: string): Promise<ATProfile | null> {
  try {
    const url = `${PUBLIC_API}/xrpc/app.bsky.actor.getProfile?actor=${encodeURIComponent(didOrHandle)}`;
    const res = await fetch(url);
    if (!res.ok) return null;
    const data = await res.json();
    const profile: ATProfile = {
      did: data.did,
      handle: data.handle,
      displayName: data.displayName,
      avatar: data.avatar,
      description: data.description,
    };
    cache.set(didOrHandle, { profile, fetchedAt: Date.now() });
    // Also cache by DID if we looked up by handle
    if (didOrHandle !== data.did) {
      cache.set(data.did, { profile, fetchedAt: Date.now() });
    }
    return profile;
  } catch {
    return null;
  }
}

/** Batch-fetch profiles for multiple DIDs. Non-blocking, best-effort. */
export function prefetchProfiles(dids: string[]): void {
  for (const did of dids) {
    if (!cache.has(did)) {
      fetchProfile(did).catch(() => {});
    }
  }
}

export function getCachedProfile(didOrHandle: string): ATProfile | null {
  const cached = cache.get(didOrHandle);
  return cached ? cached.profile : null;
}
