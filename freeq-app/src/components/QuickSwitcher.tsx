import { useState, useEffect, useRef, useMemo } from 'react';
import { useStore } from '../store';
import { joinChannel } from '../irc/client';

interface QuickSwitcherProps {
  open: boolean;
  onClose: () => void;
}

export function QuickSwitcher({ open, onClose }: QuickSwitcherProps) {
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const channels = useStore((s) => s.channels);
  const setActive = useStore((s) => s.setActiveChannel);

  // Build results list
  const results = useMemo(() => {
    const items: { type: 'channel' | 'action'; name: string; label: string }[] = [];

    // Joined channels
    for (const ch of channels.values()) {
      if (ch.isJoined) {
        items.push({ type: 'channel', name: ch.name, label: ch.name });
      }
    }

    // Server buffer
    items.push({ type: 'channel', name: 'server', label: '(server)' });

    // If query looks like a channel, offer to join it
    const q = query.trim();
    if (q && (q.startsWith('#') || q.length > 1)) {
      const chanName = q.startsWith('#') ? q : `#${q}`;
      const exists = [...channels.values()].some(
        (ch) => ch.name.toLowerCase() === chanName.toLowerCase()
      );
      if (!exists) {
        items.push({ type: 'action', name: chanName, label: `Join ${chanName}` });
      }
    }

    // Filter by query
    if (!q) return items;
    const lower = q.toLowerCase();
    return items.filter((item) => item.label.toLowerCase().includes(lower));
  }, [channels, query]);

  useEffect(() => {
    if (open) {
      setQuery('');
      setSelected(0);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open]);

  useEffect(() => {
    setSelected(0);
  }, [query]);

  const execute = (index: number) => {
    const item = results[index];
    if (!item) return;
    if (item.type === 'action') {
      joinChannel(item.name);
    } else {
      setActive(item.name);
    }
    onClose();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') { onClose(); return; }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, results.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      execute(selected);
    }
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[15vh]" onClick={onClose}>
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />

      {/* Modal */}
      <div
        className="relative bg-bg-secondary border border-border rounded-xl w-[480px] max-w-[90vw] shadow-2xl animate-fadeIn overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 px-4 border-b border-border">
          <svg className="w-4 h-4 text-fg-dim shrink-0" viewBox="0 0 16 16" fill="currentColor">
            <path d="M11.742 10.344a6.5 6.5 0 10-1.397 1.398h-.001l3.85 3.85a1 1 0 001.415-1.414l-3.85-3.85-.017.016zm-5.242.156a5 5 0 110-10 5 5 0 010 10z"/>
          </svg>
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Switch to channel..."
            className="flex-1 bg-transparent py-3 text-sm text-fg outline-none placeholder:text-fg-dim"
          />
          <kbd className="text-[10px] text-fg-dim bg-bg-tertiary px-1.5 py-0.5 rounded font-mono">esc</kbd>
        </div>

        <div className="max-h-[300px] overflow-y-auto py-1">
          {results.length === 0 && (
            <div className="px-4 py-6 text-fg-dim text-sm text-center">No results</div>
          )}
          {results.map((item, i) => (
            <button
              key={item.name + item.type}
              onClick={() => execute(i)}
              onMouseEnter={() => setSelected(i)}
              className={`w-full text-left px-4 py-2 flex items-center gap-2 text-sm ${
                i === selected ? 'bg-bg-tertiary text-fg' : 'text-fg-muted hover:bg-bg-tertiary/50'
              }`}
            >
              {item.type === 'channel' ? (
                <span className="text-accent text-xs">#</span>
              ) : (
                <span className="text-success text-xs">+</span>
              )}
              <span>{item.label}</span>
              {item.type === 'action' && (
                <span className="ml-auto text-[10px] text-fg-dim">join</span>
              )}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
