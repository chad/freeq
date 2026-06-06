import { useEffect, useState } from 'react';
import { useStore } from '../store';
import { getForks, getLineage, getTopForks, type ForkRow, type ForkKind, type ForksResponse, type ForkLeader } from '../lib/forks';

/** Tail segment of an at:// URI (or the id itself), for compact display. */
export function shortId(id: string): string {
  const parts = id.split('/').filter(Boolean);
  return parts[parts.length - 1] || id;
}

/**
 * Browsable fork graph: lineage (ancestors) → the focused node → its
 * direct forks, with counts. Click any node to recenter; "Fork this"
 * jumps into Persona Studio pre-seeded with this node as the parent.
 */
export function ForkGraph() {
  const open = useStore((s) => s.forkGraphOpen);
  const setOpen = useStore((s) => s.setForkGraphOpen);
  const kind = useStore((s) => s.forkGraphKind) as ForkKind;
  const focusId = useStore((s) => s.forkGraphId);
  const recenter = useStore((s) => s.openForkGraph);
  const openStudio = useStore((s) => s.setPersonaStudioOpen);
  const setForkFrom = useStore((s) => s.setStudioForkFrom);

  const [forks, setForks] = useState<ForksResponse | null>(null);
  const [lineage, setLineage] = useState<ForkRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [input, setInput] = useState('');
  const [top, setTop] = useState<ForkLeader[]>([]);

  // Discovery: load the leaderboard whenever nothing is focused.
  useEffect(() => {
    if (!open || focusId.trim()) return;
    let cancelled = false;
    getTopForks(kind, 20)
      .then((r) => !cancelled && setTop(r.top))
      .catch(() => !cancelled && setTop([]));
    return () => { cancelled = true; };
  }, [open, focusId, kind]);

  useEffect(() => {
    if (!open) return;
    const id = focusId.trim();
    if (!id) {
      setForks(null);
      setLineage([]);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setErr(null);
    Promise.all([getForks(kind, id), getLineage(kind, id)])
      .then(([f, l]) => {
        if (cancelled) return;
        setForks(f);
        setLineage(l.lineage);
      })
      .catch((e) => !cancelled && setErr(String(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [open, focusId, kind]);

  if (!open) return null;

  const id = focusId.trim();
  // Ancestors come back nearest-first; reverse to read root → parent.
  const ancestors = [...lineage].reverse().map((f) => f.parent_id);
  const forkThis = () => {
    setForkFrom(id);
    openStudio(true);
    setOpen(false);
  };

  return (
    <>
      <div className="fixed inset-0 z-40 bg-black/50 backdrop-blur-sm" onClick={() => setOpen(false)} />
      <div className="fixed right-0 top-0 bottom-0 z-50 w-[34rem] max-w-full bg-bg-secondary border-l border-border shadow-2xl animate-slideIn overflow-y-auto">
        <div className="p-4 border-b border-border flex items-center justify-between sticky top-0 bg-bg-secondary z-10">
          <h2 className="font-semibold flex items-center gap-2">
            <span className="text-accent">⑂</span> Fork graph
          </h2>
          <button onClick={() => setOpen(false)} className="text-fg-dim hover:text-fg text-lg">✕</button>
        </div>

        <div className="p-4 space-y-4">
          {/* Look up any persona/character by id or at:// URI. */}
          <form
            onSubmit={(e) => { e.preventDefault(); if (input.trim()) recenter(kind, input.trim()); }}
            className="flex gap-2"
          >
            <input
              className="flex-1 bg-bg-tertiary border border-border rounded-md px-2.5 py-1.5 text-sm text-fg outline-none focus:border-accent placeholder:text-fg-dim"
              placeholder="persona id or at:// URI"
              value={input}
              onChange={(e) => setInput(e.target.value)}
            />
            <button type="submit" className="text-sm px-3 rounded-md bg-bg-tertiary hover:bg-surface text-fg-muted">
              Look up
            </button>
          </form>

          {!id && (
            <div className="space-y-3">
              <div className="text-sm text-fg-dim">Enter a persona id or paste an at:// URI — or pick from the most-forked below.</div>
              <div>
                <div className="text-xs font-semibold text-fg-dim mb-1.5">🔥 Most forked</div>
                {top.length > 0 ? (
                  <ul className="space-y-1">
                    {top.map((t, i) => (
                      <li key={t.id}>
                        <button
                          onClick={() => recenter(kind, t.id)}
                          className="w-full text-left px-2.5 py-1.5 rounded-md hover:bg-bg-tertiary flex items-center justify-between group"
                          title={t.id}
                        >
                          <span className="truncate text-sm text-fg-muted group-hover:text-fg">
                            <span className="text-fg-dim mr-1.5">{i + 1}.</span>{shortId(t.id)}
                          </span>
                          <span className="shrink-0 ml-2 text-[10px] px-1.5 py-0.5 rounded-full bg-accent/15 text-accent font-semibold">
                            {t.fork_count}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <div className="text-sm text-fg-dim">No forks recorded yet.</div>
                )}
              </div>
            </div>
          )}
          {loading && <div className="text-sm text-fg-dim">Loading…</div>}
          {err && <div className="text-sm text-danger">{err}</div>}

          {id && !loading && !err && (
            <>
              {/* Lineage breadcrumb: root → … → parent → THIS */}
              {ancestors.length > 0 && (
                <div className="text-[11px] text-fg-dim flex flex-wrap items-center gap-1">
                  {ancestors.map((a) => (
                    <span key={a} className="contents">
                      <button onClick={() => recenter(kind, a)} className="hover:text-accent underline-offset-2 hover:underline" title={a}>
                        {shortId(a)}
                      </button>
                      <span>→</span>
                    </span>
                  ))}
                  <span className="text-fg font-medium">{shortId(id)}</span>
                </div>
              )}

              {/* Focused node */}
              <div className="rounded-lg border border-border bg-bg-tertiary/50 p-3">
                <div className="flex items-center justify-between">
                  <div className="min-w-0">
                    <div className="font-medium text-fg truncate" title={id}>{shortId(id)}</div>
                    <div className="text-[11px] text-fg-dim truncate">{id}</div>
                  </div>
                  <span className="shrink-0 ml-2 text-xs px-2 py-0.5 rounded-full bg-accent/15 text-accent font-semibold">
                    {forks?.fork_count ?? 0} {forks?.fork_count === 1 ? 'fork' : 'forks'}
                  </span>
                </div>
                {forks?.forked_from && (
                  <button
                    onClick={() => recenter(kind, forks.forked_from!.parent_id)}
                    className="mt-1.5 text-[11px] text-fg-dim hover:text-accent"
                    title={forks.forked_from.parent_id}
                  >
                    ↑ forked from {shortId(forks.forked_from.parent_id)}
                  </button>
                )}
                <div className="mt-2">
                  <button onClick={forkThis} className="text-xs px-2.5 py-1 rounded-md bg-accent text-white hover:bg-accent/90 font-medium">
                    Fork this →
                  </button>
                </div>
              </div>

              {/* Direct forks (children) */}
              <div>
                <div className="text-xs font-semibold text-fg-dim mb-1.5">
                  Forks ({forks?.forks.length ?? 0})
                </div>
                {forks && forks.forks.length > 0 ? (
                  <ul className="space-y-1">
                    {forks.forks.map((f) => (
                      <li key={f.fork_id}>
                        <button
                          onClick={() => recenter(kind, f.child_id)}
                          className="w-full text-left px-2.5 py-1.5 rounded-md hover:bg-bg-tertiary flex items-center justify-between group"
                          title={f.child_id}
                        >
                          <span className="truncate text-sm text-fg-muted group-hover:text-fg">{shortId(f.child_id)}</span>
                          {f.forked_by && <span className="shrink-0 ml-2 text-[10px] text-fg-dim">{shortId(f.forked_by)}</span>}
                        </button>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <div className="text-sm text-fg-dim">No forks yet — be the first.</div>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}
