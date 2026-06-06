import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { getForks, getLineage, getTopForks, recordFork, ingestPersonaRecord } from './forks';

function mockFetch(body: unknown, ok = true, status = 200) {
  const fn = vi.fn(async () => ({
    ok,
    status,
    json: async () => body,
  })) as unknown as typeof fetch;
  vi.stubGlobal('fetch', fn);
  return fn as unknown as ReturnType<typeof vi.fn>;
}

afterEach(() => vi.unstubAllGlobals());

describe('forks API client', () => {
  it('getForks uses the ?id= query form and url-encodes at:// ids', async () => {
    const fetchMock = mockFetch({ kind: 'persona', id: 'x', fork_count: 2, forks: [], forked_from: null });
    await getForks('persona', 'at://did:plc:o/at.freeq.persona/p1');
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('/api/v1/forks/persona?id=at%3A%2F%2Fdid%3Aplc%3Ao%2Fat.freeq.persona%2Fp1');
  });

  it('getLineage hits the lineage query endpoint', async () => {
    const fetchMock = mockFetch({ kind: 'persona', id: 'x', depth: 0, root: 'x', lineage: [] });
    const r = await getLineage('persona', 'eliza');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/forks'.replace('forks', 'lineage') + '/persona?id=eliza');
    expect(r.depth).toBe(0);
  });

  it('recordFork POSTs the right body', async () => {
    const fetchMock = mockFetch({ fork: {}, already_recorded: false });
    await recordFork('persona', 'eliza', 'cassandra', { forkedBy: 'did:plc:me', note: 'darker' });
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/forks');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({
      kind: 'persona', parent_id: 'eliza', child_id: 'cassandra',
      forked_by: 'did:plc:me', note: 'darker',
    });
  });

  it('ingestPersonaRecord POSTs uri + record', async () => {
    const fetchMock = mockFetch({ ingested: true, uri: 'u', name: 'n', fork_recorded: true });
    const rec = {
      $type: 'at.freeq.persona' as const, name: 'n', systemPrompt: 'p', createdAt: 't',
    };
    await ingestPersonaRecord('at://did:plc:me/at.freeq.persona/c1', rec);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/personas/record');
    expect(JSON.parse(init.body as string).record.name).toBe('n');
  });

  it('getTopForks hits the leaderboard endpoint', async () => {
    const fetchMock = mockFetch({ kind: 'persona', top: [{ id: 'eliza', fork_count: 5 }] });
    const r = await getTopForks('persona', 10);
    expect(fetchMock.mock.calls[0][0]).toBe('/api/v1/forks/top?kind=persona&limit=10');
    expect(r.top[0].fork_count).toBe(5);
  });

  it('throws on a non-ok response', async () => {
    mockFetch({}, false, 500);
    await expect(getForks('persona', 'x')).rejects.toThrow(/getForks failed: 500/);
  });
});
