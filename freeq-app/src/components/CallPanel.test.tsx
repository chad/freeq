// @vitest-environment jsdom
/**
 * Behavior tests for <CallPanel/>.
 *
 * We mount the real component, mock out everything below it (moq elements,
 * fetch for /api/v1/sessions, getUserMedia, the IRC client) and assert
 * what the panel actually does:
 *
 *  - which remote tiles render given a polled participant list (the
 *    user-visible bug: "I could not see the web guy on the iOS app");
 *  - that mute is *actually* applied to <moq-publish> on both attribute
 *    and property (the user's "icon toggles but voice still goes
 *    through" report);
 *  - that camera toggle drives both <moq-publish> visibility and the
 *    local preview's media stream;
 *  - that leaving / unmounting releases all moq resources.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent, act, cleanup } from '@testing-library/react';
import { useStore } from '../store';
import {
  __setClientForTests,
  __resetAvInstanceForTests,
  joinAvSession,
} from '../irc/client';

// ── Mocks below CallPanel ───────────────────────────────────────

// MoQ loader: pretend the script loaded instantly.
vi.mock('../lib/moq-loader', () => ({
  loadMoqComponents: vi.fn(() => Promise.resolve()),
  isMoqLoaded: () => true,
}));

// Profile cache lookups — return null so we always hit the initials fallback.
vi.mock('../lib/profiles', () => ({
  getCachedProfile: () => null,
  prefetchProfiles: vi.fn(),
  fetchProfile: vi.fn(),
}));

// ── DOM fakes for moq-publish / moq-watch ───────────────────────
// These are bare custom elements that just record attributes/properties.
// We can't import the real ones (they need a network) so we stub them.

class FakeMoqElement extends HTMLElement {
  // Track every setAttribute / removeAttribute so tests can assert on it.
}

// Define once, idempotent in case multiple test files load.
if (!customElements.get('moq-publish')) {
  customElements.define('moq-publish', class extends FakeMoqElement {});
}
if (!customElements.get('moq-watch')) {
  customElements.define('moq-watch', class extends FakeMoqElement {});
}

// ── Helpers ─────────────────────────────────────────────────────

function makeMediaStream(): MediaStream {
  const tracks = [{ stop: vi.fn(), kind: 'audio' as const }];
  // Minimal MediaStream surface — getTracks() is all CallPanel uses.
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

/**
 * Install a minimal `client` so `getNick()` (used by CallPanel) returns
 * something, and so `leaveAvSession` doesn't crash. We also set the
 * av-instance via joinAvSession so the broadcast name matches what a
 * real call would publish.
 */
function setupClient(nick: string) {
  const raw = vi.fn();
  __setClientForTests({ nick, raw } as any);
  return raw;
}

function setupSession(): { sessionId: string; instance: string } {
  const sess = makeSession('sess-1');
  useStore.getState().updateAvSession(sess);
  joinAvSession('#room', 'sess-1');
  // joinAvSession sets activeAvSession; flip on audio so the panel renders.
  useStore.getState().setAvAudioActive(true);
  return { sessionId: 'sess-1', instance: '<set-by-joinAvSession>' };
}

function mockSessionsApi(participants: Array<{ nick: string; instance_id?: string | null }>) {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
    const u = String(url);
    if (u.startsWith('/api/v1/sessions/')) {
      return new Response(JSON.stringify({ participants }), { status: 200 }) as any;
    }
    return new Response('{}', { status: 200 }) as any;
  });
}

beforeEach(() => {
  resetStore();
  __resetAvInstanceForTests();
  setupClient('me');
  // getUserMedia default — succeeds for both audio and video.
  Object.defineProperty(globalThis.navigator, 'mediaDevices', {
    value: {
      getUserMedia: vi.fn(() => Promise.resolve(makeMediaStream())),
      enumerateDevices: vi.fn(() => Promise.resolve([])),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    },
    configurable: true,
  });
});

afterEach(() => {
  cleanup();
  __setClientForTests(null);
  vi.restoreAllMocks();
});

// We import CallPanel *after* the mocks above are registered.
import { CallPanel } from './CallPanel';

// flush microtasks + small timers so async effects run
async function flush(times = 3) {
  for (let i = 0; i < times; i++) {
    await act(async () => { await Promise.resolve(); });
  }
}

// ═══════════════════════════════════════════════════════════════
// Participant list logic
// ═══════════════════════════════════════════════════════════════

describe('CallPanel — participant tiles', () => {
  it('renders no remote tile when the API returns zero participants', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container } = render(<CallPanel />);
    await flush();

    expect(container.querySelectorAll('moq-watch')).toHaveLength(0);
  });

  it('renders exactly one remote tile for a different (nick, instance)', async () => {
    setupSession();
    mockSessionsApi([{ nick: 'alice', instance_id: 'aaaaaaaa' }]);

    const { container } = render(<CallPanel />);
    await flush();

    const watches = container.querySelectorAll('moq-watch');
    expect(watches).toHaveLength(1);
    expect(watches[0].getAttribute('name')).toBe('sess-1/alice~aaaaaaaa');
  });

  it('filters out my own slot (matching nick AND matching instance)', async () => {
    setupSession();
    // The instance the IRC layer assigned to *us*
    const myInstance = (await import('../irc/client')).getAvInstanceId()!;
    expect(myInstance).toMatch(/^[0-9a-f]{8}$/);
    mockSessionsApi([{ nick: 'me', instance_id: myInstance }]);

    const { container } = render(<CallPanel />);
    await flush();

    expect(container.querySelectorAll('moq-watch')).toHaveLength(0);
  });

  it('renders one tile for the multi-device same-DID case (my nick, different instance)', async () => {
    setupSession();
    const myInstance = (await import('../irc/client')).getAvInstanceId()!;
    mockSessionsApi([
      { nick: 'me', instance_id: myInstance },     // this device — filter
      { nick: 'me', instance_id: 'deadbeef' },     // the other device — keep
    ]);

    const { container } = render(<CallPanel />);
    await flush();

    const watches = container.querySelectorAll('moq-watch');
    expect(watches).toHaveLength(1);
    expect(watches[0].getAttribute('name')).toBe('sess-1/me~deadbeef');
  });

  it('renders a participant whose instance_id is null even when ours is set (legacy device)', async () => {
    setupSession();
    // Participant from an older SDK that doesn't include instance_id.
    mockSessionsApi([{ nick: 'bob', instance_id: null }]);

    const { container } = render(<CallPanel />);
    await flush();

    const watches = container.querySelectorAll('moq-watch');
    expect(watches).toHaveLength(1);
    // No "~" suffix because the participant has no instance.
    expect(watches[0].getAttribute('name')).toBe('sess-1/bob');
  });

  it('adds a tile within one poll cycle when a new participant appears', async () => {
    setupSession();

    let participants: Array<{ nick: string; instance_id?: string | null }> = [];
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const u = String(url);
      if (u.startsWith('/api/v1/sessions/')) {
        return new Response(JSON.stringify({ participants }), { status: 200 }) as any;
      }
      return new Response('{}', { status: 200 }) as any;
    });

    vi.useFakeTimers({ shouldAdvanceTime: false });
    const { container } = render(<CallPanel />);
    // Microtask-drain so the initial pollParticipants completes.
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    expect(container.querySelectorAll('moq-watch')).toHaveLength(0);

    participants = [{ nick: 'alice', instance_id: 'aaaaaaaa' }];
    await act(async () => {
      vi.advanceTimersByTime(3001);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.useRealTimers();
    await flush();

    expect(fetchSpy).toHaveBeenCalled();
    expect(container.querySelectorAll('moq-watch')).toHaveLength(1);
  });

  it('renders three tiles for three distinct DIDs', async () => {
    // State matrix cell #4: three-way call. All three remote nicks must
    // appear as moq-watch elements with their correct broadcast names.
    setupSession();
    mockSessionsApi([
      { nick: 'alice', instance_id: 'aaaaaaaa' },
      { nick: 'bob', instance_id: 'bbbbbbbb' },
      { nick: 'carol', instance_id: 'cccccccc' },
    ]);

    const { container } = render(<CallPanel />);
    await flush();

    const watches = Array.from(container.querySelectorAll('moq-watch'));
    expect(watches).toHaveLength(3);
    const names = watches.map((w) => w.getAttribute('name')).sort();
    expect(names).toEqual([
      'sess-1/alice~aaaaaaaa',
      'sess-1/bob~bbbbbbbb',
      'sess-1/carol~cccccccc',
    ]);
  });

  it('keeps the live other-instance tile when a phantom stale slot is reaped', async () => {
    // State matrix cell #18: a stale slot for our DID exists alongside a
    // live other-device slot. The server-side reaper takes the stale slot
    // out of the participants list before responding to /api/v1/sessions,
    // and the LIVE same-DID slot must remain.
    setupSession();
    const myInstance = (await import('../irc/client')).getAvInstanceId()!;
    // After reaping: only the live other-device slot (and our own) remain.
    mockSessionsApi([
      { nick: 'me', instance_id: myInstance },       // our own slot — filter
      { nick: 'me', instance_id: 'livesecond' },     // OTHER live device — keep
      // No stale entries here — the reaper would have removed them.
    ]);

    const { container } = render(<CallPanel />);
    await flush();

    const watches = Array.from(container.querySelectorAll('moq-watch'));
    expect(watches).toHaveLength(1);
    expect(watches[0].getAttribute('name')).toBe('sess-1/me~livesecond');
  });

  it('swaps the tile broadcast name when a participant re-joins with a NEW instance id', async () => {
    // State matrix cell #13: same nick, fresh instance after a quick
    // leave/rejoin. The poller sees the new instance the next cycle; the
    // tile mounted previously must be torn down (url cleared, element
    // removed) and a fresh tile mounted for the new broadcast path.
    setupSession();

    let participants: Array<{ nick: string; instance_id?: string | null }> = [
      { nick: 'alice', instance_id: 'first' },
    ];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const u = String(url);
      if (u.startsWith('/api/v1/sessions/')) {
        return new Response(JSON.stringify({ participants }), { status: 200 }) as any;
      }
      return new Response('{}', { status: 200 }) as any;
    });

    vi.useFakeTimers({ shouldAdvanceTime: false });
    const { container } = render(<CallPanel />);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    let watches = Array.from(container.querySelectorAll('moq-watch'));
    expect(watches).toHaveLength(1);
    expect(watches[0].getAttribute('name')).toBe('sess-1/alice~first');
    const firstWatch = watches[0];

    // Alice re-joins from the same device (fresh instance) without ever
    // appearing as a ghost. The new participant list has a different
    // instance_id; the previous watch element must be removed (its url
    // cleared) and a new watch element mounted.
    participants = [{ nick: 'alice', instance_id: 'second' }];
    await act(async () => {
      vi.advanceTimersByTime(3001);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.useRealTimers();
    await flush();

    watches = Array.from(container.querySelectorAll('moq-watch'));
    expect(watches).toHaveLength(1);
    expect(watches[0].getAttribute('name')).toBe('sess-1/alice~second');
    // The previous element was unmounted; its url should have been cleared
    // by the cleanup effect.
    expect(firstWatch.getAttribute('url')).toBe('');
  });

  it('a participant leaving with no av-leave is removed by the next poll cycle', async () => {
    // State matrix cell #12: A's IRC connection drops without an av-leave.
    // Server cleanup runs (leave_for_did_instance or leave_all_for_did),
    // which removes them from the participants list. The poller picks
    // that up on the next cycle and the tile disappears.
    setupSession();
    let participants: Array<{ nick: string; instance_id?: string | null }> = [
      { nick: 'alice', instance_id: 'aaaaaaaa' },
    ];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const u = String(url);
      if (u.startsWith('/api/v1/sessions/')) {
        return new Response(JSON.stringify({ participants }), { status: 200 }) as any;
      }
      return new Response('{}', { status: 200 }) as any;
    });
    vi.useFakeTimers({ shouldAdvanceTime: false });
    const { container } = render(<CallPanel />);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    expect(container.querySelectorAll('moq-watch')).toHaveLength(1);

    // Server's disconnect handler removed alice from participants.
    participants = [];
    await act(async () => {
      vi.advanceTimersByTime(3001);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.useRealTimers();
    await flush();

    expect(container.querySelectorAll('moq-watch')).toHaveLength(0);
  });

  it('removes a tile within one poll cycle when a participant disappears', async () => {
    setupSession();

    let participants: Array<{ nick: string; instance_id?: string | null }> = [
      { nick: 'alice', instance_id: 'aaaaaaaa' },
    ];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) => {
      const u = String(url);
      if (u.startsWith('/api/v1/sessions/')) {
        return new Response(JSON.stringify({ participants }), { status: 200 }) as any;
      }
      return new Response('{}', { status: 200 }) as any;
    });

    vi.useFakeTimers({ shouldAdvanceTime: false });
    const { container } = render(<CallPanel />);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    expect(container.querySelectorAll('moq-watch')).toHaveLength(1);

    participants = [];
    await act(async () => {
      vi.advanceTimersByTime(3001);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.useRealTimers();
    await flush();

    expect(container.querySelectorAll('moq-watch')).toHaveLength(0);
  });
});

// ═══════════════════════════════════════════════════════════════
// Mute
// ═══════════════════════════════════════════════════════════════

describe('CallPanel — mute', () => {
  it('mutes <moq-publish> via both attribute AND property (the icon-but-no-mic bug)', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container } = render(<CallPanel />);
    await flush();

    const pub = container.querySelector('moq-publish') as HTMLElement & { muted?: boolean };
    expect(pub).toBeTruthy();
    expect(pub.hasAttribute('muted')).toBe(false);
    expect(pub.muted).toBeFalsy();

    await act(async () => { useStore.getState().setAvMuted(true); });

    expect(pub.hasAttribute('muted')).toBe(true);
    expect(pub.muted).toBe(true);

    await act(async () => { useStore.getState().setAvMuted(false); });

    expect(pub.hasAttribute('muted')).toBe(false);
    expect(pub.muted).toBe(false);
  });

  it('mute button click toggles avMuted in the store', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container } = render(<CallPanel />);
    await flush();

    const muteBtn = container.querySelector('button[title="Mute"]') as HTMLButtonElement;
    expect(muteBtn).toBeTruthy();
    expect(useStore.getState().avMuted).toBe(false);

    await act(async () => { fireEvent.click(muteBtn); });
    expect(useStore.getState().avMuted).toBe(true);

    const unmuteBtn = container.querySelector('button[title="Unmute"]') as HTMLButtonElement;
    expect(unmuteBtn).toBeTruthy();

    await act(async () => { fireEvent.click(unmuteBtn); });
    expect(useStore.getState().avMuted).toBe(false);
  });
});

// ═══════════════════════════════════════════════════════════════
// Camera
// ═══════════════════════════════════════════════════════════════

describe('CallPanel — camera', () => {
  it('removes `invisible` from <moq-publish> when avCameraOn=true', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container } = render(<CallPanel />);
    await flush();

    const pub = container.querySelector('moq-publish') as HTMLElement;
    expect(pub.hasAttribute('invisible')).toBe(true);

    await act(async () => { useStore.getState().setAvCameraOn(true); });
    await flush();
    expect(pub.hasAttribute('invisible')).toBe(false);
  });

  it('adds `invisible` back when avCameraOn flips to false', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container } = render(<CallPanel />);
    await flush();

    await act(async () => { useStore.getState().setAvCameraOn(true); });
    await flush();
    await act(async () => { useStore.getState().setAvCameraOn(false); });
    await flush();

    const pub = container.querySelector('moq-publish') as HTMLElement;
    expect(pub.hasAttribute('invisible')).toBe(true);
  });

  it('delegates the camera capture to moq-publish — no second getUserMedia({video})', async () => {
    // Regression: CallPanel used to open its own getUserMedia({video:true})
    // for the local preview, *in addition to* moq-publish's internal one
    // for the publish path. On browsers that won't grant the same camera
    // twice, moq-publish's request silently failed (it catches the error)
    // and the broadcast went out with no video rendition — so e.g. Eliza
    // would never see anything on a "what's on my screen?" question.
    // The fix routes the local preview through moq-publish's own capture
    // signal, leaving exactly one getUserMedia per resource.
    setupSession();
    mockSessionsApi([]);

    const stream = {
      getTracks: () => [{ stop: vi.fn(), kind: 'audio' as const }],
      getAudioTracks: () => [{ stop: vi.fn(), kind: 'audio' as const }],
      getVideoTracks: () => [],
    } as unknown as MediaStream;

    const getUserMedia = vi.fn(() => Promise.resolve(stream));
    Object.defineProperty(globalThis.navigator, 'mediaDevices', {
      value: {
        getUserMedia,
        enumerateDevices: vi.fn(() => Promise.resolve([])),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      },
      configurable: true,
    });

    render(<CallPanel />);
    await flush();

    await act(async () => { useStore.getState().setAvCameraOn(true); });
    await flush();

    // The only getUserMedia call CallPanel itself owns is the mic
    // permission probe (audio:true) at call start. moq-publish does the
    // camera capture; CallPanel must not duplicate it.
    const sawVideoFromCallPanel = getUserMedia.mock.calls.some(
      ([c]) => (c as MediaStreamConstraints)?.video !== undefined
                && (c as MediaStreamConstraints)?.video !== false,
    );
    expect(sawVideoFromCallPanel).toBe(false);
  });

  it('toggling camera while NOT in a call does not crash or send anything', async () => {
    // No session active.
    useStore.setState({ avAudioActive: false, activeAvSession: null });
    // CallPanel renders null in this state; toggling the store action
    // shouldn't throw.
    const { container } = render(<CallPanel />);
    await flush();
    expect(container.firstChild).toBeNull();

    await act(async () => { useStore.getState().setAvCameraOn(true); });
    expect(useStore.getState().avCameraOn).toBe(true);
    // No publish/watch elements get mounted.
    expect(container.querySelectorAll('moq-publish')).toHaveLength(0);
  });
});

// ═══════════════════════════════════════════════════════════════
// Leave / unmount / session-end cleanup
// ═══════════════════════════════════════════════════════════════

describe('CallPanel — cleanup', () => {
  it('leave button: calls leaveAvSession and clears avAudioActive + avCameraOn', async () => {
    setupSession();
    useStore.getState().setAvCameraOn(true);
    mockSessionsApi([]);

    // Re-install a client we own so we can assert on the raw line emitted
    // by `leaveAvSession`. (The default beforeEach installs one but we
    // need a fresh spy here.)
    const ownRaw = vi.fn();
    __setClientForTests({ nick: 'me', raw: ownRaw } as any);

    const { container } = render(<CallPanel />);
    await flush();

    const leaveBtn = container.querySelector('button[title="Leave call"]') as HTMLButtonElement;
    expect(leaveBtn).toBeTruthy();

    await act(async () => { fireEvent.click(leaveBtn); });

    expect(useStore.getState().avAudioActive).toBe(false);
    expect(useStore.getState().avCameraOn).toBe(false);
    // leaveAvSession was called → at least one TAGMSG av-leave on the wire.
    const sawLeave = ownRaw.mock.calls.some(
      ([line]: [string]) => line.includes('+freeq.at/av-leave'),
    );
    expect(sawLeave).toBe(true);
  });

  it('on unmount the <moq-publish> element is removed AND its url cleared', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container, unmount } = render(<CallPanel />);
    await flush();

    const pub = container.querySelector('moq-publish') as HTMLElement | null;
    expect(pub).toBeTruthy();
    expect(pub!.getAttribute('url')).not.toBe('');

    await act(async () => { unmount(); });

    // After unmount, the publish element should not be in the document.
    expect(document.querySelector('moq-publish')).toBeNull();
    // And its url attribute should have been cleared (releases publish).
    expect(pub!.getAttribute('url')).toBe('');
  });

  it('renders null when avAudioActive flips to false (panel closes for all participants on av-state=ended)', async () => {
    setupSession();
    mockSessionsApi([]);

    const { container } = render(<CallPanel />);
    await flush();
    expect(container.querySelector('moq-publish')).toBeTruthy();

    // Simulate the wireEvents handler firing on av-state=ended from peer.
    await act(async () => {
      useStore.getState().setAvAudioActive(false);
    });

    expect(container.querySelector('moq-publish')).toBeNull();
    expect(container.querySelectorAll('moq-watch')).toHaveLength(0);
  });

  it('on unmount every <moq-watch> is removed and its url cleared', async () => {
    setupSession();
    mockSessionsApi([
      { nick: 'alice', instance_id: 'aaaa' },
      { nick: 'bob', instance_id: 'bbbb' },
    ]);

    const { container, unmount } = render(<CallPanel />);
    await flush();
    const watches = Array.from(container.querySelectorAll('moq-watch'));
    expect(watches).toHaveLength(2);
    for (const w of watches) expect(w.getAttribute('url')).not.toBe('');

    await act(async () => { unmount(); });

    expect(document.querySelectorAll('moq-watch')).toHaveLength(0);
    for (const w of watches) expect(w.getAttribute('url')).toBe('');
  });
});
