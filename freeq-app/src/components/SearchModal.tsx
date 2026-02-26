import React, { useRef, useEffect, useMemo } from 'react';
import { useStore, type Message } from '../store';
// useStore imported above — also used imperatively for setScrollToMsgId

export function SearchModal() {
  const open = useStore((s) => s.searchOpen);
  const query = useStore((s) => s.searchQuery);
  const setQuery = useStore((s) => s.setSearchQuery);
  const setOpen = useStore((s) => s.setSearchOpen);
  const channels = useStore((s) => s.channels);
  const setActive = useStore((s) => s.setActiveChannel);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open]);

  const results = useMemo(() => {
    if (!query || query.length < 2) return [];
    const q = query.toLowerCase();
    const hits: { channel: string; msg: Message }[] = [];

    for (const [key, ch] of channels) {
      for (const msg of ch.messages) {
        if (msg.isSystem || msg.deleted) continue;
        if (msg.text.toLowerCase().includes(q) || msg.from.toLowerCase().includes(q)) {
          hits.push({ channel: key, msg });
          if (hits.length >= 50) break;
        }
      }
      if (hits.length >= 50) break;
    }

    return hits.sort((a, b) => b.msg.timestamp.getTime() - a.msg.timestamp.getTime());
  }, [query, channels]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[150] flex items-start justify-center pt-[10vh]" onClick={() => setOpen(false)}>
      <div
        className="bg-bg-secondary border border-border rounded-xl shadow-2xl w-[560px] max-w-[90vw] max-h-[70vh] flex flex-col animate-fadeIn"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Search input */}
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
          <svg className="w-4 h-4 text-fg-dim shrink-0" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
            <circle cx="7" cy="7" r="5" />
            <path d="M11 11l3.5 3.5" />
          </svg>
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search messages..."
            className="flex-1 bg-transparent text-sm text-fg outline-none placeholder:text-fg-dim"
            onKeyDown={(e) => {
              if (e.key === 'Escape') setOpen(false);
            }}
          />
          <kbd className="text-[10px] text-fg-dim bg-bg px-1.5 py-0.5 rounded border border-border">ESC</kbd>
        </div>

        {/* Results */}
        <div className="flex-1 overflow-y-auto">
          {query.length < 2 ? (
            <div className="text-center text-fg-dim text-sm py-8">
              Type at least 2 characters to search
            </div>
          ) : results.length === 0 ? (
            <div className="text-center text-fg-dim text-sm py-8">
              No messages found
            </div>
          ) : (
            results.map((r, i) => (
              <button
                key={`${r.msg.id}-${i}`}
                onClick={() => {
                  setActive(r.channel);
                  setOpen(false);
                  useStore.getState().setScrollToMsgId(r.msg.id);
                }}
                className="w-full text-left px-4 py-2.5 hover:bg-bg-tertiary border-b border-border/50 last:border-0"
              >
                <div className="flex items-center gap-2 mb-0.5">
                  <span className="text-xs font-semibold text-fg">{r.msg.from}</span>
                  <span className="text-[10px] text-fg-dim">
                    in {channels.get(r.channel)?.name || r.channel}
                  </span>
                  <span className="text-[10px] text-fg-dim ml-auto">
                    {r.msg.timestamp.toLocaleDateString()} {r.msg.timestamp.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                  </span>
                </div>
                <div className="text-sm text-fg-muted truncate">
                  {highlightQuery(r.msg.text, query)}
                </div>
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

function highlightQuery(text: string, query: string): React.ReactElement {
  const idx = text.toLowerCase().indexOf(query.toLowerCase());
  if (idx < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <mark className="bg-accent/20 text-accent rounded px-0.5">{text.slice(idx, idx + query.length)}</mark>
      {text.slice(idx + query.length)}
    </>
  );
}
