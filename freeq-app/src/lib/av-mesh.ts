/**
 * AV call mesh: who subscribes to whom.
 *
 * A freeq AV call is a full mesh over the MoQ SFU. Every participant
 * publishes ONE broadcast (their mic + camera) under a path
 * `{sessionId}/{nick}~{instance}`; everyone else subscribes to it. The
 * server's session roster (`/api/v1/sessions/{id}`) is the same for all
 * pollers, so the ONLY per-client decision is "which roster entries are
 * *me*, and what broadcast path does each *other* entry publish under".
 *
 * Getting that decision wrong is exactly how a "network split" looks from
 * the outside: A hears B but C doesn't, because C wrongly dropped B's slot
 * from its subscribe set, or computed a path that doesn't match what B
 * actually publishes. This module is the single source of truth for both
 * halves — the publisher and every subscriber build their paths through
 * {@link broadcastName}, so the two can't drift — and {@link computeParticipantSlots}
 * is the one place "is this me?" is decided.
 */

/** A participant as returned by `GET /api/v1/sessions/{id}`. */
export interface RosterParticipant {
  /** AT Protocol DID — stable identity. Empty/absent for legacy/guest rows. */
  did?: string | null;
  /** Display nick. NOT an identity — two different people can share one. */
  nick: string;
  /** Per-call, per-device random suffix. Globally unique per device-call. */
  instance_id?: string | null;
}

/** This client's own identity within the call. */
export interface SelfIdentity {
  nick: string;
  /** Our per-call instance suffix (null only for legacy clients). */
  instance: string | null;
  /** Our DID, when authenticated (null for guests). */
  did?: string | null;
}

/** A remote broadcast this client should subscribe to. */
export interface Slot {
  nick: string;
  /** `{nick}~{instance}` (or bare `{nick}` with no instance). */
  broadcastKey: string;
  /** Full MoQ broadcast path: `{sessionId}/{broadcastKey}`. */
  broadcastName: string;
}

/**
 * The canonical MoQ broadcast path for a (nick, instance) in a session.
 *
 * Load-bearing invariant: a publisher names its broadcast with this, and
 * every subscriber computes the path to watch with this. Same function on
 * both ends ⇒ the paths cannot diverge. An empty/missing instance yields a
 * bare `{sessionId}/{nick}` (legacy clients that never minted an instance).
 */
export function broadcastName(
  sessionId: string,
  nick: string,
  instance?: string | null,
): string {
  return instance ? `${sessionId}/${nick}~${instance}` : `${sessionId}/${nick}`;
}

/** The `{nick}~{instance}` key (bare `{nick}` when no instance). */
function broadcastKey(nick: string, instance?: string | null): string {
  return instance ? `${nick}~${instance}` : nick;
}

/**
 * Decide whether a roster entry is *this* client (so we don't subscribe to
 * our own broadcast — a feedback loop / wasted decode).
 *
 * Identity is NEVER decided by nick. A nick is a display alias that two
 * different people routinely share (two guests pick "chad"; a guest squats
 * an authed handle; someone renames mid-call). A nick-based self-check makes
 * a client disown a real peer who happens to share its nick — one-way
 * deafness, i.e. a network split. So:
 *
 *   1. Instance first. The per-call instance suffix is a globally-unique
 *      per-device-call id, so an exact instance match is unambiguously us,
 *      and a different instance is unambiguously someone else (including our
 *      OWN other device, which we DO subscribe to).
 *   2. DID fallback for legacy/native rows with no instance: us iff the DID
 *      matches. Two different DIDs sharing a nick are different people.
 *   3. Only as a last resort — no instance and no DID anywhere — fall back
 *      to nick. Degenerate; such rows also collide at the publish path.
 */
function isSelf(p: RosterParticipant, me: SelfIdentity): boolean {
  if (me.instance || p.instance_id) {
    return !!me.instance && p.instance_id === me.instance;
  }
  if (me.did || p.did) {
    return !!me.did && p.did === me.did;
  }
  return p.nick.toLowerCase() === me.nick.toLowerCase();
}

/**
 * Build the subscribe set for this client: one {@link Slot} per *other*
 * live participant. Excludes our own slot; everyone else (including our own
 * other devices) is subscribed so the mesh is complete.
 */
export function computeParticipantSlots(
  participants: RosterParticipant[],
  me: SelfIdentity,
  sessionId: string,
): Slot[] {
  return participants
    .filter((p) => !isSelf(p, me))
    .map((p) => ({
      nick: p.nick,
      broadcastKey: broadcastKey(p.nick, p.instance_id),
      broadcastName: broadcastName(sessionId, p.nick, p.instance_id),
    }));
}
