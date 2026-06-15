/**
 * Behavior tests for AV-session protocol bits in irc/client.ts:
 *  - startAvSession (discover-then-join vs av-start)
 *  - joinAvSession (always carries instance + id tags)
 *  - leaveAvSession (clears module state; sends both tags)
 *  - endAvSession (carries av-id)
 *  - the avSessionUpdate/avSessionRemoved wiring that tears down the
 *    panel when the active session ends
 *
 * Each test was first written against the un-patched code and observed
 * to fail before the fix landed — they pin actual bugs we were chasing.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import type { Mock } from 'vitest';

// Prevent notifications.ts from crashing on `document.title` in node env.
vi.mock('../lib/notifications', () => ({
  notify: () => {},
  setNotificationsEnabled: () => {},
  setSoundEnabled: () => {},
  requestPermission: async () => false,
}));

// ── Global mocks (must be before importing the modules under test) ──
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
  value: {
    randomUUID: () => 'uuid-' + Math.random().toString(36).slice(2),
    getRandomValues: (buf: Uint8Array) => {
      for (let i = 0; i < buf.length; i++) buf[i] = Math.floor(Math.random() * 256);
      return buf;
    },
    subtle: {},
  },
  writable: true, configurable: true,
});
// @ts-expect-error mock
globalThis.window = { localStorage: globalThis.localStorage, location: { hash: '', origin: 'http://localhost' }, addEventListener: () => {} };

const { useStore } = await import('../store');
const {
  startAvSession,
  joinAvSession,
  leaveAvSession,
  endAvSession,
  getAvInstanceId,
  __setClientForTests,
  __resetAvInstanceForTests,
  __wireEventsForTests,
} = await import('./client');

// Stub FreeqClient that records `.on` registrations and exposes an
// `emit(event, ...args)` helper so tests can fire synthetic events.
function makeEventStub(nick = 'me') {
  const handlers = new Map<string, Array<(...args: any[]) => void>>();
  const raw = vi.fn();
  return {
    raw,
    nick,
    on(event: string, fn: (...args: any[]) => void) {
      const list = handlers.get(event) ?? [];
      list.push(fn);
      handlers.set(event, list);
    },
    emit(event: string, ...args: any[]) {
      for (const fn of handlers.get(event) ?? []) fn(...args);
    },
    opts: { url: 'ws://test' },
  };
}

// ── Mock SDK client (just enough for our AV senders) ──

type RawCall = string;
function makeMockClient(nick = 'me'): { raw: Mock<(line: string) => void>; nick: string; rawCalls: RawCall[] } {
  const rawCalls: RawCall[] = [];
  const raw = vi.fn((line: string) => { rawCalls.push(line); });
  return { raw: raw as Mock<(line: string) => void>, nick, rawCalls };
}

// Tag extraction helpers — the TAGMSG lines look like
// `@+freeq.at/av-start=;+freeq.at/av-instance=abc TAGMSG #ch`
function parseTags(line: string): Record<string, string> {
  if (!line.startsWith('@')) return {};
  const end = line.indexOf(' ');
  const tagStr = line.slice(1, end);
  const out: Record<string, string> = {};
  for (const part of tagStr.split(';')) {
    const eq = part.indexOf('=');
    if (eq === -1) out[part] = '';
    else out[part.slice(0, eq)] = part.slice(eq + 1);
  }
  return out;
}

function command(line: string): string {
  // Strip tags prefix if present, then take the first token.
  const start = line.startsWith('@') ? line.indexOf(' ') + 1 : 0;
  return line.slice(start).split(' ')[0];
}

beforeEach(() => {
  storage.clear();
  useStore.getState().reset();
  useStore.setState({
    avSessions: new Map(),
    activeAvSession: null,
    avAudioActive: false,
    avMuted: false,
    avCameraOn: false,
    authDid: null,
    connectionState: 'disconnected',
  });
  __resetAvInstanceForTests();
  vi.restoreAllMocks();
});

afterEach(() => {
  __setClientForTests(null);
});

// ═══════════════════════════════════════════════════════════════
// startAvSession
// ═══════════════════════════════════════════════════════════════

describe('startAvSession', () => {
  beforeEach(() => {
    useStore.setState({
      authDid: 'did:plc:me',
      connectionState: 'connected',
      avAudioActive: false,
    });
  });

  it('sends av-start TAGMSG with +freeq.at/av-instance when no session is active', async () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    // fetch returns no active session
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ active: null }), { status: 200 }) as any,
    );

    await startAvSession('#room');

    const tagMsg = mock.rawCalls.find((l) => command(l) === 'TAGMSG');
    expect(tagMsg, 'no TAGMSG line was sent').toBeDefined();
    const tags = parseTags(tagMsg!);
    expect(tags['+freeq.at/av-start']).toBeDefined();
    expect(tags['+freeq.at/av-instance']).toMatch(/^[0-9a-f]{8}$/);
    expect(useStore.getState().avAudioActive).toBe(true);
  });

  it('joins an existing Active session instead of sending av-start', async () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({ active: { id: 'sess-xyz', state: 'Active', participant_count: 2 } }),
        { status: 200 },
      ) as any,
    );

    await startAvSession('#room');

    const tagMsg = mock.rawCalls.find((l) => command(l) === 'TAGMSG');
    expect(tagMsg).toBeDefined();
    const tags = parseTags(tagMsg!);
    // The right shape is "av-join" with an av-id + instance, NOT av-start.
    expect(tags['+freeq.at/av-start']).toBeUndefined();
    expect(tags['+freeq.at/av-join']).toBeDefined();
    expect(tags['+freeq.at/av-id']).toBe('sess-xyz');
    expect(tags['+freeq.at/av-instance']).toMatch(/^[0-9a-f]{8}$/);
    expect(useStore.getState().avAudioActive).toBe(true);
    expect(useStore.getState().activeAvSession).toBe('sess-xyz');
  });

  it('still proceeds when avAudioActive is true (regression: stuck flag was blocking new calls)', async () => {
    // The previous guard checked avAudioActive and silently no-op'd —
    // which broke the green "Join voice call" button if the flag stuck
    // true after any teardown blip. The right guard is per-channel
    // in-flight (covered in the next test); avAudioActive alone is not
    // a reliable "we're currently calling" signal.
    useStore.setState({ avAudioActive: true });
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ active: null }), { status: 200 }) as any,
    );

    await startAvSession('#room');

    expect(mock.rawCalls).toHaveLength(1);
    expect(mock.rawCalls[0]).toMatch(/av-start/);
  });

  it('suppresses concurrent invocations on the same channel (no duplicate av-start)', async () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    // Make the discovery fetch hang so both invocations race through
    // the guard window.
    let resolveFetch: (r: Response) => void = () => {};
    const fetchPromise = new Promise<Response>((r) => { resolveFetch = r; });
    vi.spyOn(globalThis, 'fetch').mockReturnValue(fetchPromise as any);

    const p1 = startAvSession('#room');
    const p2 = startAvSession('#room');
    resolveFetch(new Response(JSON.stringify({ active: null }), { status: 200 }));
    await Promise.all([p1, p2]);

    const startCount = mock.rawCalls.filter((l) => l.includes('av-start')).length;
    expect(startCount).toBe(1);
  });

  it('warns and exits when not authenticated (no TAGMSG)', async () => {
    useStore.setState({ authDid: null });
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);

    await startAvSession('#room');

    expect(mock.rawCalls).toHaveLength(0);
  });
});

// ═══════════════════════════════════════════════════════════════
// joinAvSession
// ═══════════════════════════════════════════════════════════════

describe('joinAvSession', () => {
  it('always carries both +freeq.at/av-instance and +freeq.at/av-id tags', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);

    joinAvSession('#room', 'sess-abc');

    expect(mock.rawCalls).toHaveLength(1);
    const tags = parseTags(mock.rawCalls[0]);
    expect(tags['+freeq.at/av-join']).toBeDefined();
    expect(tags['+freeq.at/av-instance']).toMatch(/^[0-9a-f]{8}$/);
    expect(tags['+freeq.at/av-id']).toBe('sess-abc');
    expect(useStore.getState().activeAvSession).toBe('sess-abc');
  });

  it('generates a fresh instance suffix and remembers it via getAvInstanceId', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    expect(getAvInstanceId()).toBeNull();

    joinAvSession('#room', 'sess-abc');

    const inst = getAvInstanceId();
    expect(inst).toMatch(/^[0-9a-f]{8}$/);
    const tags = parseTags(mock.rawCalls[0]);
    expect(tags['+freeq.at/av-instance']).toBe(inst);
  });

  it('falls back gracefully when called with no sessionId — does not send half-formed TAGMSG', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);

    joinAvSession('#room', undefined);

    // Sending an av-join with no av-id was useless — the server has no
    // session to route the join to, so the receiver just sees noise. We
    // chose to make it a no-op; this test pins that choice.
    expect(mock.rawCalls).toHaveLength(0);
    expect(useStore.getState().activeAvSession).toBeNull();
  });
});

// ═══════════════════════════════════════════════════════════════
// leaveAvSession
// ═══════════════════════════════════════════════════════════════

describe('leaveAvSession', () => {
  it('sends av-leave with both av-id and av-instance tags', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    joinAvSession('#room', 'sess-abc');
    mock.rawCalls.length = 0;

    leaveAvSession('#room', 'sess-abc');

    expect(mock.rawCalls).toHaveLength(1);
    const tags = parseTags(mock.rawCalls[0]);
    expect(tags['+freeq.at/av-leave']).toBeDefined();
    expect(tags['+freeq.at/av-id']).toBe('sess-abc');
    expect(tags['+freeq.at/av-instance']).toMatch(/^[0-9a-f]{8}$/);
  });

  it('clears currentAvInstance after leaving (so the next call generates a fresh suffix)', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);
    joinAvSession('#room', 'sess-abc');
    const firstInst = getAvInstanceId();
    expect(firstInst).not.toBeNull();

    leaveAvSession('#room', 'sess-abc');

    expect(getAvInstanceId()).toBeNull();
    expect(useStore.getState().activeAvSession).toBeNull();
  });
});

// ═══════════════════════════════════════════════════════════════
// endAvSession
// ═══════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════
// Instance suffix uniqueness across consecutive sessions
// ═══════════════════════════════════════════════════════════════

describe('av-instance lifecycle', () => {
  // State matrix cell #13/#14: after leaving and re-joining (same device
  // or different), the next session must mint a FRESH instance suffix.
  // If the old one stuck around, the SDK's `path == our_name` self-filter
  // wouldn't know which broadcast was "us" the next call and we'd either
  // subscribe to our own echo or skip a legitimate remote.
  it('leaveAvSession clears the instance so the next call mints a new one', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);

    joinAvSession('#room', 'sess-a');
    const firstInst = getAvInstanceId();
    expect(firstInst).toMatch(/^[0-9a-f]{8}$/);

    leaveAvSession('#room', 'sess-a');
    expect(getAvInstanceId()).toBeNull();

    // Re-join — a fresh suffix is minted.
    joinAvSession('#room', 'sess-b');
    const secondInst = getAvInstanceId();
    expect(secondInst).toMatch(/^[0-9a-f]{8}$/);
    expect(secondInst).not.toBe(firstInst);
  });

  it('multiple joinAvSession calls in the same session reuse the same instance', () => {
    // If the user joins the same session twice (idempotent — duplicate
    // click on "Join voice"), we keep the same suffix so the server
    // sees the second join as a no-op on the same slot.
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);

    joinAvSession('#room', 'sess-1');
    const inst = getAvInstanceId();

    joinAvSession('#room', 'sess-1');
    expect(getAvInstanceId()).toBe(inst);

    // Both wire lines must carry the same instance.
    const insts = mock.rawCalls.map((l) => parseTags(l)['+freeq.at/av-instance']);
    expect(insts).toEqual([inst, inst]);
  });
});

describe('endAvSession', () => {
  it('sends av-end with av-id tag', () => {
    const mock = makeMockClient('me');
    __setClientForTests(mock as any);

    endAvSession('#room', 'sess-xyz');

    expect(mock.rawCalls).toHaveLength(1);
    const tags = parseTags(mock.rawCalls[0]);
    expect(tags['+freeq.at/av-end']).toBeDefined();
    expect(tags['+freeq.at/av-id']).toBe('sess-xyz');
  });
});

// ═══════════════════════════════════════════════════════════════
// Active-session cleanup on av-state=ended (regression test for the
// "I left the call on phone but my laptop still shows the panel" bug)
// ═══════════════════════════════════════════════════════════════

describe('avSessionUpdate state=ended (wireEvents integration)', () => {
  it('tears down the local call panel when the SDK emits the active session ending', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as any);

    useStore.setState({
      activeAvSession: 'sess-1',
      avAudioActive: true,
      avCameraOn: true,
      avSessions: new Map([['sess-1', {
        id: 'sess-1',
        channel: '#room',
        createdBy: 'did:plc:me',
        createdByNick: 'me',
        participants: new Map(),
        state: 'active',
        startedAt: new Date(),
      }]]),
    });

    // SDK emits avSessionUpdate with state='ended' on av-state=ended TAGMSG.
    stub.emit('avSessionUpdate', {
      id: 'sess-1',
      channel: '#room',
      createdBy: 'did:plc:me',
      createdByNick: 'me',
      participants: new Map(),
      state: 'ended',
      startedAt: new Date(),
    });

    expect(useStore.getState().avAudioActive).toBe(false);
    expect(useStore.getState().avCameraOn).toBe(false);
    expect(useStore.getState().activeAvSession).toBeNull();
  });

  it('leaves the panel up when a different (non-active) session ends', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as any);

    useStore.setState({
      activeAvSession: 'sess-1',
      avAudioActive: true,
      avSessions: new Map(),
    });

    stub.emit('avSessionUpdate', {
      id: 'sess-2', // not the active one
      channel: '#elsewhere',
      createdBy: 'did:plc:them',
      createdByNick: 'them',
      participants: new Map(),
      state: 'ended',
      startedAt: new Date(),
    });

    expect(useStore.getState().avAudioActive).toBe(true);
    expect(useStore.getState().activeAvSession).toBe('sess-1');
  });

  it('tears down the panel when the SDK reaps the active session via avSessionRemoved', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as any);

    useStore.setState({
      activeAvSession: 'sess-1',
      avAudioActive: true,
      avCameraOn: true,
      avSessions: new Map([['sess-1', {
        id: 'sess-1',
        channel: '#room',
        createdBy: 'did:plc:me',
        createdByNick: 'me',
        participants: new Map(),
        state: 'active',
        startedAt: new Date(),
      }]]),
    });

    stub.emit('avSessionRemoved', 'sess-1');

    expect(useStore.getState().avAudioActive).toBe(false);
    expect(useStore.getState().avCameraOn).toBe(false);
    expect(useStore.getState().activeAvSession).toBeNull();
  });
});
