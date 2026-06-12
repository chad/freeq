// @vitest-environment jsdom
/**
 * Screen-sharing behaviour for the web call panel.
 *
 * Screen share rides a SECOND moq-publish broadcast `{name}/screen` so the
 * camera+mic publisher is never disturbed (a single MoQ broadcast can't carry
 * mic audio + screen video). Viewers reveal a spotlight tile only when the
 * `…/screen` broadcast's moq-watch `status` goes 'live'.
 *
 * These tests use faithful stubs that model:
 *  - moq-publish's `source` contract (camera/screen/file acquire a track;
 *    removeAttribute('source') closes it; '' throws) + a `video` Signal that
 *    yields the captured track (so we can fire its `ended` event), and
 *  - moq-watch's `status` Signal (offline → live).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent, act, cleanup as rtlCleanup } from '@testing-library/react';
import { useStore } from '../store';
import {
  __setClientForTests,
  __resetAvInstanceForTests,
  joinAvSession,
} from '../irc/client';

vi.mock('../lib/moq-loader', () => ({
  loadMoqComponents: vi.fn(() => Promise.resolve()),
  isMoqLoaded: () => true,
}));
vi.mock('../lib/profiles', () => ({
  getCachedProfile: () => null,
  prefetchProfiles: vi.fn(),
  fetchProfile: vi.fn(),
}));

// jsdom has no MediaStream; the panel builds one for the local preview.
if (typeof (globalThis as { MediaStream?: unknown }).MediaStream === 'undefined') {
  (globalThis as { MediaStream?: unknown }).MediaStream = class {
    tracks: unknown[];
    constructor(tracks: unknown[] = []) {
      this.tracks = tracks;
    }
    getTracks() {
      return this.tracks;
    }
  };
}

// ── tiny @moq/signals-shaped signal ─────────────────────────────
function makeSignal<T>(initial: T) {
  let value = initial;
  const subs = new Set<(v: T) => void>();
  return {
    peek: () => value,
    subscribe(fn: (v: T) => void) {
      subs.add(fn);
      fn(value); // @moq/signals invokes immediately with the current value
      return () => subs.delete(fn);
    },
    set(v: T) {
      value = v;
      subs.forEach((f) => f(v));
    },
  };
}

type FakeTrack = {
  kind: string;
  readyState: string;
  stop: ReturnType<typeof vi.fn>;
  addEventListener: (ev: string, fn: () => void) => void;
  removeEventListener: (ev: string, fn: () => void) => void;
  dispatch: (ev: string) => void;
};

function makeFakeTrack(): FakeTrack {
  const listeners: Record<string, Set<() => void>> = {};
  const t: FakeTrack = {
    kind: 'video',
    readyState: 'live',
    stop: vi.fn(() => {
      t.readyState = 'ended';
    }),
    addEventListener: (ev, fn) => {
      (listeners[ev] ??= new Set()).add(fn);
    },
    removeEventListener: (ev, fn) => {
      listeners[ev]?.delete(fn);
    },
    dispatch: (ev) => {
      listeners[ev]?.forEach((f) => f());
    },
  };
  return t;
}

// ── Faithful <moq-publish> stub with a `video` track Signal ──────
class FakeMoqPublish extends HTMLElement {
  static observedAttributes = ['url', 'name', 'muted', 'invisible', 'source'];
  videoTrack: FakeTrack | null = null;
  #sourceSig = makeSignal<MediaStreamTrack | undefined>(undefined);
  video = makeSignal<{ source: typeof this.#sourceSig } | undefined>(undefined);
  audio = makeSignal<unknown>(undefined);

  attributeChangedCallback(name: string, _old: string | null, value: string | null) {
    if (name !== 'source') return;
    if (value === 'camera' || value === 'screen' || value === 'file') {
      const track = makeFakeTrack();
      this.videoTrack = track;
      this.#sourceSig.set(track as unknown as MediaStreamTrack);
      this.video.set({ source: this.#sourceSig });
    } else if (value === null) {
      // removeAttribute('source') → close capture (the only correct teardown)
      this.videoTrack?.stop();
      this.#sourceSig.set(undefined);
      this.video.set(undefined);
      this.videoTrack = null;
    } else {
      throw new Error(`Invalid source: ${value}`); // '' throws, faithful to moq
    }
  }
}

// ── <moq-watch> stub exposing a settable `status` Signal ─────────
class FakeMoqWatch extends HTMLElement {
  status = makeSignal<string>('offline');
}

if (!customElements.get('moq-publish')) customElements.define('moq-publish', FakeMoqPublish);
if (!customElements.get('moq-watch')) customElements.define('moq-watch', FakeMoqWatch);

// ── harness ─────────────────────────────────────────────────────
function makeMediaStream(): MediaStream {
  const tracks = [{ stop: vi.fn(), kind: 'audio' as const }];
  return {
    getTracks: () => tracks,
    getAudioTracks: () => tracks,
    getVideoTracks: () => [],
  } as unknown as MediaStream;
}

function resetStore() {
  useStore.getState().reset();
  useStore.setState({
    nick: 'me',
    avSessions: new Map(),
    activeAvSession: null,
    avAudioActive: false,
    avMuted: false,
    avCameraOn: false,
    avScreenShareOn: false,
    authDid: 'did:plc:me',
  });
}

function makeSession(id = 'sess-1') {
  return {
    id,
    channel: '#room',
    createdBy: 'did:plc:me',
    createdByNick: 'me',
    participants: new Map(),
    state: 'active' as const,
    startedAt: new Date(),
  };
}

function setupClient(nick: string) {
  const raw = vi.fn();
  __setClientForTests({ nick, raw } as any);
  return raw;
}

function setupSession() {
  useStore.getState().updateAvSession(makeSession('sess-1'));
  joinAvSession('#room', 'sess-1');
  useStore.getState().setAvAudioActive(true);
}

function mockSessionsApi(participants: unknown[] = []) {
  vi.spyOn(globalThis, 'fetch').mockImplementation(
    async () => new Response(JSON.stringify({ participants }), { status: 200 }) as any,
  );
}

function installMediaDevices(withDisplay: boolean) {
  const value: Record<string, unknown> = {
    getUserMedia: vi.fn(() => Promise.resolve(makeMediaStream())),
    enumerateDevices: vi.fn(() => Promise.resolve([])),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  };
  if (withDisplay) value.getDisplayMedia = vi.fn(() => Promise.resolve(makeMediaStream()));
  Object.defineProperty(globalThis.navigator, 'mediaDevices', { value, configurable: true });
}

beforeEach(() => {
  resetStore();
  __resetAvInstanceForTests();
  setupClient('me');
  installMediaDevices(true);
});

afterEach(() => {
  rtlCleanup();
  __setClientForTests(null);
  vi.restoreAllMocks();
});

import { CallPanel } from './CallPanel';

async function flush(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

const screenPub = (root: ParentNode) =>
  root.querySelector('moq-publish[name$="/screen"]') as FakeMoqPublish | null;

describe('CallPanel — screen sharing', () => {
  it('shows a Share screen button when the browser supports getDisplayMedia', async () => {
    setupSession();
    mockSessionsApi();
    const { container } = render(<CallPanel />);
    await flush();
    expect(container.querySelector('button[title="Share screen"]')).toBeTruthy();
  });

  it('hides the Share screen button when getDisplayMedia is unavailable', async () => {
    installMediaDevices(false);
    setupSession();
    mockSessionsApi();
    const { container } = render(<CallPanel />);
    await flush();
    expect(container.querySelector('button[title="Share screen"]')).toBeNull();
    expect(container.querySelector('button[title="Stop sharing screen"]')).toBeNull();
  });

  it('clicking Share screen publishes a separate `…/screen` broadcast with source=screen', async () => {
    setupSession();
    mockSessionsApi();
    const { container } = render(<CallPanel />);
    await flush();

    // No screen broadcast yet — only the camera/mic publisher.
    expect(screenPub(container)).toBeNull();

    await act(async () => {
      fireEvent.click(container.querySelector('button[title="Share screen"]') as HTMLButtonElement);
    });
    await flush();

    const sp = screenPub(container);
    expect(sp).toBeTruthy();
    expect(sp!.getAttribute('source')).toBe('screen');
    expect(sp!.getAttribute('name')).toMatch(/^sess-1\/me(~[a-z0-9]+)?\/screen$/);
    // Video only: published muted.
    expect(sp!.getAttribute('muted')).toBe('');
    // The camera/mic publisher is untouched and still present.
    expect(container.querySelector('moq-publish:not([name$="/screen"])')).toBeTruthy();
    expect(useStore.getState().avScreenShareOn).toBe(true);
  });

  it('toggling Share screen off tears down the screen broadcast and closes the capture', async () => {
    setupSession();
    mockSessionsApi();
    const { container } = render(<CallPanel />);
    await flush();

    await act(async () => {
      fireEvent.click(container.querySelector('button[title="Share screen"]') as HTMLButtonElement);
    });
    await flush();
    const sp = screenPub(container)!;
    const track = sp.videoTrack!;
    expect(track.readyState).toBe('live');

    await act(async () => {
      fireEvent.click(
        container.querySelector('button[title="Stop sharing screen"]') as HTMLButtonElement,
      );
    });
    await flush();

    expect(screenPub(container)).toBeNull();
    expect(track.stop).toHaveBeenCalled(); // removeAttribute('source') closed it
    expect(track.readyState).toBe('ended');
    expect(useStore.getState().avScreenShareOn).toBe(false);
  });

  it("the browser's native Stop-sharing (track 'ended') resets the toggle and tears down", async () => {
    setupSession();
    mockSessionsApi();
    const { container } = render(<CallPanel />);
    await flush();

    await act(async () => {
      fireEvent.click(container.querySelector('button[title="Share screen"]') as HTMLButtonElement);
    });
    await flush();
    const track = screenPub(container)!.videoTrack!;

    await act(async () => {
      track.dispatch('ended'); // user clicked the browser's "Stop sharing"
    });
    await flush();

    expect(useStore.getState().avScreenShareOn).toBe(false);
    expect(screenPub(container)).toBeNull();
  });

  it('leaving the call while sharing closes the screen capture too', async () => {
    setupSession();
    mockSessionsApi();
    const { container } = render(<CallPanel />);
    await flush();
    await act(async () => {
      fireEvent.click(container.querySelector('button[title="Share screen"]') as HTMLButtonElement);
    });
    await flush();
    const track = screenPub(container)!.videoTrack!;

    await act(async () => {
      fireEvent.click(container.querySelector('button[title="Leave call"]') as HTMLButtonElement);
    });
    await flush();

    expect(track.stop).toHaveBeenCalled();
    expect(document.querySelector('moq-publish[name$="/screen"]')).toBeNull();
  });

  it("reveals a remote participant's screen tile only when its broadcast is live", async () => {
    setupSession();
    mockSessionsApi([{ nick: 'bob', instance_id: 'b1' }]);
    const { container } = render(<CallPanel />);
    await flush();

    const watch = container.querySelector(
      'moq-watch[name="sess-1/bob~b1/screen"]',
    ) as FakeMoqWatch | null;
    expect(watch).toBeTruthy(); // mounted up-front to detect the announce
    // Hidden while offline.
    expect(watch!.closest('.hidden')).toBeTruthy();

    // Broadcast goes live → tile revealed, label shown.
    await act(async () => {
      watch!.status.set('live');
    });
    await flush();
    expect(watch!.closest('.hidden')).toBeNull();
    expect(container.textContent).toContain('bob — screen');

    // Stops → hidden again.
    await act(async () => {
      watch!.status.set('offline');
    });
    await flush();
    expect(watch!.closest('.hidden')).toBeTruthy();
  });
});
