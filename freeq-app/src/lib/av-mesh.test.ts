/**
 * Deep functional tests for the AV call mesh.
 *
 * The contract under test is "no network splits": in a call of N
 * participants, EVERY participant must subscribe to EVERY other
 * participant's actual published broadcast — exactly once, on the exact
 * path that peer publishes under. A single dropped or mis-pathed slot is a
 * one-way deafness ("A can hear B, B can't hear A") — precisely the
 * symptom reported live.
 *
 * The centrepiece is `simulateCall`, a full-party model: it builds the
 * server roster the way `session_to_json` does, then asks each client what
 * it would subscribe to (via `computeParticipantSlots`) and what it would
 * publish (via `broadcastName`), and computes the reachability matrix. Any
 * empty cell is a split.
 */
import { describe, it, expect } from 'vitest';
import {
  broadcastName,
  computeParticipantSlots,
  type RosterParticipant,
  type SelfIdentity,
} from './av-mesh';

// ── Full-party simulation harness ──────────────────────────────────

interface Member {
  did: string | null;
  nick: string;
  instance: string | null;
}

/**
 * Model a whole call. Returns, for each ordered pair (listener, speaker),
 * whether `listener` subscribes to `speaker`'s real published path.
 *
 * `roster` mirrors what the server hands every client: one entry per live
 * participant (the server already drops `left_at` rows). Each client sees
 * the SAME roster — the only variation is which entry it treats as itself.
 */
function simulateCall(sessionId: string, members: Member[]) {
  const roster: RosterParticipant[] = members.map((m) => ({
    did: m.did,
    nick: m.nick,
    instance_id: m.instance,
  }));

  // What each member actually publishes under.
  const published = new Map<Member, string>(
    members.map((m) => [m, broadcastName(sessionId, m.nick, m.instance)]),
  );

  // What each member subscribes to.
  const subscriptions = new Map<Member, Set<string>>(
    members.map((m) => {
      const me: SelfIdentity = { nick: m.nick, instance: m.instance, did: m.did };
      const slots = computeParticipantSlots(roster, me, sessionId);
      return [m, new Set(slots.map((s) => s.broadcastName))];
    }),
  );

  /** Does `listener` hear `speaker`? */
  const hears = (listener: Member, speaker: Member): boolean =>
    subscriptions.get(listener)!.has(published.get(speaker)!);

  return { roster, published, subscriptions, hears };
}

/** Every distinct pair (A,B), A≠B, that fails bidirectional audibility. */
function findSplits(sessionId: string, members: Member[]): string[] {
  const { hears } = simulateCall(sessionId, members);
  const splits: string[] = [];
  for (const a of members) {
    for (const b of members) {
      if (a === b) continue;
      if (!hears(a, b)) {
        splits.push(`${a.nick}(${a.instance ?? '-'}) cannot hear ${b.nick}(${b.instance ?? '-'})`);
      }
    }
  }
  return splits;
}

// ── Canonical path builder ─────────────────────────────────────────

describe('broadcastName', () => {
  it('builds {session}/{nick}~{instance}', () => {
    expect(broadcastName('S', 'alice', 'web1')).toBe('S/alice~web1');
  });
  it('omits the suffix when no instance (legacy)', () => {
    expect(broadcastName('S', 'alice', null)).toBe('S/alice');
    expect(broadcastName('S', 'alice', '')).toBe('S/alice');
    expect(broadcastName('S', 'alice')).toBe('S/alice');
  });
});

// ── Subscribe-set basics ───────────────────────────────────────────

describe('computeParticipantSlots', () => {
  const sid = 'SESS';

  it('subscribes to every other participant, never to self', () => {
    const roster: RosterParticipant[] = [
      { did: 'did:a', nick: 'alice', instance_id: 'a1' },
      { did: 'did:b', nick: 'bob', instance_id: 'b1' },
      { did: 'did:c', nick: 'carol', instance_id: 'c1' },
    ];
    const slots = computeParticipantSlots(roster, { nick: 'alice', instance: 'a1', did: 'did:a' }, sid);
    expect(slots.map((s) => s.broadcastName).sort()).toEqual(['SESS/bob~b1', 'SESS/carol~c1']);
  });

  it('subscribes to my OWN other device (same DID, different instance)', () => {
    const roster: RosterParticipant[] = [
      { did: 'did:a', nick: 'alice', instance_id: 'web' },
      { did: 'did:a', nick: 'alice', instance_id: 'phone' },
    ];
    const slots = computeParticipantSlots(roster, { nick: 'alice', instance: 'web', did: 'did:a' }, sid);
    expect(slots.map((s) => s.broadcastName)).toEqual(['SESS/alice~phone']);
  });

  it('computes a legacy (no-instance) peer path that matches its publish', () => {
    const roster: RosterParticipant[] = [
      { did: 'did:a', nick: 'alice', instance_id: 'web' },
      { did: 'did:legacy', nick: 'oldclient', instance_id: null },
    ];
    const slots = computeParticipantSlots(roster, { nick: 'alice', instance: 'web', did: 'did:a' }, sid);
    expect(slots.map((s) => s.broadcastName)).toEqual(['SESS/oldclient']);
    // And that path is exactly what the legacy client publishes.
    expect(broadcastName(sid, 'oldclient', null)).toBe('SESS/oldclient');
  });
});

// ── Full-mesh completeness (the no-split invariant) ────────────────

describe('full-mesh completeness', () => {
  it('a 4-person call has zero splits', () => {
    const members: Member[] = [
      { did: 'did:a', nick: 'alice', instance: 'a1' },
      { did: 'did:b', nick: 'bob', instance: 'b1' },
      { did: 'did:c', nick: 'carol', instance: 'c1' },
      { did: 'did:e', nick: 'eliza', instance: 'e1' },
    ];
    expect(findSplits('S', members)).toEqual([]);
  });

  it('one person on two devices: everyone hears both devices, devices hear each other', () => {
    const members: Member[] = [
      { did: 'did:a', nick: 'alice', instance: 'web' },
      { did: 'did:a', nick: 'alice', instance: 'phone' },
      { did: 'did:b', nick: 'bob', instance: 'b1' },
    ];
    expect(findSplits('S', members)).toEqual([]);
  });

  it('a mixed legacy + modern call has zero splits', () => {
    const members: Member[] = [
      { did: 'did:a', nick: 'alice', instance: 'a1' },
      { did: 'did:legacy', nick: 'oldclient', instance: null },
      { did: 'did:b', nick: 'bob', instance: 'b1' },
    ];
    expect(findSplits('S', members)).toEqual([]);
  });
});

// ── Split regressions: identity must not be decided by nick ────────

describe('no split when two different people share a nick', () => {
  // freeq identity is the DID; a nick is a display alias and collides
  // (two guests pick "chad"; a guest squats an authed user's handle).
  // Deciding "is this me?" by nick makes a client disown a real peer.
  it('two distinct DIDs, same nick, both with instances → mutual audibility', () => {
    const members: Member[] = [
      { did: 'did:real', nick: 'chad', instance: 'realdev' },
      { did: 'did:guest', nick: 'chad', instance: 'guestdev' },
      { did: 'did:b', nick: 'bob', instance: 'b1' },
    ];
    expect(findSplits('S', members)).toEqual([]);
  });

  it('two distinct people, same nick, NEITHER has an instance (native/legacy) → no mutual deafness', () => {
    // This is the worst case for nick-based self-detection: both rows look
    // identical to a nick check, so each disowns the other → they go deaf
    // to each other while everyone else hears both. A classic split.
    const members: Member[] = [
      { did: 'did:x', nick: 'chad', instance: null },
      { did: 'did:y', nick: 'chad', instance: null },
      { did: 'did:b', nick: 'bob', instance: 'b1' },
    ];
    expect(findSplits('S', members)).toEqual([]);
  });
});
