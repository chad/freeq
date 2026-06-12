// @vitest-environment jsdom
/**
 * Stale-tab detector tests.
 *
 * The real-world failure (2026-06-12): a tab loaded days before a deploy
 * kept running the old bundle — including a fixed-then-stale camera bug —
 * with no prompt to reload. These tests pin the detection mechanics:
 * extracting the bundle hash from index.html, comparing against the live
 * <script> tag, and prompting (once per visibility cycle) when they differ.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../components/Toast', () => ({ showToast: vi.fn() }));

import { showToast } from '../components/Toast';
import { extractBundleHash, loadedBundleHash, startUpdateCheck } from './update-check';

const INDEX_HTML = (hash: string) => `<!doctype html>
<html><head>
<script type="module" crossorigin src="/assets/${hash}.js"></script>
<link rel="stylesheet" crossorigin href="/assets/index-C5tDKDPl.css">
</head><body><div id="root"></div></body></html>`;

function mountBundleScript(hash: string) {
  const script = document.createElement('script');
  script.type = 'module';
  script.src = `https://irc.freeq.at/assets/${hash}.js`;
  document.head.appendChild(script);
  return script;
}

function mockServedIndex(hash: string) {
  // Fresh Response per call — bodies are one-shot streams and the checker
  // fetches more than once across visibility cycles.
  return vi.spyOn(globalThis, 'fetch').mockImplementation(
    async () => new Response(INDEX_HTML(hash), { status: 200 }),
  );
}

function setVisibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
  document.dispatchEvent(new Event('visibilitychange'));
}

async function flush() {
  // fetch + Response.text() resolve through real macrotasks (undici
  // streams) — microtask-only flushing is not enough.
  for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0));
}

// Failed assertions must not leak visibility listeners into later tests.
let stops: Array<() => void> = [];
function start() {
  const stop = startUpdateCheck();
  stops.push(stop);
  return stop;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  stops.forEach((s) => s());
  stops = [];
  document.head.querySelectorAll('script').forEach((s) => s.remove());
  vi.restoreAllMocks();
});

describe('extractBundleHash', () => {
  it('pulls the hashed bundle name out of index.html', () => {
    expect(extractBundleHash(INDEX_HTML('index-BqhcCn03'))).toBe('index-BqhcCn03');
  });

  it('returns null when there is no hashed bundle (dev index.html)', () => {
    expect(extractBundleHash('<script type="module" src="/src/main.tsx"></script>')).toBeNull();
  });

  it('ignores the css asset', () => {
    expect(extractBundleHash('<link href="/assets/index-C5tDKDPl.css">')).toBeNull();
  });
});

describe('loadedBundleHash', () => {
  it('reads the hash from the live module script tag', () => {
    mountBundleScript('index-CGcAnG_o');
    expect(loadedBundleHash()).toBe('index-CGcAnG_o');
  });

  it('returns null with no bundle script (dev)', () => {
    expect(loadedBundleHash()).toBeNull();
  });
});

describe('startUpdateCheck', () => {
  it('is a no-op in dev (no bundle script tag)', () => {
    const fetchSpy = mockServedIndex('index-NEW');
    const stop = start();
    setVisibility('visible');
    expect(fetchSpy).not.toHaveBeenCalled();
    stop();
  });

  it('does not prompt while the served bundle matches the loaded one', async () => {
    mountBundleScript('index-BqhcCn03');
    mockServedIndex('index-BqhcCn03');
    const stop = start();
    setVisibility('visible');
    await flush();
    expect(showToast).not.toHaveBeenCalled();
    stop();
  });

  it('prompts with a Reload action when a new bundle is deployed', async () => {
    mountBundleScript('index-CGcAnG_o'); // the stale Jun 7 tab
    mockServedIndex('index-BqhcCn03'); // the Jun 10 deploy
    const stop = start();
    setVisibility('visible');
    await flush();
    expect(showToast).toHaveBeenCalledTimes(1);
    const [message, type, duration, action] = vi.mocked(showToast).mock.calls[0];
    expect(message).toMatch(/new version/i);
    expect(type).toBe('info');
    expect(duration).toBe(0); // persistent
    expect(action?.label).toBe('Reload');
    stop();
  });

  it('prompts once per visibility cycle, re-arming when the tab is hidden', async () => {
    mountBundleScript('index-CGcAnG_o');
    mockServedIndex('index-BqhcCn03');
    const stop = start();

    setVisibility('visible');
    await flush();
    setVisibility('visible'); // second focus without going hidden
    await flush();
    expect(showToast).toHaveBeenCalledTimes(1);

    setVisibility('hidden'); // user tabs away (maybe dismissed the toast)
    setVisibility('visible'); // ...and comes back
    await flush();
    expect(showToast).toHaveBeenCalledTimes(2);
    stop();
  });

  it('stays quiet when the served index is unreadable (offline / mid-deploy)', async () => {
    mountBundleScript('index-CGcAnG_o');
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('offline'));
    const stop = start();
    setVisibility('visible');
    await flush();
    expect(showToast).not.toHaveBeenCalled();
    stop();
  });

  it('stops checking after cleanup', async () => {
    mountBundleScript('index-CGcAnG_o');
    const fetchSpy = mockServedIndex('index-BqhcCn03');
    const stop = start();
    stop();
    setVisibility('visible');
    await flush();
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });
});
