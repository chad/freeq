/**
 * Wiring tests for irc/client.ts (hotspot, gamma 133).
 *
 * Covers two orthogonal seams:
 *
 * A) Exported action functions (sendMessage, sendReply, joinChannel,
 *    partChannel) — these have side effects on the Zustand store *beyond*
 *    the SDK call that were not previously tested.
 *
 * B) wireEvents integration — exercised through __wireEventsForTests so we
 *    drive the real dispatch chain (not the hand-rolled simulation in
 *    client-state.test.ts). Covers:
 *      • 'message' DM auto-creates the buffer + increments mentions
 *      • 'error' "same identity reconnected" triggers fullReset
 *      • 'pinAdded' / 'pinRemoved' dispatches to store
 *      • 'motdStart' / 'motd' append to the MOTD accumulator
 *      • 'historyBatch' merges into existing channel history
 *
 * Pattern: use __setClientForTests to inject a stub, use __wireEventsForTests
 * to attach handlers to the stub, then emit events and assert store state.
 * Runs in the default node environment (no jsdom) — same as client-state.test.ts.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// ── Global mocks (must come before any module import) ──
// Mirrors the setup in client-state.test.ts and client-reconnect.test.ts.
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
globalThis.window = {
  localStorage: globalThis.localStorage,
  location: { hash: '', origin: 'http://localhost' },
  addEventListener: () => {},
};

// Mock notifications so the module-level `document.title` access doesn't throw
// in the node environment.
vi.mock('../lib/notifications', () => ({
  notify: vi.fn(),
  setNotificationsEnabled: vi.fn(),
  setSoundEnabled: vi.fn(),
  requestPermission: vi.fn(),
}));

// Mock @freeq/sdk so we control FreeqClient construction.  The mock client is
// only needed for tests in Section A where the production `connect()` path is
// not exercised; for wireEvents tests we inject our own stub via
// __setClientForTests / __wireEventsForTests.
vi.mock('@freeq/sdk', () => ({
  FreeqClient: class MockFreeqClient {
    nick = 'me';
    joinedChannels = new Set<string>();
    nickToDid: unknown = null;
    opts: Record<string, unknown> = {};
    on() { /* stub */ }
    connect() { /* stub */ }
    disconnect() { /* stub */ }
    setSaslCredentials() { /* stub */ }
    sendMessage = vi.fn();
    sendReply = vi.fn();
    join = vi.fn();
    part = vi.fn();
  },
  format: {},
  prefetchProfiles: vi.fn(),
}));

const { useStore } = await import('../store');
const {
  sendMessage,
  sendReply,
  joinChannel,
  partChannel,
  __setClientForTests,
  __wireEventsForTests,
} = await import('./client');

// ── Stub FreeqClient for wireEvents tests ────────────────────────────────────
// Mirrors the pattern from client-av.test.ts: record `.on` subscriptions and
// provide an `emit` helper so tests can fire synthetic SDK events.
function makeEventStub(nick = 'me') {
  const handlers = new Map<string, Array<(...args: unknown[]) => void>>();
  return {
    nick,
    joinedChannels: new Set<string>(),
    sendMessage: vi.fn(),
    sendReply: vi.fn(),
    join: vi.fn(),
    part: vi.fn(),
    on(event: string, fn: (...args: unknown[]) => void) {
      const list = handlers.get(event) ?? [];
      list.push(fn);
      handlers.set(event, list);
    },
    emit(event: string, ...args: unknown[]) {
      for (const fn of handlers.get(event) ?? []) fn(...args);
    },
  };
}

const s = () => useStore.getState();

beforeEach(() => {
  storage.clear();
  s().reset();
  vi.restoreAllMocks();
});

afterEach(() => {
  __setClientForTests(null);
});

// ═══════════════════════════════════════════════════════════════════════════════
// Section A — Exported action functions
// ═══════════════════════════════════════════════════════════════════════════════

describe('sendMessage', () => {
  it('creates a DM buffer when the target is a nick (not a channel)', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);

    sendMessage('alice', 'hello');

    expect(s().channels.has('alice'), 'DM buffer should exist after sendMessage').toBe(true);
  });

  it('does not create an extra entry when the DM buffer already exists', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);
    s().addChannel('alice');
    const before = s().channels.size;

    sendMessage('alice', 'hello again');

    expect(s().channels.size).toBe(before);
  });

  it('does NOT create a buffer for a channel target (#)', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);

    sendMessage('#general', 'hello channel');

    expect(s().channels.has('#general')).toBe(false);
  });

  it('still creates the DM buffer when no SDK client is connected', () => {
    __setClientForTests(null);

    sendMessage('bob', 'offline DM');

    expect(s().channels.has('bob')).toBe(true);
  });
});

describe('sendReply', () => {
  it('creates a DM buffer when the target is a nick', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);

    sendReply('alice', 'msg-001', 'replying to you');

    expect(s().channels.has('alice')).toBe(true);
  });

  it('does not add a buffer for a channel reply', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);

    sendReply('#general', 'msg-002', 'replying in channel');

    expect(s().channels.has('#general')).toBe(false);
  });
});

describe('joinChannel', () => {
  it('adds the channel to the store', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);

    joinChannel('#news');

    expect(s().channels.has('#news')).toBe(true);
  });

  it('sets the joined channel as active', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);
    // addChannel first so setActiveChannel accepts it, then joinChannel does both
    s().addChannel('#news');
    joinChannel('#news');

    expect(s().activeChannel.toLowerCase()).toBe('#news');
  });
});

describe('partChannel', () => {
  it('removes the channel from the store', () => {
    const stub = makeEventStub('me');
    __setClientForTests(stub as never);
    s().addChannel('#temp');

    partChannel('#temp');

    expect(s().channels.has('#temp')).toBe(false);
  });

  it('persists remaining joined channels to localStorage', () => {
    const stub = makeEventStub('me');
    stub.joinedChannels.add('#stay');
    __setClientForTests(stub as never);
    s().addChannel('#temp');
    s().addChannel('#stay');

    partChannel('#temp');

    const saved = storage.get('freeq-joined-channels');
    expect(saved).toBeDefined();
    const parsed = JSON.parse(saved!) as string[];
    expect(parsed).toContain('#stay');
  });
});

// ═══════════════════════════════════════════════════════════════════════════════
// Section B — wireEvents integration (real dispatch chain)
// ═══════════════════════════════════════════════════════════════════════════════

describe("wireEvents 'message' — DM buffer auto-creation", () => {
  it('creates a buffer for a DM that arrives in an unknown buffer', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);

    stub.emit('message', 'carol', {
      id: 'msg-1', from: 'carol', text: 'hey', timestamp: new Date(), isSelf: false, tags: {},
    });

    expect(s().channels.has('carol'), 'DM buffer auto-created on inbound message').toBe(true);
  });

  it('increments mention count for an inbound DM', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('carol');
    // Ensure carol is not the active channel so incrementMentions fires
    s().setActiveChannel('server');
    const before = s().channels.get('carol')?.mentionCount ?? 0;

    stub.emit('message', 'carol', {
      id: 'msg-2', from: 'carol', text: 'ping', timestamp: new Date(), isSelf: false, tags: {},
    });

    const after = s().channels.get('carol')?.mentionCount ?? 0;
    expect(after).toBeGreaterThan(before);
  });

  it('does NOT increment mention count for a self-sent DM', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('dave');
    s().setActiveChannel('server');

    stub.emit('message', 'dave', {
      id: 'msg-3', from: 'me', text: 'sent by self', timestamp: new Date(), isSelf: true, tags: {},
    });

    expect(s().channels.get('dave')?.mentionCount ?? 0).toBe(0);
  });

  it('adds the message to the channel buffer', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('#room');

    stub.emit('message', '#room', {
      id: 'msg-4', from: 'alice', text: 'hello room', timestamp: new Date(), isSelf: false, tags: {},
    });

    const msgs = s().channels.get('#room')?.messages ?? [];
    expect(msgs.some((m) => (m as { id: string }).id === 'msg-4')).toBe(true);
  });
});

describe("wireEvents 'error' — same identity reconnected", () => {
  it('calls fullReset when the error contains the sentinel string', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    // Put some state in the store so we can verify it gets wiped
    s().addChannel('#test');
    s().setNick('me');

    stub.emit('error', 'same identity reconnected from another location');

    // fullReset clears channels and resets nick to ''
    expect(s().nick).toBe('');
    expect(s().channels.size).toBe(0);
  });

  it('does NOT call fullReset for an unrelated error string', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('#test');

    stub.emit('error', 'connection refused by server');

    // Channel should still be there — fullReset was not called
    expect(s().channels.has('#test')).toBe(true);
  });
});

describe("wireEvents 'pinAdded' / 'pinRemoved'", () => {
  it('pinAdded appends the pin to the channel', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('#pinroom');

    stub.emit('pinAdded', '#pinroom', 'ulid-001', 'alice');

    const pins = s().channels.get('#pinroom')?.pins ?? [];
    expect(pins.some((p) => p.msgid === 'ulid-001')).toBe(true);
  });

  it('pinRemoved removes the pin from the channel', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('#pinroom');
    s().addPin('#pinroom', 'ulid-002', 'alice');

    stub.emit('pinRemoved', '#pinroom', 'ulid-002');

    const pins = s().channels.get('#pinroom')?.pins ?? [];
    expect(pins.some((p) => p.msgid === 'ulid-002')).toBe(false);
  });

  it('pinAdded is idempotent — duplicate pin is not added twice', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('#pinroom');

    stub.emit('pinAdded', '#pinroom', 'ulid-003', 'bob');
    stub.emit('pinAdded', '#pinroom', 'ulid-003', 'bob');

    const pins = s().channels.get('#pinroom')?.pins ?? [];
    expect(pins.filter((p) => p.msgid === 'ulid-003').length).toBe(1);
  });
});

describe("wireEvents 'motdStart' / 'motd'", () => {
  it('motdStart resets the MOTD buffer and clears dismissed flag', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    // Pre-populate stale MOTD
    useStore.setState({ motd: ['old line'], motdDismissed: true });

    stub.emit('motdStart');

    expect(s().motd).toEqual([]);
    expect(s().motdDismissed).toBe(false);
  });

  it('motd lines are appended in order', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    stub.emit('motdStart');

    stub.emit('motd', 'Welcome to freeq');
    stub.emit('motd', 'Have fun!');

    expect(s().motd).toEqual(['Welcome to freeq', 'Have fun!']);
  });
});

describe("wireEvents 'historyBatch'", () => {
  it('merges history messages into the channel', () => {
    const stub = makeEventStub('me');
    __wireEventsForTests(stub as never);
    s().addChannel('#hist');

    const msgs = [
      { id: 'h-1', from: 'alice', text: 'old message', timestamp: new Date(), isSelf: false, tags: {} },
      { id: 'h-2', from: 'bob',   text: 'older message', timestamp: new Date(), isSelf: false, tags: {} },
    ];
    stub.emit('historyBatch', '#hist', msgs);

    const stored = s().channels.get('#hist')?.messages ?? [];
    expect(stored.some((m) => (m as { id: string }).id === 'h-1')).toBe(true);
    expect(stored.some((m) => (m as { id: string }).id === 'h-2')).toBe(true);
  });
});
