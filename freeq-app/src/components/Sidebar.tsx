import { useStore } from '../store';

export function Sidebar() {
  const channels = useStore((s) => s.channels);
  const activeChannel = useStore((s) => s.activeChannel);
  const setActive = useStore((s) => s.setActiveChannel);
  const serverMessages = useStore((s) => s.serverMessages);

  const sorted = [...channels.values()]
    .filter((ch) => ch.isJoined)
    .sort((a, b) => a.name.localeCompare(b.name));

  return (
    <aside className="w-56 bg-bg-secondary border-r border-border flex flex-col shrink-0 overflow-y-auto">
      {/* Server buffer */}
      <div className="px-3 pt-3 pb-1">
        <div className="text-[10px] uppercase tracking-wider text-fg-dim font-semibold mb-1">
          Status
        </div>
        <button
          onClick={() => setActive('server')}
          className={`w-full text-left px-2 py-1 rounded text-sm truncate ${
            activeChannel === 'server'
              ? 'bg-bg-tertiary text-accent'
              : 'text-fg-muted hover:bg-bg-tertiary/50'
          }`}
        >
          (server)
          {serverMessages.length > 0 && activeChannel !== 'server' && (
            <span className="ml-auto text-[10px] text-fg-dim"> •</span>
          )}
        </button>
      </div>

      {/* Channels */}
      <div className="px-3 pt-3 pb-1">
        <div className="text-[10px] uppercase tracking-wider text-fg-dim font-semibold mb-1">
          Channels
        </div>
        {sorted.map((ch) => (
          <button
            key={ch.name}
            onClick={() => setActive(ch.name)}
            className={`w-full text-left px-2 py-1 rounded text-sm truncate flex items-center gap-1 ${
              activeChannel.toLowerCase() === ch.name.toLowerCase()
                ? 'bg-bg-tertiary text-accent'
                : ch.mentionCount > 0
                  ? 'text-danger font-semibold'
                  : 'text-fg-muted hover:bg-bg-tertiary/50'
            }`}
          >
            <span className="truncate">{ch.name}</span>
            {ch.mentionCount > 0 && (
              <span className="ml-auto shrink-0 bg-danger text-white text-[10px] px-1.5 py-0.5 rounded-full font-bold">
                {ch.mentionCount}
              </span>
            )}
            {ch.mentionCount === 0 && ch.unreadCount > 0 && (
              <span className="ml-auto shrink-0 bg-surface text-fg-dim text-[10px] px-1.5 py-0.5 rounded-full">
                {ch.unreadCount}
              </span>
            )}
          </button>
        ))}
      </div>
    </aside>
  );
}
