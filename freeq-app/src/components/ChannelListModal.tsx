import { useRef, useEffect, useState, useMemo } from 'react';
import { useStore } from '../store';
import { joinChannel, rawCommand } from '../irc/client';

export function ChannelListModal() {
  const open = useStore((s) => s.channelListOpen);
  const list = useStore((s) => s.channelList);
  const setOpen = useStore((s) => s.setChannelListOpen);
  const [filter, setFilter] = useState('');
  const [confirmCreate, setConfirmCreate] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      rawCommand('LIST');
      setFilter('');
      setConfirmCreate(null);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open]);

  const filtered = useMemo(() => {
    if (!filter) return list;
    const q = filter.toLowerCase();
    return list.filter((ch) =>
      ch.name.toLowerCase().includes(q) || ch.topic.toLowerCase().includes(q)
    );
  }, [list, filter]);

  // Derive the channel name from filter for "create" suggestion
  const createName = useMemo(() => {
    if (!filter.trim()) return '';
    const f = filter.trim();
    return f.startsWith('#') ? f : `#${f}`;
  }, [filter]);

  // Show create suggestion when filter doesn't match any channel exactly
  const showCreateSuggestion = filter.trim().length > 0 &&
    !list.some((ch) => ch.name.toLowerCase() === createName.toLowerCase());

  const doCreate = (name: string) => {
    joinChannel(name);
    setOpen(false);
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[150] flex items-start justify-center pt-[10vh]" onClick={() => setOpen(false)}>
      <div
        className="bg-bg-secondary border border-border rounded-xl shadow-2xl w-[560px] max-w-[90vw] max-h-[70vh] flex flex-col animate-fadeIn"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
          <span className="text-accent text-sm font-semibold">Browse Channels</span>
          <span className="text-[10px] text-fg-dim bg-bg px-1.5 py-0.5 rounded-full">{list.length}</span>
          <div className="flex-1" />
          <kbd className="text-[10px] text-fg-dim bg-bg px-1.5 py-0.5 rounded border border-border">ESC</kbd>
        </div>

        {/* Filter / Search */}
        <div className="px-4 py-2 border-b border-border">
          <input
            ref={inputRef}
            value={filter}
            onChange={(e) => { setFilter(e.target.value); setConfirmCreate(null); }}
            placeholder="Search or create a channel..."
            className="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-fg outline-none focus:border-accent placeholder:text-fg-dim"
            onKeyDown={(e) => {
              if (e.key === 'Escape') setOpen(false);
              if (e.key === 'Enter' && showCreateSuggestion && !confirmCreate) {
                setConfirmCreate(createName);
              } else if (e.key === 'Enter' && confirmCreate) {
                doCreate(confirmCreate);
              } else if (e.key === 'Enter' && filtered.length === 1) {
                joinChannel(filtered[0].name);
                setOpen(false);
              }
            }}
          />
        </div>

        {/* Confirm create */}
        {confirmCreate && (
          <div className="px-4 py-3 border-b border-border bg-accent/[0.03] flex items-center gap-3 animate-fadeIn">
            <div className="flex-1">
              <div className="text-sm text-fg">
                Create <span className="font-semibold text-accent">{confirmCreate}</span>?
              </div>
              <div className="text-xs text-fg-dim mt-0.5">
                This channel doesn't exist yet. You'll be the founder.
              </div>
            </div>
            <button
              onClick={() => setConfirmCreate(null)}
              className="text-xs text-fg-dim hover:text-fg-muted px-2 py-1"
            >
              Cancel
            </button>
            <button
              onClick={() => doCreate(confirmCreate)}
              className="bg-accent text-black text-xs font-bold px-3 py-1.5 rounded-lg hover:bg-accent-hover"
            >
              Create
            </button>
          </div>
        )}

        {/* Channel list */}
        <div className="flex-1 overflow-y-auto">
          {/* Create suggestion when no exact match */}
          {showCreateSuggestion && !confirmCreate && (
            <button
              onClick={() => setConfirmCreate(createName)}
              className="w-full text-left px-4 py-3 hover:bg-bg-tertiary border-b border-border/50 flex items-center gap-2"
            >
              <span className="w-7 h-7 rounded-lg bg-accent/10 flex items-center justify-center text-accent text-sm font-bold">+</span>
              <div>
                <span className="text-sm font-semibold text-fg">Create {createName}</span>
                <span className="text-xs text-fg-dim ml-2">New channel</span>
              </div>
            </button>
          )}

          {filtered.length === 0 && !showCreateSuggestion ? (
            <div className="text-center text-fg-dim text-sm py-8">
              {list.length === 0 ? 'Loading channels...' : 'No channels match'}
            </div>
          ) : (
            filtered.map((ch) => (
              <button
                key={ch.name}
                onClick={() => {
                  joinChannel(ch.name);
                  setOpen(false);
                }}
                className="w-full text-left px-4 py-3 hover:bg-bg-tertiary border-b border-border/50 last:border-0"
              >
                <div className="flex items-center gap-2">
                  <span className="text-sm font-semibold text-fg">{ch.name}</span>
                  <span className="text-[10px] text-fg-dim">{ch.count} members</span>
                </div>
                {ch.topic && (
                  <div className="text-xs text-fg-muted mt-0.5 truncate">{ch.topic}</div>
                )}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
