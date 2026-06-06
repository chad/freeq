// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor, fireEvent } from '@testing-library/react';
import { PersonaStudio } from './PersonaStudio';
import { useStore } from '../store';
import * as forks from '../lib/forks';

vi.mock('../lib/forks', () => ({
  getLineage: vi.fn(),
  ingestPersonaRecord: vi.fn(),
}));

describe('<PersonaStudio> register flow', () => {
  beforeEach(() => {
    useStore.setState({ personaStudioOpen: true, studioForkFrom: null });
    vi.mocked(forks.ingestPersonaRecord).mockResolvedValue({
      ingested: true, uri: 'at://did:plc:you/at.freeq.persona/c1', name: 'Cassandra', fork_recorded: true,
    });
  });
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it('ingests a published record and reports the fork edge', async () => {
    const { getByPlaceholderText, getByText } = render(<PersonaStudio />);
    fireEvent.change(getByPlaceholderText('Cassandra'), { target: { value: 'Cassandra' } });
    fireEvent.change(getByPlaceholderText(/You are Cassandra/), { target: { value: 'You foresee.' } });
    fireEvent.change(getByPlaceholderText(/at:\/\/did:plc:you/), {
      target: { value: 'at://did:plc:you/at.freeq.persona/c1' },
    });
    fireEvent.click(getByText('Register'));

    await waitFor(() => expect(forks.ingestPersonaRecord).toHaveBeenCalled());
    const [uri, record] = vi.mocked(forks.ingestPersonaRecord).mock.calls[0];
    expect(uri).toBe('at://did:plc:you/at.freeq.persona/c1');
    expect(record.name).toBe('Cassandra');
    await waitFor(() => expect(getByText(/fork edge recorded/)).toBeTruthy());
  });

  it('pre-fills forkedFrom when opened via "Fork this"', async () => {
    cleanup();
    useStore.setState({ personaStudioOpen: true, studioForkFrom: 'at://did:plc:o/at.freeq.persona/p1' });
    const { getByDisplayValue } = render(<PersonaStudio />);
    await waitFor(() => expect(getByDisplayValue('at://did:plc:o/at.freeq.persona/p1')).toBeTruthy());
    // The store flag is consumed (cleared) after seeding.
    expect(useStore.getState().studioForkFrom).toBeNull();
  });
});
