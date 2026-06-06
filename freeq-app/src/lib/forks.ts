// Client for the server's fork-lineage graph + persona-record ingest.
// Thin wrappers over the REST API (relative same-origin /api/v1).

import type { PersonaRecord } from './personaRecord';

export type ForkKind = 'persona' | 'character' | 'agent';

export interface ForkRow {
  fork_id: string;
  kind: string;
  parent_id: string;
  child_id: string;
  forked_by: string | null;
  forked_at: number;
  note: string | null;
}

export interface ForksResponse {
  kind: string;
  id: string;
  fork_count: number;
  forks: ForkRow[];
  forked_from: ForkRow | null;
}

export interface LineageResponse {
  kind: string;
  id: string;
  depth: number;
  root: string;
  lineage: ForkRow[];
}

export interface ForkLeader {
  id: string;
  fork_count: number;
}

/** Most-forked artifacts of a kind — the discovery leaderboard. */
export async function getTopForks(kind: ForkKind, limit = 20): Promise<{ kind: string; top: ForkLeader[] }> {
  const resp = await fetch(`/api/v1/forks/top?kind=${kind}&limit=${limit}`);
  if (!resp.ok) throw new Error(`getTopForks failed: ${resp.status}`);
  return resp.json();
}

/** Direct forks of `id` + count + what it was forked from. */
export async function getForks(kind: ForkKind, id: string): Promise<ForksResponse> {
  // Query form (not path) so at:// ids — which contain slashes — work.
  const resp = await fetch(`/api/v1/forks/${kind}?id=${encodeURIComponent(id)}`);
  if (!resp.ok) throw new Error(`getForks failed: ${resp.status}`);
  return resp.json();
}

/** Ancestor chain from `id`'s nearest parent up to the root. */
export async function getLineage(kind: ForkKind, id: string): Promise<LineageResponse> {
  const resp = await fetch(`/api/v1/lineage/${kind}?id=${encodeURIComponent(id)}`);
  if (!resp.ok) throw new Error(`getLineage failed: ${resp.status}`);
  return resp.json();
}

/** Record a fork edge directly (parent → child). */
export async function recordFork(
  kind: ForkKind,
  parentId: string,
  childId: string,
  opts?: { forkedBy?: string; note?: string },
): Promise<{ fork: ForkRow; parent_fork_count?: number; already_recorded: boolean }> {
  const resp = await fetch('/api/v1/forks', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      kind,
      parent_id: parentId,
      child_id: childId,
      forked_by: opts?.forkedBy,
      note: opts?.note,
    }),
  });
  if (!resp.ok) throw new Error(`recordFork failed: ${resp.status}`);
  return resp.json();
}

/**
 * Ingest a persona record (as it lives in a PDS) so its `forkedFrom`
 * edge is folded into the graph. `uri` is the record's at:// URI.
 */
export async function ingestPersonaRecord(
  uri: string,
  record: PersonaRecord,
): Promise<{ ingested: boolean; uri: string; name: string; fork_recorded: boolean }> {
  const resp = await fetch('/api/v1/personas/record', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ uri, record }),
  });
  if (!resp.ok) throw new Error(`ingestPersonaRecord failed: ${resp.status}`);
  return resp.json();
}
