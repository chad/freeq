// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor, fireEvent } from '@testing-library/react';
import { shortId, ForkGraph } from './ForkGraph';
import { useStore } from '../store';
import * as forks from '../lib/forks';

vi.mock('../lib/forks', () => ({
  getForks: vi.fn(),
  getLineage: vi.fn(),
}));

describe('shortId', () => {
  it('returns the tail of an at:// URI', () => {
    expect(shortId('at://did:plc:abc/at.freeq.persona/3kxyz')).toBe('3kxyz');
    expect(shortId('eliza')).toBe('eliza');
  });
});

describe('<ForkGraph>', () => {
  beforeEach(() => {
    useStore.setState({ forkGraphOpen: true, forkGraphKind: 'persona', forkGraphId: 'eliza' });
    vi.mocked(forks.getForks).mockResolvedValue({
      kind: 'persona', id: 'eliza', fork_count: 2,
      forks: [
        { fork_id: 'f1', kind: 'persona', parent_id: 'eliza', child_id: 'oblivion', forked_by: 'did:plc:a', forked_at: 1, note: null },
        { fork_id: 'f2', kind: 'persona', parent_id: 'eliza', child_id: 'utopia', forked_by: null, forked_at: 2, note: null },
      ],
      forked_from: null,
    });
    vi.mocked(forks.getLineage).mockResolvedValue({ kind: 'persona', id: 'eliza', depth: 0, root: 'eliza', lineage: [] });
  });
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it('shows the fork count and direct forks, and recenters on click', async () => {
    const { getByText, getByTitle } = render(<ForkGraph />);
    await waitFor(() => expect(getByText('2 forks')).toBeTruthy());
    expect(getByText('oblivion')).toBeTruthy();
    expect(getByText('utopia')).toBeTruthy();

    // Clicking a fork recenters the graph on that child.
    fireEvent.click(getByTitle('oblivion'));
    expect(useStore.getState().forkGraphId).toBe('oblivion');
  });

  it('"Fork this" opens the Studio seeded with the focused id as parent', async () => {
    const { getByText } = render(<ForkGraph />);
    await waitFor(() => expect(getByText('Fork this →')).toBeTruthy());
    fireEvent.click(getByText('Fork this →'));
    const s = useStore.getState();
    expect(s.studioForkFrom).toBe('eliza');
    expect(s.personaStudioOpen).toBe(true);
    expect(s.forkGraphOpen).toBe(false);
  });

  it('renders nothing when closed', () => {
    useStore.setState({ forkGraphOpen: false });
    const { container } = render(<ForkGraph />);
    expect(container.firstChild).toBeNull();
  });
});
