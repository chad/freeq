import { useStore } from '../store';
import { disconnect, reconnect } from '../irc/client';

export function ReconnectBanner() {
  const connectionState = useStore((s) => s.connectionState);
  const registered = useStore((s) => s.registered);
  const authDid = useStore((s) => s.authDid);

  // Show identity loss warning (reconnected as guest after having AT identity)
  const hadIdentity = !!localStorage.getItem('freeq-handle');
  const identityLost = registered && connectionState === 'connected' && !authDid && hadIdentity;

  // Show reconnecting/disconnected banner
  const showReconnect = registered && connectionState !== 'connected';

  if (!showReconnect && !identityLost) return null;

  if (identityLost) {
    return (
      <div className="flex items-center justify-center gap-3 py-1.5 text-xs font-medium shrink-0 bg-warning/10 text-warning">
        <span>Signed in as guest — AT Protocol session expired</span>
        <button
          onClick={() => disconnect()}
          className="underline hover:no-underline"
        >
          Sign in again
        </button>
      </div>
    );
  }

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
          <button
            onClick={() => reconnect()}
            className="ml-2 px-2 py-0.5 rounded bg-danger/20 hover:bg-danger/30 text-danger font-medium transition-colors"
          >
            Reconnect
          </button>
          <button
            onClick={() => disconnect()}
            className="px-2 py-0.5 rounded hover:bg-danger/10 text-danger/60 transition-colors"
          >
            Sign out
          </button>
        </>
      )}
    </div>
  );
}
