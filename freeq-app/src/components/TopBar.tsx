import { useStore } from '../store';

export function TopBar() {
  const connectionState = useStore((s) => s.connectionState);
  const nick = useStore((s) => s.nick);
  const authDid = useStore((s) => s.authDid);
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);

  const ch = channels.get(activeChannel.toLowerCase());
  const topic = ch?.topic || '';

  return (
    <header className="h-10 bg-bg-secondary border-b border-border flex items-center gap-3 px-4 shrink-0">
      <span className="font-semibold text-sm text-fg">
        {activeChannel === 'server' ? 'Server' : ch?.name || activeChannel}
      </span>

      {topic && (
        <span className="text-fg-dim text-xs truncate flex-1" title={topic}>
          {topic}
        </span>
      )}
      {!topic && <span className="flex-1" />}

      {/* Status pills */}
      <span
        className={`text-[10px] px-2 py-0.5 rounded-full font-semibold ${
          connectionState === 'connected'
            ? 'bg-success/10 text-success'
            : connectionState === 'connecting'
              ? 'bg-warning/10 text-warning'
              : 'bg-danger/10 text-danger'
        }`}
      >
        {connectionState}
      </span>

      {nick && (
        <span className="text-purple text-xs font-semibold">{nick}</span>
      )}

      {authDid && (
        <span
          className="text-[10px] px-2 py-0.5 rounded-full bg-purple/10 text-purple truncate max-w-32"
          title={authDid}
        >
          {authDid.length > 20 ? authDid.slice(0, 20) + '…' : authDid}
        </span>
      )}
    </header>
  );
}
