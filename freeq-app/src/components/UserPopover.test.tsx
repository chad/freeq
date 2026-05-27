// @vitest-environment jsdom
/**
 * Tests for UserPopover provenance rendering.
 *
 * Focus is the creator-lineage walk that powers the "Creator: lobot ← Nap"
 * display in sub-agent cards. The walk has cycle detection, max-depth
 * truncation, and async ordering — all places where silent regressions
 * are likely if someone later refactors.
 *
 * `walkCreatorChain` is tested as a pure function (cheap, fast). A
 * couple of light `<ProvenanceBlock>` mounts check that the chain
 * reaches the DOM with arrows + clickable buttons; no DOM-structure
 * brittleness.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor, fireEvent } from '@testing-library/react';
import {
  walkCreatorChain,
  CREATOR_CHAIN_MAX_DEPTH,
  ProvenanceBlock,
} from './UserPopover';

// Mock the AT Protocol profile fetch — ProvenanceBlock imports it
// directly, so vi.mock has to be at module scope.
vi.mock('../lib/profiles', () => ({
  fetchProfile: vi.fn(async () => null),
}));

// Toast import is dynamic (clipboard copy feedback). Stub it so the
// import resolution doesn't blow up under jsdom.
vi.mock('./Toast', () => ({
  showToast: vi.fn(),
}));

// ── walkCreatorChain (pure logic) ───────────────────────────────

describe('walkCreatorChain', () => {
  it('returns empty when root is missing', async () => {
    const fetchActor = vi.fn();
    const fetchProfile = vi.fn();
    const chain = await walkCreatorChain(undefined, fetchActor, fetchProfile);
    expect(chain).toEqual([]);
    expect(fetchActor).not.toHaveBeenCalled();
    expect(fetchProfile).not.toHaveBeenCalled();
  });

  it('walks a 2-link chain bot → bot → human, marking did:plc as human', async () => {
    // panel-2's view: creator is lobot (did:key bot), lobot's creator
    // is a did:plc human (zapnap).
    const fetchActor = vi.fn(async (did: string) => {
      if (did === 'did:key:lobot') {
        return { nick: 'lobot', provenance: { creator_did: 'did:plc:zap' } };
      }
      if (did === 'did:plc:zap') {
        // Humans declare no provenance — walk terminates here.
        return { nick: 'zapnap', provenance: null };
      }
      return null;
    });
    const fetchProfile = vi.fn(async (did: string) => {
      if (did === 'did:plc:zap') {
        return { displayName: 'Nap', handle: 'zapnap.bsky.social', avatar: 'a.png' };
      }
      return null;
    });

    const chain = await walkCreatorChain('did:key:lobot', fetchActor, fetchProfile);

    expect(chain).toEqual([
      {
        did: 'did:key:lobot',
        nick: 'lobot',
        displayName: null,
        avatar: null,
        isHuman: false,
      },
      {
        did: 'did:plc:zap',
        nick: 'zapnap',
        displayName: 'Nap',
        avatar: 'a.png',
        isHuman: true,
      },
    ]);
    // Profile only fetched for did:plc (skipped for did:key bots).
    expect(fetchProfile).toHaveBeenCalledTimes(1);
    expect(fetchProfile).toHaveBeenCalledWith('did:plc:zap');
  });

  it('stops on cycle without infinite loop', async () => {
    // Pathological data: bot1 claims itself as its own creator.
    const fetchActor = vi.fn(async (did: string) => ({
      nick: did.slice('did:key:'.length),
      provenance: { creator_did: did }, // points back to self
    }));
    const fetchProfile = vi.fn(async () => null);

    const chain = await walkCreatorChain(
      'did:key:bot1',
      fetchActor,
      fetchProfile,
    );

    expect(chain).toHaveLength(1);
    expect(chain[0].did).toBe('did:key:bot1');
    // fetchActor called once — the cycle guard fires before the second
    // iteration's fetch.
    expect(fetchActor).toHaveBeenCalledTimes(1);
  });

  it('respects maxDepth on an unbounded chain', async () => {
    // Each bot points to a fresh distinct bot. Without the depth cap,
    // this would loop forever.
    let counter = 0;
    const fetchActor = vi.fn(async () => {
      counter++;
      return {
        nick: `bot${counter}`,
        provenance: { creator_did: `did:key:bot${counter + 1000}` },
      };
    });
    const fetchProfile = vi.fn(async () => null);

    const chain = await walkCreatorChain(
      'did:key:bot0',
      fetchActor,
      fetchProfile,
      5,
    );

    expect(chain).toHaveLength(5);
    expect(fetchActor).toHaveBeenCalledTimes(5);
  });

  it('default max depth is 8', () => {
    expect(CREATOR_CHAIN_MAX_DEPTH).toBe(8);
  });

  it('includes a link with no nick when the actor endpoint fails', async () => {
    // Simulates an offline creator whose identities row is also
    // missing — fetchActor returns null. The link should still appear
    // (so the chain is visible) but with nick=null and the walk
    // terminates because there's no further creator_did.
    const fetchActor = vi.fn(async () => null);
    const fetchProfile = vi.fn(async () => null);

    const chain = await walkCreatorChain(
      'did:key:unknown',
      fetchActor,
      fetchProfile,
    );

    expect(chain).toHaveLength(1);
    expect(chain[0]).toMatchObject({
      did: 'did:key:unknown',
      nick: null,
      displayName: null,
      isHuman: false,
    });
  });

  it('swallows fetchActor exceptions and treats them like null', async () => {
    // Network glitches shouldn't crash the popover.
    const fetchActor = vi.fn(async () => {
      throw new Error('network down');
    });
    const fetchProfile = vi.fn(async () => null);

    const chain = await walkCreatorChain(
      'did:key:flaky',
      fetchActor,
      fetchProfile,
    );

    expect(chain).toHaveLength(1);
    expect(chain[0].nick).toBeNull();
  });
});

// ── ProvenanceBlock (component) ─────────────────────────────────

describe('<ProvenanceBlock>', () => {
  beforeEach(() => {
    // Reset clipboard mock between tests so writeText calls don't bleed.
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn() },
      configurable: true,
    });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('renders chain links separated by an arrow', async () => {
    // Stub /api/v1/actors with a 2-link chain: lobot → zapnap (did:plc).
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes(encodeURIComponent('did:key:lobot'))) {
        return new Response(
          JSON.stringify({
            nick: 'lobot',
            provenance: { creator_did: 'did:plc:zap' },
          }),
          { status: 200 },
        );
      }
      if (url.includes(encodeURIComponent('did:plc:zap'))) {
        return new Response(
          JSON.stringify({ nick: 'zapnap', provenance: null }),
          { status: 200 },
        );
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetchMock);

    const { container, findByText } = render(
      <ProvenanceBlock provenance={{ creator_did: 'did:key:lobot' }} />,
    );

    // Both nicks should appear once the chain resolves.
    await findByText('lobot');
    await findByText('zapnap');

    // The arrow separator must render between them (← reads as "was
    // created by"). We assert via visible text, not class names, so the
    // test stays robust to CSS changes.
    expect(container.textContent).toContain('←');
  });

  it('copies the right DID when a chain link is clicked', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ nick: 'lobot', provenance: null }), {
        status: 200,
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const { findByText } = render(
      <ProvenanceBlock provenance={{ creator_did: 'did:key:lobot' }} />,
    );

    const lobotButton = await findByText('lobot');
    fireEvent.click(lobotButton);

    // Clipboard receives the *DID* of the clicked link, not its label.
    await waitFor(() => {
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith('did:key:lobot');
    });
  });

  it('renders nothing when there is no creator_did', () => {
    // ProvenanceBlock should still render its frame (so other
    // provenance fields like source_repo/impl could show), but the
    // Creator line must be absent. We assert by checking that no
    // chain-link button gets rendered.
    const { container } = render(<ProvenanceBlock provenance={{}} />);
    // No button means no Creator line — chain is empty.
    expect(container.querySelectorAll('button').length).toBe(0);
  });
});
