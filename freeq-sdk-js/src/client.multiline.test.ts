/**
 * Unit tests for the JS SDK's `draft/multiline` wire support — both
 * outbound chunking (sendMessage / sendMultiline routing to a BATCH
 * when the cap is acked) and inbound assembly (BATCH chunks reassemble
 * into a single `message` event with the assembled body).
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { FreeqClient } from './client.js';
import type { Message } from './types.js';

// ── WebSocket mock (mirrors client.test.ts so multiline tests are self-contained) ──

type ReadyState = 0 | 1 | 2 | 3;

class MockWebSocket {
  static CONNECTING: ReadyState = 0;
  static OPEN: ReadyState = 1;
  static CLOSING: ReadyState = 2;
  static CLOSED: ReadyState = 3;
  static instances: MockWebSocket[] = [];

  CONNECTING: ReadyState = 0;
  OPEN: ReadyState = 1;
  CLOSING: ReadyState = 2;
  CLOSED: ReadyState = 3;

  url: string;
  readyState: ReadyState = 0;
  bufferedAmount = 0;
  sent: string[] = [];

  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.readyState = 1;
      this.onopen?.({});
    });
  }

  send(data: string): void {
    if (this.readyState !== 1) return;
    this.sent.push(data);
  }

  close(): void {
    this.readyState = 3;
    this.onclose?.({});
  }

  recv(line: string): void {
    this.onmessage?.({ data: line + '\r\n' });
  }
}

beforeEach(() => {
  MockWebSocket.instances = [];
  // @ts-expect-error mock global
  globalThis.WebSocket = MockWebSocket;
  if (!globalThis.crypto || !(globalThis.crypto as { randomUUID?: () => string }).randomUUID) {
    Object.defineProperty(globalThis, 'crypto', {
      value: {
        randomUUID: () => 'uuid-' + Math.random().toString(36).slice(2),
        subtle: {
          generateKey: () => Promise.reject(new Error('Ed25519 unavailable in test env')),
        },
      },
      configurable: true,
      writable: true,
    });
  }
});

afterEach(() => {
  vi.restoreAllMocks();
});

async function flushAsync(): Promise<void> {
  // Each ws.recv() chains another handleLine onto a serialized queue
  // (`lineQueue`), so multiline tests with N chunks need N+ microtask
  // ticks to fully drain. Be generous here — the BATCH dispatch path
  // adds its own awaits on top.
  for (let i = 0; i < 32; i++) await Promise.resolve();
}

/**
 * Build a connected client that has negotiated `draft/multiline` +
 * `batch`. ACK is offered via the server CAP LS line so the SDK
 * actually requests + tracks them as acked.
 */
async function makeMultilineClient(nick = 'alice'): Promise<{
  client: FreeqClient;
  ws: MockWebSocket;
}> {
  const { FreeqClient } = await import('./client.js');
  const client = new FreeqClient({
    url: 'wss://test/irc',
    nick,
    skipInitialBrokerRefresh: true,
  });
  client.connect();
  await flushAsync();
  const ws = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
  ws.recv(
    ':srv CAP * LS :message-tags server-time batch echo-message ' +
      'draft/multiline=max-bytes=40000,max-lines=100',
  );
  await flushAsync();
  // Server ACKs the requested caps (the SDK CAP-REQ logic asks for these)
  ws.recv(':srv CAP * ACK :message-tags server-time batch draft/multiline');
  await flushAsync();
  ws.recv(`:srv 001 ${nick} :Welcome`);
  await flushAsync();
  ws.sent.length = 0;
  return { client, ws };
}

/**
 * Same builder but the server does NOT advertise `draft/multiline`.
 * Lets us test the legacy single-PRIVMSG fallback path.
 */
async function makeLegacyClient(nick = 'alice'): Promise<{
  client: FreeqClient;
  ws: MockWebSocket;
}> {
  const { FreeqClient } = await import('./client.js');
  const client = new FreeqClient({
    url: 'wss://test/irc',
    nick,
    skipInitialBrokerRefresh: true,
  });
  client.connect();
  await flushAsync();
  const ws = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
  ws.recv(':srv CAP * LS :message-tags server-time'); // no batch, no draft/multiline
  await flushAsync();
  ws.recv(':srv CAP * ACK :message-tags server-time');
  await flushAsync();
  ws.recv(`:srv 001 ${nick} :Welcome`);
  await flushAsync();
  ws.sent.length = 0;
  return { client, ws };
}

// ────────────────────────────────────────────────────────────────────
// CAP REQ negotiation
// ────────────────────────────────────────────────────────────────────

describe('draft/multiline CAP REQ', () => {
  it('requests draft/multiline when server advertises it with params', async () => {
    const { FreeqClient } = await import('./client.js');
    const client = new FreeqClient({
      url: 'wss://test/irc',
      nick: 'cap-tester',
      skipInitialBrokerRefresh: true,
    });
    client.connect();
    await flushAsync();
    const ws = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
    ws.recv(
      ':srv CAP * LS :batch draft/multiline=max-bytes=40000,max-lines=100',
    );
    await flushAsync();
    const capReq = ws.sent.find((l) => l.startsWith('CAP REQ'));
    expect(capReq).toBeDefined();
    expect(capReq!).toContain('draft/multiline');
    expect(capReq!).toContain('batch');
  });

  it('does NOT request draft/multiline when not advertised', async () => {
    const { FreeqClient } = await import('./client.js');
    const client = new FreeqClient({
      url: 'wss://test/irc',
      nick: 'cap-tester',
      skipInitialBrokerRefresh: true,
    });
    client.connect();
    await flushAsync();
    const ws = MockWebSocket.instances[MockWebSocket.instances.length - 1]!;
    ws.recv(':srv CAP * LS :batch message-tags');
    await flushAsync();
    const capReq = ws.sent.find((l) => l.startsWith('CAP REQ')) ?? '';
    expect(capReq).not.toContain('draft/multiline');
  });
});

// ────────────────────────────────────────────────────────────────────
// Outbound routing: sendMessage / sendMultiline
// ────────────────────────────────────────────────────────────────────

describe('outbound: sendMessage with multiline cap acked', () => {
  it('emits BATCH frames when text contains \\n', async () => {
    const { client, ws } = await makeMultilineClient();
    client.sendMessage('#room', 'line one\nline two\nline three');
    await flushAsync();
    const opener = ws.sent.find((l) => l.includes('BATCH +') && l.includes('draft/multiline'));
    const closer = ws.sent.find((l) => /BATCH -\S+/.test(l));
    const privmsgs = ws.sent.filter((l) => l.includes('PRIVMSG #room'));
    expect(opener).toBeDefined();
    expect(closer).toBeDefined();
    expect(privmsgs).toHaveLength(3);
    expect(privmsgs[0]).toContain('line one');
    expect(privmsgs[1]).toContain('line two');
    expect(privmsgs[2]).toContain('line three');
    // Each chunk carries batch=<id>
    const m = opener!.match(/BATCH \+(\S+)/);
    const batchId = m![1];
    for (const p of privmsgs) {
      expect(p).toContain(`batch=${batchId}`);
    }
  });

  it('falls through to single PRIVMSG when text has no \\n', async () => {
    const { client, ws } = await makeMultilineClient();
    client.sendMessage('#room', 'single line');
    await flushAsync();
    const opener = ws.sent.find((l) => l.includes('BATCH +'));
    const privmsg = ws.sent.find((l) => l.includes('PRIVMSG #room :single line'));
    expect(opener).toBeUndefined(); // no batch for single-line
    expect(privmsg).toBeDefined();
  });

  it('does NOT emit +freeq.at/multiline tag on BATCH path (real \\n carried via wire)', async () => {
    const { client, ws } = await makeMultilineClient();
    client.sendMessage('#room', 'a\nb');
    await flushAsync();
    expect(ws.sent.some((l) => l.includes('+freeq.at/multiline'))).toBe(false);
  });
});

describe('outbound: sendMessage without multiline cap (legacy)', () => {
  it('falls back to single PRIVMSG with escaped \\n and +freeq.at/multiline tag', async () => {
    const { client, ws } = await makeLegacyClient();
    client.sendMessage('#room', 'a\nb\nc');
    await flushAsync();
    const opener = ws.sent.find((l) => l.includes('BATCH +'));
    expect(opener).toBeUndefined();
    const privmsg = ws.sent.find((l) => l.includes('PRIVMSG #room'));
    expect(privmsg).toBeDefined();
    expect(privmsg!).toContain('+freeq.at/multiline');
    expect(privmsg!).toContain('a\\nb\\nc');
  });
});

describe('outbound: sendMultiline (explicit API)', () => {
  it('accepts a string body and emits BATCH', async () => {
    const { client, ws } = await makeMultilineClient();
    client.sendMultiline('#room', 'one\ntwo');
    await flushAsync();
    const opener = ws.sent.find((l) => l.includes('BATCH +') && l.includes('draft/multiline'));
    expect(opener).toBeDefined();
    // Batch id is async (await signing); not synchronously returned.
    const m = opener!.match(/BATCH \+(ml\w+)/);
    expect(m).not.toBeNull();
  });

  it('accepts an array body, joined with \\n', async () => {
    const { client, ws } = await makeMultilineClient();
    client.sendMultiline('#room', ['alpha', 'beta', 'gamma']);
    await flushAsync();
    const privmsgs = ws.sent.filter((l) => l.includes('PRIVMSG #room'));
    expect(privmsgs).toHaveLength(3);
    expect(privmsgs[0]).toContain('alpha');
    expect(privmsgs[1]).toContain('beta');
    expect(privmsgs[2]).toContain('gamma');
  });

  it('threads opener tags via options.tags onto the BATCH opener only', async () => {
    const { client, ws } = await makeMultilineClient();
    client.sendMultiline('#room', 'x\ny', { tags: { '+reply': 'msg-abc' } });
    await flushAsync();
    const opener = ws.sent.find((l) => l.includes('BATCH +'));
    const privmsgs = ws.sent.filter((l) => l.includes('PRIVMSG #room'));
    expect(opener!).toContain('+reply=msg-abc');
    for (const p of privmsgs) {
      expect(p).not.toContain('+reply=msg-abc');
    }
  });

  it('chunks DO NOT carry +freeq.at/sig — sigs ride on the opener', async () => {
    // We can't easily provision a signing key in the test env (Web
    // Crypto Ed25519 is platform-dependent), so we just verify the
    // CHUNK PRIVMSGs are clean. The opener-sig case is exercised in
    // the comprehensive test that runs under a real signing context.
    const { client, ws } = await makeMultilineClient();
    client.sendMultiline('#room', 'a\nb\nc');
    await flushAsync();
    const privmsgs = ws.sent.filter((l) => l.includes('PRIVMSG #room'));
    for (const p of privmsgs) {
      expect(p).not.toContain('+freeq.at/sig');
    }
  });
});

// ────────────────────────────────────────────────────────────────────
// Inbound: BATCH assembly
// ────────────────────────────────────────────────────────────────────

describe('inbound: draft/multiline assembly', () => {
  it('reassembles N PRIVMSG chunks into one `message` event with real \\n', async () => {
    const { client, ws } = await makeMultilineClient();
    const seen: Array<{ ch: string; msg: Message }> = [];
    client.on('message', (ch, msg) => seen.push({ ch, msg }));
    ws.recv(
      '@msgid=01XYZ;time=2026-05-29T17:00:00.000Z :bob!u@h BATCH +ab1 draft/multiline #room',
    );
    ws.recv('@batch=ab1 :bob!u@h PRIVMSG #room :hello');
    ws.recv('@batch=ab1 :bob!u@h PRIVMSG #room :world');
    ws.recv('@batch=ab1 :bob!u@h PRIVMSG #room :foo');
    ws.recv(':srv BATCH -ab1');
    await flushAsync();
    expect(seen).toHaveLength(1);
    expect(seen[0].ch).toBe('#room');
    expect(seen[0].msg.text).toBe('hello\nworld\nfoo');
    expect(seen[0].msg.id).toBe('01XYZ');
    expect(seen[0].msg.from).toBe('bob');
  });

  it('honors +draft/multiline-concat (joins without separator)', async () => {
    const { client, ws } = await makeMultilineClient();
    const seen: Array<{ ch: string; msg: Message }> = [];
    client.on('message', (ch, msg) => seen.push({ ch, msg }));
    ws.recv('@msgid=01ABC :bob!u@h BATCH +ab2 draft/multiline #room');
    ws.recv('@batch=ab2 :bob!u@h PRIVMSG #room :alpha');
    ws.recv('@batch=ab2;+draft/multiline-concat= :bob!u@h PRIVMSG #room :beta');
    ws.recv('@batch=ab2 :bob!u@h PRIVMSG #room :gamma');
    ws.recv(':srv BATCH -ab2');
    await flushAsync();
    expect(seen).toHaveLength(1);
    expect(seen[0].msg.text).toBe('alphabeta\ngamma');
  });

  it('does NOT emit per-chunk message events while batch is open', async () => {
    const { client, ws } = await makeMultilineClient();
    const seen: unknown[] = [];
    client.on('message', (ch, msg) => seen.push({ ch, msg }));
    ws.recv('@msgid=01X :bob!u@h BATCH +ab3 draft/multiline #room');
    ws.recv('@batch=ab3 :bob!u@h PRIVMSG #room :one');
    ws.recv('@batch=ab3 :bob!u@h PRIVMSG #room :two');
    await flushAsync();
    expect(seen).toHaveLength(0); // not until BATCH -<id>
    ws.recv(':srv BATCH -ab3');
    await flushAsync();
    expect(seen).toHaveLength(1);
  });

  it('routes an opener with +draft/edit through messageEdited (not message)', async () => {
    const { client, ws } = await makeMultilineClient();
    const messages: unknown[] = [];
    const edits: unknown[] = [];
    client.on('message', (...args) => messages.push(args));
    client.on('messageEdited', (...args) => edits.push(args));
    ws.recv(
      '@msgid=02ED;+draft/edit=01ORIG :bob!u@h BATCH +ed1 draft/multiline #room',
    );
    ws.recv('@batch=ed1 :bob!u@h PRIVMSG #room :corrected line 1');
    ws.recv('@batch=ed1 :bob!u@h PRIVMSG #room :corrected line 2');
    ws.recv(':srv BATCH -ed1');
    await flushAsync();
    expect(messages).toHaveLength(0);
    expect(edits).toHaveLength(1);
    // edits = [[bufName, editOf, newText, msgid, isStreaming]]
    expect((edits[0] as unknown[])[1]).toBe('01ORIG');
    expect((edits[0] as unknown[])[2]).toBe('corrected line 1\ncorrected line 2');
  });
});

// ────────────────────────────────────────────────────────────────────
// Inbound: legacy `+freeq.at/multiline` decode normalization
// ────────────────────────────────────────────────────────────────────

describe('inbound: legacy +freeq.at/multiline decode', () => {
  it('SDK decodes \\\\n → \\n so consumers see real line breaks', async () => {
    const { client, ws } = await makeMultilineClient();
    const seen: Array<Message> = [];
    client.on('message', (_ch, msg) => seen.push(msg));
    ws.recv(
      '@msgid=01LG;+freeq.at/multiline= :bob!u@h PRIVMSG #room :line a\\nline b\\nline c',
    );
    await flushAsync();
    expect(seen).toHaveLength(1);
    expect(seen[0].text).toBe('line a\nline b\nline c');
  });

  it('does NOT alter text on a regular PRIVMSG (no tag, no decode)', async () => {
    const { client, ws } = await makeMultilineClient();
    const seen: Array<Message> = [];
    client.on('message', (_ch, msg) => seen.push(msg));
    ws.recv(':bob!u@h PRIVMSG #room :literal \\n stays \\n literal');
    await flushAsync();
    expect(seen).toHaveLength(1);
    expect(seen[0].text).toBe('literal \\n stays \\n literal');
  });
});
