/**
 * DM not refreshing on incoming messages.
 *
 * Reported symptom: when actively viewing a DM with `zapnap`, an incoming
 * PRIVMSG from zapnap arrives but the message list doesn't update. The
 * user has to switch to another conversation and back to see the new
 * message. The user suspects multi-session (logged in via two clients)
 * as a trigger.
 *
 * These tests exercise the most plausible failure modes in the store
 * and its subscription contract:
 *
 *  1. The MessageList selector — `s.channels.get(active)?.messages` —
 *     must return a *new array reference* on every successful addMessage
 *     so Zustand fires re-renders.
 *  2. A subscriber listening to that selector must be notified when a
 *     new DM arrives for the active conversation.
 *  3. addMessage's msgid-dedup short-circuit (`return {}`) must not
 *     leave behind in-place mutations that change observable state
 *     without notifying subscribers (the cause of the buffer being
 *     "auto-joined" without the sidebar re-rendering, etc.).
 *  4. Multi-session redelivery (same msgid arriving twice on different
 *     IRC sessions) must result in exactly one stored message and no
 *     state mutation on the second arrival.
 */
import { describe, it, expect, beforeEach } from 'vitest';

const storage = new Map<string, string>();
// @ts-expect-error mock
globalThis.localStorage = {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => storage.set(k, v),
  removeItem: (k: string) => { storage.delete(k); },
  clear: () => storage.clear(),
  get length() { return storage.size; },
  key: (i: number) => [...storage.keys()][i] ?? null,
};
Object.defineProperty(globalThis, 'crypto', {
  value: { randomUUID: () => 'uuid-' + Math.random().toString(36).slice(2), subtle: {} },
  writable: true, configurable: true,
});
// @ts-expect-error mock
globalThis.window = { localStorage: globalThis.localStorage, location: { hash: '' }, addEventListener: () => {} };

const { useStore } = await import('./store');

beforeEach(() => {
  storage.clear();
  useStore.getState().reset();
});

function dmFromZapnap(id: string, text = 'hi') {
  return {
    id,
    from: 'zapnap',
    text,
    timestamp: new Date(),
    tags: { msgid: id },
    isSelf: false,
  };
}

// ─────────────────────────────────────────────────────────────────
// 1. Selector identity contract
// ─────────────────────────────────────────────────────────────────

describe('DM message list selector returns a fresh array on each addMessage', () => {
  it('messages array reference changes after a new DM arrives', () => {
    const buf = 'zapnap';
    useStore.setState({ activeChannel: buf });
    useStore.getState().addMessage(buf, dmFromZapnap('m1', 'first'));

    const select = (s: ReturnType<typeof useStore.getState>) =>
      s.channels.get(buf.toLowerCase())?.messages ?? [];

    const before = select(useStore.getState());
    useStore.getState().addMessage(buf, dmFromZapnap('m2', 'second'));
    const after = select(useStore.getState());

    expect(after).not.toBe(before);
    expect(after.map(m => m.text)).toContain('second');
  });

  it('channels Map reference changes after addMessage so Zustand fires subscribers', () => {
    const buf = 'zapnap';
    useStore.getState().addMessage(buf, dmFromZapnap('m1'));
    const before = useStore.getState().channels;
    useStore.getState().addMessage(buf, dmFromZapnap('m2'));
    const after = useStore.getState().channels;
    expect(after).not.toBe(before);
  });
});

// ─────────────────────────────────────────────────────────────────
// 2. Subscriber wakes up
// ─────────────────────────────────────────────────────────────────

describe('Zustand subscriber for active DM messages fires on new arrivals', () => {
  it('subscribe(selector) fires when a DM arrives for the active channel', () => {
    const buf = 'zapnap';
    useStore.setState({ activeChannel: buf });
    // Seed the buffer so the selector starts with something non-empty.
    useStore.getState().addMessage(buf, dmFromZapnap('seed'));

    const seen: number[] = [];
    let prev = useStore.getState().channels.get(buf.toLowerCase())?.messages ?? [];
    const unsub = useStore.subscribe((s) => {
      const cur = s.channels.get(s.activeChannel.toLowerCase())?.messages ?? [];
      if (cur !== prev) {
        seen.push(cur.length);
        prev = cur;
      }
    });

    useStore.getState().addMessage(buf, dmFromZapnap('live-1'));
    useStore.getState().addMessage(buf, dmFromZapnap('live-2'));
    unsub();

    expect(seen).toEqual([2, 3]);
  });
});

// ─────────────────────────────────────────────────────────────────
// 3. Dedup short-circuit must not mutate state silently
// ─────────────────────────────────────────────────────────────────

describe('addMessage dedup does not silently mutate state', () => {
  it('a duplicate msgid does not flip ch.isJoined without a state notification', () => {
    const buf = 'zapnap';
    // First arrival establishes the buffer + isJoined=true.
    useStore.getState().addMessage(buf, dmFromZapnap('m1'));
    // Manually clear isJoined to simulate a prior session-state where
    // the buffer existed but wasn't auto-joined yet.
    const ch0 = useStore.getState().channels.get(buf.toLowerCase())!;
    ch0.isJoined = false;
    const channelsRefBefore = useStore.getState().channels;

    // Second arrival has the SAME msgid → dedup path must NOT run any
    // state mutation, including setting isJoined back to true.
    useStore.getState().addMessage(buf, dmFromZapnap('m1'));

    const channelsRefAfter = useStore.getState().channels;
    const isJoinedAfter = useStore.getState().channels.get(buf.toLowerCase())!.isJoined;

    // Either: nothing changed (state ref unchanged AND isJoined unchanged)
    // Or: state ref changed (subscribers notified). Mutating without
    // notifying subscribers is the bug — that combination must NOT happen.
    const refChanged = channelsRefAfter !== channelsRefBefore;
    const fieldChanged = isJoinedAfter !== false;
    expect(refChanged || !fieldChanged).toBe(true);
  });
});

// ─────────────────────────────────────────────────────────────────
// 4. Channel object identity changes
// ─────────────────────────────────────────────────────────────────

describe('Channel object identity changes after addMessage', () => {
  it('the channel object reference changes so memoized React subscribers re-render', () => {
    const buf = 'zapnap';
    useStore.getState().addMessage(buf, dmFromZapnap('m1'));
    const before = useStore.getState().channels.get(buf.toLowerCase());
    useStore.getState().addMessage(buf, dmFromZapnap('m2'));
    const after = useStore.getState().channels.get(buf.toLowerCase());
    expect(after).not.toBe(before);
    // and the messages array on the new ch contains both
    expect(after!.messages.map(m => m.id)).toEqual(['m1', 'm2']);
  });
});

// ─────────────────────────────────────────────────────────────────
// 5. Multi-session redelivery
// ─────────────────────────────────────────────────────────────────

describe('multi-session DM redelivery', () => {
  it('two sessions delivering the same msgid produces exactly one message', () => {
    const buf = 'zapnap';
    useStore.setState({ activeChannel: buf });
    const m = dmFromZapnap('shared-msgid', 'hello');

    // Simulate two IRC sessions on the same browser process each invoking
    // addMessage for the same incoming PRIVMSG.
    useStore.getState().addMessage(buf, m);
    useStore.getState().addMessage(buf, { ...m });

    const ch = useStore.getState().channels.get(buf.toLowerCase())!;
    const matching = ch.messages.filter(x => x.id === 'shared-msgid');
    expect(matching.length).toBe(1);
  });

  it('redelivered DM after a real new message still leaves both messages visible', () => {
    const buf = 'zapnap';
    useStore.setState({ activeChannel: buf });

    useStore.getState().addMessage(buf, dmFromZapnap('A', 'first'));
    // Redelivered duplicate of A — should be deduped.
    useStore.getState().addMessage(buf, dmFromZapnap('A', 'first'));
    // Genuinely new message B.
    useStore.getState().addMessage(buf, dmFromZapnap('B', 'second'));

    const ch = useStore.getState().channels.get(buf.toLowerCase())!;
    expect(ch.messages.map(m => m.id)).toEqual(['A', 'B']);
  });
});
