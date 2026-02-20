import { useRef, useEffect, useState, useMemo } from 'react';
import { useStore } from '../store';
import { joinChannel, rawCommand } from '../irc/client';

export function ChannelListModal() {
  const open = useStore((s) => s.channelListOpen);
  const list = useStore((s) => s.channelList);
  const setOpen = useStore((s) => s.setChannelListOpen);
  const [filter, setFilter] = useState('');
  const [createMode, setCreateMode] = useState(false);
  const [newChan, setNewChan] = useState('#');
  const inputRef = useRef<HTMLInputElement>(null);
  const createRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      // Request channel list from server
      rawCommand('LIST');
      setFilter('');
      setCreateMode(false);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open]);

  useEffect(() => {
    if (createMode) createRef.current?.focus();
  }, [createMode]);

  const filtered = useMemo(() => {
    if (!filter) return list;
    const q = filter.toLowerCase();
    return list.filter((ch) =>
      ch.name.toLowerCase().includes(q) || ch.topic.toLowerCase().includes(q)
    );
  }, [list, filter]);

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
          <button
            onClick={() => setCreateMode(!createMode)}
            className="text-xs text-accent hover:text-accent-hover font-medium"
          >
            + Create
          </button>
        </div>

        {/* Create channel */}
        {createMode && (
          <div className="px-4 py-2 border-b border-border flex gap-2 animate-fadeIn">
            <input
              ref={createRef}
              value={newChan}
              onChange={(e) => setNewChan(e.target.value)}
              placeholder="#new-channel"
              className="flex-1 bg-bg border border-border rounded-lg px-3 py-1.5 text-sm text-fg outline-none focus:border-accent"
              onKeyDown={(e) => {
                if (e.key === 'Enter' && newChan.trim()) {
                  const name = newChan.startsWith('#') ? newChan : `#${newChan}`;
                  joinChannel(name);
                  setOpen(false);
                }
                if (e.key === 'Escape') setCreateMode(false);
              }}
            />
            <button
              onClick={() => {
                const name = newChan.startsWith('#') ? newChan : `#${newChan}`;
                joinChannel(name);
                setOpen(false);
              }}
              className="bg-accent text-black text-xs font-bold px-3 py-1.5 rounded-lg hover:bg-accent-hover"
            >
              Create
            </button>
          </div>
        )}

        {/* Filter */}
        <div className="px-4 py-2 border-b border-border">
          <input
            ref={inputRef}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter channels..."
            className="w-full bg-bg border border-border rounded-lg px-3 py-1.5 text-sm text-fg outline-none focus:border-accent"
            onKeyDown={(e) => {
              if (e.key === 'Escape') setOpen(false);
            }}
          />
        </div>

        {/* Channel list */}
        <div className="flex-1 overflow-y-auto">
          {filtered.length === 0 ? (
            <div className="text-center text-fg-dim text-sm py-8">
              {list.length === 0 ? 'Loading channels...' : 'No channels match filter'}
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
