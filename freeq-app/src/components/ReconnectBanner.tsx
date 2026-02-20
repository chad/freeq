import { useStore } from '../store';

export function ReconnectBanner() {
  const connectionState = useStore((s) => s.connectionState);
  const registered = useStore((s) => s.registered);

  // Only show after we were registered (so we know we disconnected)
  if (connectionState === 'connected' || !registered) return null;

  return (
    <div className={`flex items-center justify-center gap-2 py-1.5 text-xs font-medium shrink-0 ${
      connectionState === 'connecting'
        ? 'bg-warning/10 text-warning'
        : 'bg-danger/10 text-danger'
    }`}>
      {connectionState === 'connecting' ? (
        <>
          <svg className="animate-spin w-3 h-3" viewBox="0 0 24 24">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
          </svg>
          Reconnecting...
        </>
      ) : (
        <>
          <span>●</span>
          Disconnected
        </>
      )}
    </div>
  );
}
