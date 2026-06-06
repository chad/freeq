import { useEffect, useState } from 'react';
import { useStore } from '../store';
import {
  validatePersonaForm,
  buildPersonaRecord,
  personaRecordJson,
  BUILTIN_CHARACTERS,
  type PersonaForm,
} from '../lib/personaRecord';
import { getLineage, ingestPersonaRecord, type ForkRow } from '../lib/forks';

const EMPTY: PersonaForm = {
  name: '',
  systemPrompt: '',
  greeting: '',
  voiceId: '',
  speed: 1.0,
  faceCharacter: 'eliza',
  facePack: '',
  forkedFrom: '',
};

/** No-code authoring for a forkable `at.freeq.persona` record. */
export function PersonaStudio() {
  const open = useStore((s) => s.personaStudioOpen);
  const setOpen = useStore((s) => s.setPersonaStudioOpen);
  const forkFrom = useStore((s) => s.studioForkFrom);
  const clearForkFrom = useStore((s) => s.setStudioForkFrom);
  const openForkGraph = useStore((s) => s.openForkGraph);
  const [form, setForm] = useState<PersonaForm>(EMPTY);
  const [lineage, setLineage] = useState<ForkRow[] | null>(null);
  const [copied, setCopied] = useState(false);
  const [publishUri, setPublishUri] = useState('');
  const [regResult, setRegResult] = useState<string | null>(null);

  // When opened via "Fork this" from the graph, pre-seed the parent.
  useEffect(() => {
    if (open && forkFrom) {
      setForm((f) => ({ ...f, forkedFrom: forkFrom }));
      clearForkFrom(null);
    }
  }, [open, forkFrom, clearForkFrom]);

  if (!open) return null;

  const errors = validatePersonaForm(form);
  const valid = errors.length === 0;
  const record = valid ? buildPersonaRecord(form) : null;
  const json = record ? personaRecordJson(record) : '';

  const set = (patch: Partial<PersonaForm>) => setForm((f) => ({ ...f, ...patch }));

  const copy = async () => {
    if (!json) return;
    await navigator.clipboard.writeText(json);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const download = () => {
    if (!record) return;
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${record.name.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'persona'}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const register = async () => {
    const uri = publishUri.trim();
    if (!record || !uri.startsWith('at://') || !uri.includes('/at.freeq.persona/')) {
      setRegResult('Enter the at:// URI of the persona record you published.');
      return;
    }
    try {
      const r = await ingestPersonaRecord(uri, record);
      setRegResult(
        r.fork_recorded
          ? `Registered ✓ — fork edge recorded (${r.name}).`
          : `Registered ✓ — ${r.name} (no fork edge; this is an original).`,
      );
    } catch (e) {
      setRegResult(`Failed: ${String(e)}`);
    }
  };

  const loadLineage = async () => {
    const uri = form.forkedFrom?.trim();
    if (!uri) return;
    try {
      const r = await getLineage('persona', uri);
      setLineage(r.lineage);
    } catch {
      setLineage([]);
    }
  };

  const label = 'block text-xs font-semibold text-fg-dim mb-1';
  const input =
    'w-full bg-bg-tertiary border border-border rounded-md px-2.5 py-1.5 text-sm text-fg outline-none focus:border-accent placeholder:text-fg-dim';

  return (
    <>
      <div className="fixed inset-0 z-40 bg-black/50 backdrop-blur-sm" onClick={() => setOpen(false)} />
      <div className="fixed right-0 top-0 bottom-0 z-50 w-[36rem] max-w-full bg-bg-secondary border-l border-border shadow-2xl animate-slideIn overflow-y-auto">
        <div className="p-4 border-b border-border flex items-center justify-between sticky top-0 bg-bg-secondary z-10">
          <h2 className="font-semibold flex items-center gap-2">
            <span className="text-accent">✦</span> Persona Studio
          </h2>
          <button onClick={() => setOpen(false)} className="text-fg-dim hover:text-fg text-lg">✕</button>
        </div>

        <div className="p-4 space-y-3">
          <div>
            <label className={label}>Name</label>
            <input className={input} value={form.name} placeholder="Cassandra"
              onChange={(e) => set({ name: e.target.value })} />
          </div>

          <div>
            <label className={label}>System prompt</label>
            <textarea className={`${input} h-32 resize-y font-mono text-xs`} value={form.systemPrompt}
              placeholder="You are Cassandra. You foresee and you warn…"
              onChange={(e) => set({ systemPrompt: e.target.value })} />
          </div>

          <div>
            <label className={label}>Greeting (spoken on join, optional)</label>
            <input className={input} value={form.greeting}
              placeholder="Cassandra. You won't listen, but I'll speak anyway."
              onChange={(e) => set({ greeting: e.target.value })} />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className={label}>ElevenLabs voice ID</label>
              <input className={input} value={form.voiceId} placeholder="dG7SBJDxDoZkQUrwvqrD"
                onChange={(e) => set({ voiceId: e.target.value })} />
            </div>
            <div>
              <label className={label}>Voice speed ({(form.speed ?? 1).toFixed(2)}×)</label>
              <input type="range" min={0.5} max={2} step={0.01} value={form.speed ?? 1}
                className="w-full accent-accent" onChange={(e) => set({ speed: parseFloat(e.target.value) })} />
            </div>
          </div>

          <div>
            <label className={label}>Face — ghostly character</label>
            <select className={input} value={form.faceCharacter} disabled={!!form.facePack?.trim()}
              onChange={(e) => set({ faceCharacter: e.target.value })}>
              {BUILTIN_CHARACTERS.map((c) => <option key={c} value={c}>{c}</option>)}
            </select>
          </div>
          <div>
            <label className={label}>…or custom character pack (at:// URI)</label>
            <input className={input} value={form.facePack} placeholder="at://did:plc:…/at.freeq.character/…"
              onChange={(e) => set({ facePack: e.target.value })} />
          </div>

          <div>
            <label className={label}>Forked from (parent persona at:// URI, optional)</label>
            <div className="flex gap-2">
              <input className={input} value={form.forkedFrom} placeholder="at://did:plc:…/at.freeq.persona/…"
                onChange={(e) => { set({ forkedFrom: e.target.value }); setLineage(null); }} />
              <button onClick={loadLineage} disabled={!form.forkedFrom?.trim()}
                className="shrink-0 text-xs px-2 rounded-md bg-bg-tertiary hover:bg-surface text-fg-muted disabled:opacity-40">
                Lineage
              </button>
              <button
                onClick={() => { const u = form.forkedFrom?.trim(); if (u) { openForkGraph('persona', u); setOpen(false); } }}
                disabled={!form.forkedFrom?.trim()}
                className="shrink-0 text-xs px-2 rounded-md bg-bg-tertiary hover:bg-surface text-fg-muted disabled:opacity-40">
                Graph
              </button>
            </div>
            {lineage && (
              <div className="mt-1.5 text-[11px] text-fg-dim">
                {lineage.length === 0
                  ? 'No recorded ancestry (a root, or not yet aggregated).'
                  : `Ancestry: ${lineage.map((f) => shortId(f.parent_id)).join(' → ')}`}
              </div>
            )}
          </div>

          {errors.length > 0 && (
            <ul className="text-xs text-danger list-disc pl-4 space-y-0.5">
              {errors.map((e) => <li key={e}>{e}</li>)}
            </ul>
          )}

          <div>
            <label className={label}>Record preview (at.freeq.persona)</label>
            <pre className="bg-bg-tertiary border border-border rounded-md p-2.5 text-[11px] font-mono text-fg-muted overflow-x-auto max-h-56">
              {json || '// fill in name + system prompt'}
            </pre>
          </div>

          <div className="flex items-center gap-2">
            <button onClick={copy} disabled={!valid}
              className="text-sm px-3 py-1.5 rounded-md bg-accent text-white hover:bg-accent/90 font-medium disabled:opacity-40">
              {copied ? 'Copied ✓' : 'Copy record'}
            </button>
            <button onClick={download} disabled={!valid}
              className="text-sm px-3 py-1.5 rounded-md bg-bg-tertiary hover:bg-surface text-fg-muted disabled:opacity-40">
              Download
            </button>
          </div>

          <div className="border-t border-border/50 pt-3 space-y-2">
            <label className={label}>Register a published record</label>
            <p className="text-[11px] text-fg-dim leading-relaxed">
              Publish the record above to your PDS (one-click via AT-Proto OAuth is
              coming; for now write it to your repo), then paste its <code>at://</code>
              URI here so freeq folds its lineage into the fork graph.
            </p>
            <div className="flex gap-2">
              <input className={input} value={publishUri}
                placeholder="at://did:plc:you/at.freeq.persona/…"
                onChange={(e) => { setPublishUri(e.target.value); setRegResult(null); }} />
              <button onClick={register} disabled={!valid || !publishUri.trim()}
                className="shrink-0 text-sm px-3 rounded-md bg-bg-tertiary hover:bg-surface text-fg-muted disabled:opacity-40">
                Register
              </button>
            </div>
            {regResult && <div className="text-[11px] text-fg-muted">{regResult}</div>}
          </div>
        </div>
      </div>
    </>
  );
}

function shortId(id: string): string {
  // at://did:plc:abc/at.freeq.persona/rkey → rkey (or the tail).
  const parts = id.split('/');
  return parts[parts.length - 1] || id;
}
