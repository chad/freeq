import { useState } from 'react';
import { useStore } from '../store';
import { joinChannel, disconnect } from '../irc/client';

export function Sidebar() {
  const channels = useStore((s) => s.channels);
  const activeChannel = useStore((s) => s.activeChannel);
  const setActive = useStore((s) => s.setActiveChannel);
  const serverMessages = useStore((s) => s.serverMessages);
  const connectionState = useStore((s) => s.connectionState);
  const nick = useStore((s) => s.nick);
  const authDid = useStore((s) => s.authDid);
  const [joinInput, setJoinInput] = useState('');
  const [showJoin, setShowJoin] = useState(false);

  const sorted = [...channels.values()]
    .filter((ch) => ch.isJoined)
    .sort((a, b) => a.name.localeCompare(b.name));

  const handleJoin = () => {
    const ch = joinInput.trim();
    if (ch) {
      joinChannel(ch.startsWith('#') ? ch : `#${ch}`);
      setJoinInput('');
      setShowJoin(false);
    }
  };

  return (
    <aside className="w-60 bg-bg-secondary flex flex-col shrink-0 overflow-hidden">
      {/* Brand */}
      <div className="h-12 flex items-center px-4 border-b border-border shrink-0">
        <span className="text-accent font-bold text-lg tracking-tight">freeq</span>
        <span className={`ml-auto w-2 h-2 rounded-full ${
          connectionState === 'connected' ? 'bg-success' :
          connectionState === 'connecting' ? 'bg-warning animate-pulse' : 'bg-danger'
        }`} />
      </div>

      <nav className="flex-1 overflow-y-auto py-2 px-2">
        {/* Server */}
        <button
          onClick={() => setActive('server')}
          className={`w-full text-left px-3 py-1.5 rounded-lg text-sm flex items-center gap-2 mb-1 ${
            activeChannel === 'server'
              ? 'bg-surface text-fg'
              : 'text-fg-dim hover:text-fg-muted hover:bg-bg-tertiary'
          }`}
        >
          <svg className="w-4 h-4 shrink-0 opacity-60" viewBox="0 0 16 16" fill="currentColor">
            <path d="M1.5 3A1.5 1.5 0 013 1.5h10A1.5 1.5 0 0114.5 3v2A1.5 1.5 0 0113 6.5H3A1.5 1.5 0 011.5 5V3zm1 .5v1.5h11V3.5h-11zM1.5 9A1.5 1.5 0 013 7.5h10A1.5 1.5 0 0114.5 9v2a1.5 1.5 0 01-1.5 1.5H3A1.5 1.5 0 011.5 11V9zm1 .5v1.5h11V9.5h-11z"/>
          </svg>
          <span>Server</span>
          {serverMessages.length > 0 && activeChannel !== 'server' && (
            <span className="ml-auto w-1.5 h-1.5 rounded-full bg-fg-dim" />
          )}
        </button>

        {/* Channels */}
        <div className="mt-3 mb-1 px-2 flex items-center justify-between">
          <span className="text-[10px] uppercase tracking-widest text-fg-dim font-semibold">
            Channels
          </span>
          <button
            onClick={() => setShowJoin(!showJoin)}
            className="text-fg-dim hover:text-fg-muted text-lg leading-none px-1"
            title="Join channel"
          >
            +
          </button>
        </div>

        {showJoin && (
          <div className="px-1 mb-2 animate-fadeIn">
            <input
              value={joinInput}
              onChange={(e) => setJoinInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleJoin()}
              placeholder="#channel"
              autoFocus
              className="w-full bg-bg-tertiary border border-border rounded px-2 py-1 text-sm text-fg outline-none focus:border-accent placeholder:text-fg-dim"
            />
          </div>
        )}

        {sorted.map((ch) => {
          const isActive = activeChannel.toLowerCase() === ch.name.toLowerCase();
          const hasMention = ch.mentionCount > 0;
          const hasUnread = ch.unreadCount > 0;
          return (
            <button
              key={ch.name}
              onClick={() => setActive(ch.name)}
              className={`w-full text-left px-3 py-1.5 rounded-lg text-sm flex items-center gap-2 ${
                isActive
                  ? 'bg-surface text-fg'
                  : hasMention
                    ? 'text-fg font-semibold hover:bg-bg-tertiary'
                    : hasUnread
                      ? 'text-fg-muted hover:bg-bg-tertiary'
                      : 'text-fg-dim hover:text-fg-muted hover:bg-bg-tertiary'
              }`}
            >
              <span className={`shrink-0 text-xs ${isActive ? 'text-accent' : 'opacity-50'}`}>#</span>
              <span className="truncate">{ch.name.replace(/^#/, '')}</span>
              {hasMention && (
                <span className="ml-auto shrink-0 bg-danger text-white text-[10px] min-w-[18px] text-center px-1 py-0.5 rounded-full font-bold">
                  {ch.mentionCount}
                </span>
              )}
              {!hasMention && hasUnread && (
                <span className="ml-auto shrink-0 w-1.5 h-1.5 rounded-full bg-fg-muted" />
              )}
            </button>
          );
        })}
      </nav>

      {/* User footer */}
      <div className="border-t border-border px-3 py-2.5 shrink-0">
        <div className="flex items-center gap-2">
          <div className="w-8 h-8 rounded-full bg-surface flex items-center justify-center text-accent font-bold text-sm shrink-0">
            {(nick || '?')[0].toUpperCase()}
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium truncate">{nick}</div>
            {authDid && (
              <div className="text-[10px] text-fg-dim truncate" title={authDid}>
                {authDid}
              </div>
            )}
          </div>
          <button
            onClick={disconnect}
            className="text-fg-dim hover:text-danger text-xs p-1"
            title="Disconnect"
          >
            ✕
          </button>
        </div>
      </div>
    </aside>
  );
}
