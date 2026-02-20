import { useState, useRef, useEffect, useCallback } from 'react';
import { connect, setSaslCredentials } from '../irc/client';
import { useStore } from '../store';

type LoginMode = 'at-proto' | 'guest';

export function ConnectScreen() {
  const registered = useStore((s) => s.registered);
  const connectionState = useStore((s) => s.connectionState);
  const authError = useStore((s) => s.authError);

  const [mode, setMode] = useState<LoginMode>('at-proto');
  const [handle, setHandle] = useState('');
  const [nick, setNick] = useState(() => 'web' + Math.floor(Math.random() * 99999));
  const [channels, setChannels] = useState('#freeq');
  const [server, setServer] = useState(() => {
    const loc = window.location;
    if (loc.hostname === 'app.freeq.at') return 'wss://irc.freeq.at/irc';
    const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:';
    // AT Protocol OAuth forbids "localhost" — use 127.0.0.1
    const host = loc.host.replace('localhost', '127.0.0.1');
    return `${proto}//${host}/irc`;
  });
  const [webOrigin, setWebOrigin] = useState(() => {
    const loc = window.location;
    if (loc.hostname === 'app.freeq.at') return 'https://irc.freeq.at';
    const host = loc.host.replace('localhost', '127.0.0.1');
    return `${loc.protocol}//${host}`;
  });
  const [error, setError] = useState('');
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [oauthPending, setOauthPending] = useState(false);
  const handleRef = useRef<HTMLInputElement>(null);
  const nickRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (mode === 'at-proto') handleRef.current?.focus();
    else nickRef.current?.focus();
  }, [mode]);

  if (registered) return null;

  const chans = channels.split(',').map((s) => s.trim()).filter(Boolean);

  // AT Protocol OAuth login
  const doAtLogin = useCallback(async () => {
    const h = handle.trim();
    if (!h) { setError('Enter your AT Protocol handle'); return; }
    setError('');
    setOauthPending(true);

    try {
      // Open OAuth popup — use current page origin so it goes through Vite proxy
      // (which forwards /auth to the freeq server)
      const popupOrigin = window.location.origin.replace('localhost', '127.0.0.1');
      const authUrl = `${popupOrigin}/auth/login?handle=${encodeURIComponent(h)}`;
      const popup = window.open(authUrl, 'freeq-auth', 'width=500,height=700');

      // Listen for OAuth result via BroadcastChannel
      const result = await waitForOAuthResult(popup);

      if (!result || !result.did) {
        setError('Authentication failed — no result received');
        setOauthPending(false);
        return;
      }

      // Set SASL credentials and connect
      setSaslCredentials(result.access_jwt, result.did, result.pds_url, 'oauth');

      // Derive nick from handle (e.g., chad.bsky.social → chad)
      const derivedNick = result.handle?.split('.')[0] || h.split('.')[0] || nick;
      connect(server, derivedNick, chans);
      setOauthPending(false);
    } catch (e) {
      setError(`OAuth error: ${e}`);
      setOauthPending(false);
    }
  }, [handle, server, channels, nick, webOrigin, chans]);

  // Guest login (no AT auth)
  const doGuestLogin = () => {
    if (!nick.trim()) { setError('Enter a nickname'); return; }
    setError('');
    connect(server, nick.trim(), chans);
  };

  const connecting = connectionState === 'connecting' || connectionState === 'connected';
  const displayError = error || authError;

  return (
    <div className="flex-1 flex items-center justify-center bg-bg relative overflow-hidden">
      {/* Background decoration */}
      <div className="absolute inset-0 overflow-hidden pointer-events-none">
        <div className="absolute top-1/4 left-1/4 w-96 h-96 bg-accent/[0.03] rounded-full blur-[100px]" />
        <div className="absolute bottom-1/4 right-1/4 w-96 h-96 bg-purple/[0.03] rounded-full blur-[100px]" />
      </div>

      <div className="bg-bg-secondary border border-border rounded-2xl p-8 w-[420px] max-w-[92vw] shadow-2xl relative animate-fadeIn">
        {/* Logo */}
        <div className="text-center mb-6">
          <h1 className="text-3xl font-bold tracking-tight">
            <span className="text-accent">free</span><span className="text-fg">q</span>
          </h1>
          <p className="text-fg-dim text-xs mt-1">
            IRC · AT Protocol · Open Identity
          </p>
        </div>

        {/* Mode tabs */}
        <div className="flex gap-1 bg-bg rounded-lg p-1 mb-4">
          <button
            onClick={() => setMode('at-proto')}
            className={`flex-1 py-1.5 text-xs font-medium rounded-md transition-colors ${
              mode === 'at-proto'
                ? 'bg-accent/10 text-accent'
                : 'text-fg-dim hover:text-fg-muted'
            }`}
          >
            AT Protocol
          </button>
          <button
            onClick={() => setMode('guest')}
            className={`flex-1 py-1.5 text-xs font-medium rounded-md transition-colors ${
              mode === 'guest'
                ? 'bg-bg-tertiary text-fg-muted'
                : 'text-fg-dim hover:text-fg-muted'
            }`}
          >
            Guest
          </button>
        </div>

        <div className="space-y-3">
          {mode === 'at-proto' ? (
            <>
              {/* AT Handle */}
              <div>
                <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
                  AT Protocol Handle
                </label>
                <input
                  ref={handleRef}
                  value={handle}
                  onChange={(e) => setHandle(e.target.value)}
                  placeholder="you.bsky.social"
                  onKeyDown={(e) => e.key === 'Enter' && doAtLogin()}
                  className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
                />
                <p className="text-[10px] text-fg-dim mt-1">
                  Your Bluesky or AT Protocol handle. Nickname derived automatically.
                </p>
              </div>
            </>
          ) : (
            <>
              {/* Nick */}
              <div>
                <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
                  Nickname
                </label>
                <input
                  ref={nickRef}
                  value={nick}
                  onChange={(e) => setNick(e.target.value)}
                  placeholder="your_nick"
                  onKeyDown={(e) => e.key === 'Enter' && doGuestLogin()}
                  className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
                />
              </div>
            </>
          )}

          {/* Channels */}
          <div>
            <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
              Auto-join channels
            </label>
            <input
              value={channels}
              onChange={(e) => setChannels(e.target.value)}
              placeholder="#freeq"
              onKeyDown={(e) => e.key === 'Enter' && (mode === 'at-proto' ? doAtLogin() : doGuestLogin())}
              className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
            />
          </div>

          {/* Advanced */}
          {showAdvanced && (
            <div className="animate-fadeIn space-y-3">
              <div>
                <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
                  WebSocket URL
                </label>
                <input
                  value={server}
                  onChange={(e) => setServer(e.target.value)}
                  className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors font-mono text-xs placeholder:text-fg-dim"
                />
              </div>
              <div>
                <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
                  Server HTTP Origin
                </label>
                <input
                  value={webOrigin}
                  onChange={(e) => setWebOrigin(e.target.value)}
                  className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors font-mono text-xs placeholder:text-fg-dim"
                />
                <p className="text-[10px] text-fg-dim mt-1">
                  HTTP origin of the freeq server (for OAuth). Must match --web-addr.
                </p>
              </div>
            </div>
          )}

          {!showAdvanced && (
            <button
              onClick={() => setShowAdvanced(true)}
              className="text-[11px] text-fg-dim hover:text-fg-muted"
            >
              Advanced settings ›
            </button>
          )}

          {/* Connect button */}
          <button
            onClick={mode === 'at-proto' ? doAtLogin : doGuestLogin}
            disabled={connecting || oauthPending}
            className="w-full bg-accent text-black font-bold py-2.5 rounded-lg transition-all hover:bg-accent-hover hover:shadow-[0_0_24px_rgba(0,212,170,0.15)] disabled:opacity-50 disabled:hover:shadow-none mt-1"
          >
            {oauthPending ? (
              <span className="flex items-center justify-center gap-2">
                <svg className="animate-spin w-4 h-4" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                </svg>
                Waiting for authorization...
              </span>
            ) : connecting ? (
              <span className="flex items-center justify-center gap-2">
                <svg className="animate-spin w-4 h-4" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                </svg>
                Connecting...
              </span>
            ) : mode === 'at-proto' ? (
              'Sign in with AT Protocol'
            ) : (
              'Connect as Guest'
            )}
          </button>
        </div>

        {displayError && (
          <div className="mt-3 bg-danger/10 border border-danger/20 rounded-lg px-3 py-2 text-danger text-xs animate-fadeIn">
            {displayError}
          </div>
        )}

        <div className="text-center mt-5 flex items-center justify-center gap-3 text-[10px]">
          <a href="https://freeq.at" target="_blank" className="text-fg-dim hover:text-fg-muted">freeq.at</a>
          <span className="text-border">·</span>
          <a href="https://github.com/chad/freeq" target="_blank" className="text-fg-dim hover:text-fg-muted">GitHub</a>
          <span className="text-border">·</span>
          <a href="https://freeq.at/docs/" target="_blank" className="text-fg-dim hover:text-fg-muted">Docs</a>
        </div>
      </div>
    </div>
  );
}

/**
 * Wait for OAuth result from popup window.
 * Tries BroadcastChannel, postMessage, and localStorage polling.
 */
function waitForOAuthResult(popup: Window | null): Promise<OAuthResultData | null> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      cleanup();
      reject(new Error('OAuth timed out (60s)'));
    }, 60000);

    // BroadcastChannel
    let bc: BroadcastChannel | null = null;
    try {
      bc = new BroadcastChannel('freeq-oauth');
      bc.onmessage = (e) => {
        if (e.data?.type === 'freeq-oauth' && e.data.result) {
          cleanup();
          resolve(e.data.result);
        }
      };
    } catch { /* not supported */ }

    // window.postMessage
    const msgHandler = (e: MessageEvent) => {
      if (e.data?.type === 'freeq-oauth' && e.data.result) {
        cleanup();
        resolve(e.data.result);
      }
    };
    window.addEventListener('message', msgHandler);

    // localStorage polling fallback
    const pollInterval = setInterval(() => {
      try {
        const stored = localStorage.getItem('freeq-oauth-result');
        if (stored) {
          localStorage.removeItem('freeq-oauth-result');
          cleanup();
          resolve(JSON.parse(stored));
        }
      } catch { /* ignore */ }
    }, 500);

    // Check if popup closed without result
    const closedCheck = setInterval(() => {
      if (popup && popup.closed) {
        cleanup();
        resolve(null);
      }
    }, 1000);

    function cleanup() {
      clearTimeout(timeout);
      clearInterval(pollInterval);
      clearInterval(closedCheck);
      window.removeEventListener('message', msgHandler);
      if (bc) { bc.close(); bc = null; }
    }
  });
}

interface OAuthResultData {
  did: string;
  handle: string;
  access_jwt: string;
  pds_url: string;
}
