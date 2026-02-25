import { useState, useRef, useEffect, useCallback } from 'react';
import { connect, setSaslCredentials } from '../irc/client';
import { useStore } from '../store';

type LoginMode = 'at-proto' | 'guest';

type OAuthResultData = {
  did?: string;
  handle?: string;
  token?: string;
  web_token?: string;
  access_jwt?: string;
  broker_token?: string;
  pds_url?: string;
};

type BrokerSessionResponse = {
  token: string;
  nick: string;
  did: string;
  handle: string;
};

// Default AT Protocol hosting suffixes — strip these to get short nick
const DEFAULT_SUFFIXES = [
  '.bsky.social',
  '.bsky.app',
  '.bsky.team',
  '.bsky.network',
  '.atproto.com',
];

/** Derive an IRC nick from an AT Protocol handle.
 * Custom domains (e.g. chadfowler.com) → use full handle as nick.
 * Default hosting (e.g. chad.bsky.social) → strip suffix → "chad".
 */
function nickFromHandle(handle: string): string {
  const h = handle.toLowerCase().trim();
  for (const suffix of DEFAULT_SUFFIXES) {
    if (h.endsWith(suffix)) {
      return h.slice(0, -suffix.length);
    }
  }
  // Custom domain — use the full handle as nick
  return h;
}

// localStorage keys
const LS_HANDLE = 'freeq-handle';
const LS_CHANNELS = 'freeq-channels';
const LS_BROKER_TOKEN = 'freeq-broker-token';
const LS_BROKER_BASE = 'freeq-broker-base';

export function ConnectScreen() {
  const registered = useStore((s) => s.registered);
  const connectionState = useStore((s) => s.connectionState);
  const authError = useStore((s) => s.authError);

  const [mode, setMode] = useState<LoginMode>('at-proto');
  const [handle, setHandle] = useState(() => localStorage.getItem(LS_HANDLE) || '');
  const [nick, setNick] = useState(() => 'web' + Math.floor(Math.random() * 99999));
  const [atNick, setAtNick] = useState(''); // derived nick for AT login, editable
  const [channels, setChannels] = useState(() => {
    // Check for auto-join from invite link (e.g. #auto-join=#channel)
    const hash = window.location.hash;
    if (hash.startsWith('#auto-join=')) {
      const ch = decodeURIComponent(hash.slice('#auto-join='.length));
      window.location.hash = '';
      const existing = localStorage.getItem(LS_CHANNELS) || '';
      const merged = new Set(existing.split(',').map(s => s.trim()).filter(Boolean));
      merged.add(ch);
      return [...merged].join(',');
    }
    return localStorage.getItem(LS_CHANNELS) || '#freeq';
  });
  const isTauri = !!(window as any).__TAURI_INTERNALS__;
  const [server, setServer] = useState(() => {
    if (isTauri) return 'wss://irc.freeq.at/irc';
    const loc = window.location;
    const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:';
    // In dev, replace localhost with 127.0.0.1 for OAuth compliance
    const host = loc.host.replace('localhost', '127.0.0.1');
    return `${proto}//${host}/irc`;
  });
  const [webOrigin, setWebOrigin] = useState(() => {
    if (isTauri) return 'https://irc.freeq.at';
    const loc = window.location;
    const host = loc.host.replace('localhost', '127.0.0.1');
    return `${loc.protocol}//${host}`;
  });
  const [brokerOrigin] = useState(() => {
    const stored = localStorage.getItem(LS_BROKER_BASE);
    if (stored) return stored;
    if (isTauri) return 'https://auth.freeq.at';
    const host = window.location.host;
    if (host.endsWith('freeq.at')) return 'https://auth.freeq.at';
    return webOrigin;
  });
  const [error, setError] = useState('');
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [oauthPending, setOauthPending] = useState(false);
  const [autoConnecting, setAutoConnecting] = useState(false);
  const handleRef = useRef<HTMLInputElement>(null);
  const nickRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (mode === 'at-proto') handleRef.current?.focus();
    else nickRef.current?.focus();
  }, [mode]);

  // Update derived nick when handle changes
  useEffect(() => {
    if (handle.trim()) {
      setAtNick(nickFromHandle(handle.trim()));
    }
  }, [handle]);

  // Check for OAuth result on mount (same-window redirect flow for Tauri/desktop).
  // The callback page stores the result in localStorage and redirects to /.
  // No "pending" flag needed — if a result exists, consume it.
  useEffect(() => {
    const raw = localStorage.getItem('freeq-oauth-result');
    if (!raw) return;
    localStorage.removeItem('freeq-oauth-result');
    localStorage.removeItem('freeq-oauth-pending');
    try {
      const result = JSON.parse(raw) as OAuthResultData & { _ts?: number };
      // Reject stale OAuth results (>5 minutes old) — prevents auto-connect
      // with consumed web-tokens from previous sessions
      const age = result._ts ? Date.now() - result._ts : Infinity;
      if (age > 5 * 60 * 1000) return;
      if (result?.did) {
        if (result.broker_token) {
          localStorage.setItem(LS_BROKER_TOKEN, result.broker_token);
          localStorage.setItem(LS_BROKER_BASE, brokerOrigin);
        }
        setAutoConnecting(true);
        const h = localStorage.getItem(LS_HANDLE) || result.handle || '';
        const ch = (localStorage.getItem(LS_CHANNELS) || '#freeq').split(',').map(s => s.trim()).filter(Boolean);
        const finalNick = nickFromHandle(result.handle || h);
        const token = result.web_token || result.token || result.access_jwt || '';
        setSaslCredentials(token, result.did, result.pds_url || '', 'web-token');
        const loc = window.location;
        const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:';
        const host = loc.host.replace('localhost', '127.0.0.1');
        connect(`${proto}//${host}/irc`, finalNick, ch);
      }
    } catch { /* ignore parse errors */ }
  }, [brokerOrigin]);

  // Attempt broker session refresh on load (persistent login)
  useEffect(() => {
    if (registered || oauthPending || autoConnecting) return;
    const brokerToken = localStorage.getItem(LS_BROKER_TOKEN);
    if (!brokerToken) return;

    setAutoConnecting(true);
    const ch = (localStorage.getItem(LS_CHANNELS) || '#freeq').split(',').map(s => s.trim()).filter(Boolean);

    fetch(`${brokerOrigin}/session`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ broker_token: brokerToken }),
    })
      .then(async (res) => {
        if (!res.ok) {
          throw new Error(await res.text());
        }
        return res.json();
      })
      .then((session: BrokerSessionResponse) => {
        localStorage.setItem(LS_HANDLE, session.handle || localStorage.getItem(LS_HANDLE) || '');
        setSaslCredentials(session.token, session.did, '', 'web-token');
        const finalNick = nickFromHandle(session.handle || localStorage.getItem(LS_HANDLE) || session.nick);
        connect(server, finalNick, ch);
      })
      .catch(() => {
        localStorage.removeItem(LS_BROKER_TOKEN);
        setAutoConnecting(false);
      });
  }, [registered, oauthPending, autoConnecting, brokerOrigin, server]);

  // Clear autoConnecting on auth failure or disconnect
  useEffect(() => {
    if (!autoConnecting) return;
    if (registered) { setAutoConnecting(false); return; }
    if (authError) { setAutoConnecting(false); return; }
    // If we were connecting but dropped back to disconnected, give a brief grace period
    // then show the form. This prevents a permanent spinner if SASL fails.
    if (connectionState === 'disconnected') {
      const t = setTimeout(() => setAutoConnecting(false), 2000);
      return () => clearTimeout(t);
    }
  }, [autoConnecting, registered, connectionState, authError]);

  const chans = channels.split(',').map((s) => s.trim()).filter(Boolean);

  // AT Protocol OAuth login
  const doAtLogin = useCallback(async () => {
    const h = handle.trim();
    if (!h) { setError('Enter your AT Protocol handle'); return; }
    setError('');
    setOauthPending(true);

    try {
      // Persist handle + channels for next visit
      localStorage.setItem(LS_HANDLE, h);
      localStorage.setItem(LS_CHANNELS, channels);
      localStorage.setItem(LS_BROKER_BASE, brokerOrigin);

      // Clear any stale OAuth result
      try { localStorage.removeItem('freeq-oauth-result'); } catch { /* ignore */ }

      // Use webOrigin for auth URLs (same-origin in browser, explicit server in Tauri)
      const authUrl = `${brokerOrigin}/auth/login?handle=${encodeURIComponent(h)}`;

      if (isTauri) {
        // In Tauri, navigate the main window to the OAuth URL.
        // The callback page will store the result and redirect back to /
        // where the on-mount effect picks it up.
        localStorage.setItem('freeq-oauth-pending', '1');
        window.location.href = authUrl;
        return;
      }

      // Browser: open OAuth popup
      const popup = window.open(authUrl, 'freeq-auth', 'width=500,height=700');

      // Listen for OAuth result via BroadcastChannel
      const result = await waitForOAuthResult(popup);

      if (!result || !result.did) {
        setError('Authentication failed — no result received');
        setOauthPending(false);
        return;
      }

      // Set SASL credentials using the one-time web auth token
      const token = result.web_token || result.token || result.access_jwt || '';
      setSaslCredentials(token, result.did, result.pds_url || '', 'web-token');

      if (result.broker_token) {
        localStorage.setItem(LS_BROKER_TOKEN, result.broker_token);
        localStorage.setItem(LS_BROKER_BASE, brokerOrigin);
      }

      // Use the editable nick, falling back to derived from server-returned handle
      const finalNick = atNick.trim() || nickFromHandle(result.handle || h);

      // Switch from oauthPending to autoConnecting — keeps spinner showing
      // through the connection + SASL + registration phase
      setOauthPending(false);
      setAutoConnecting(true);
      connect(server, finalNick, chans);
    } catch (e) {
      setError(`OAuth error: ${e}`);
      setOauthPending(false);
    }
  }, [handle, server, channels, atNick, chans]);

  // Guest login (no AT auth)
  const doGuestLogin = () => {
    if (!nick.trim()) { setError('Enter a nickname'); return; }
    setError('');
    localStorage.setItem(LS_CHANNELS, channels);
    connect(server, nick.trim(), chans);
  };

  const connecting = connectionState === 'connecting' || connectionState === 'connected';
  const displayError = error || authError;

  // Early returns MUST come after all hooks (React rules of hooks)
  if (registered) return null;

  if (autoConnecting || oauthPending) {
    return (
      <div className="flex-1 flex items-center justify-center bg-bg">
        <div className="text-center">
          <img src="/freeq.png" alt="freeq" className="w-16 h-16 mx-auto mb-4 animate-pulse" />
          <h1 className="text-2xl font-bold mb-2">
            <span className="text-accent">free</span><span className="text-fg">q</span>
          </h1>
          <p className="text-fg-dim text-sm">
            {oauthPending ? 'Waiting for authorization...' : 'Connecting...'}
          </p>
          {authError && (
            <p className="text-danger text-xs mt-3">{authError}</p>
          )}
        </div>
      </div>
    );
  }

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
          <img src="/freeq.png" alt="freeq" className="w-16 h-16 mx-auto mb-2" />
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
            className={`flex-1 py-2 text-sm font-semibold rounded-lg transition-colors ${
              mode === 'at-proto'
                ? 'bg-accent/10 text-accent'
                : 'text-fg-dim hover:text-fg-muted'
            }`}
          >
            AT Protocol
          </button>
          <button
            onClick={() => setMode('guest')}
            className={`flex-1 py-2 text-sm font-semibold rounded-lg transition-colors ${
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
                <label className="block text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">
                  AT Protocol Handle
                </label>
                <input
                  ref={handleRef}
                  value={handle}
                  onChange={(e) => setHandle(e.target.value)}
                  placeholder="you.bsky.social"
                  onKeyDown={(e) => e.key === 'Enter' && doAtLogin()}
                  className="w-full bg-bg border border-border rounded-lg px-4 py-3 text-base text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
                />
              </div>

              {/* Derived nick (editable) */}
              <div>
                <label className="block text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">
                  Nickname
                </label>
                <input
                  value={atNick}
                  onChange={(e) => setAtNick(e.target.value)}
                  placeholder="derived from handle"
                  onKeyDown={(e) => e.key === 'Enter' && doAtLogin()}
                  className="w-full bg-bg border border-border rounded-lg px-4 py-3 text-base text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
                />
                <p className="text-xs text-fg-dim mt-1.5">
                  Your IRC nick. Defaults to your handle — edit if you prefer something different.
                </p>
              </div>
            </>
          ) : (
            <>
              {/* Nick */}
              <div>
                <label className="block text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">
                  Nickname
                </label>
                <input
                  ref={nickRef}
                  value={nick}
                  onChange={(e) => setNick(e.target.value)}
                  placeholder="your_nick"
                  onKeyDown={(e) => e.key === 'Enter' && doGuestLogin()}
                  className="w-full bg-bg border border-border rounded-lg px-4 py-3 text-base text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
                />
              </div>
            </>
          )}

          {/* Channels */}
          <div>
            <label className="block text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">
              Auto-join channels
            </label>
            <input
              value={channels}
              onChange={(e) => setChannels(e.target.value)}
              placeholder="#freeq"
              onKeyDown={(e) => e.key === 'Enter' && (mode === 'at-proto' ? doAtLogin() : doGuestLogin())}
              className="w-full bg-bg border border-border rounded-lg px-4 py-3 text-base text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
            />
          </div>

          {/* Advanced */}
          {showAdvanced && (
            <div className="animate-fadeIn space-y-3">
              <div>
                <label className="block text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">
                  WebSocket URL
                </label>
                <input
                  value={server}
                  onChange={(e) => setServer(e.target.value)}
                  className="w-full bg-bg border border-border rounded-lg px-4 py-3 text-base text-fg outline-none focus:border-accent transition-colors font-mono text-xs placeholder:text-fg-dim"
                />
              </div>
              <div>
                <label className="block text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">
                  Server HTTP Origin
                </label>
                <input
                  value={webOrigin}
                  onChange={(e) => setWebOrigin(e.target.value)}
                  className="w-full bg-bg border border-border rounded-lg px-4 py-3 text-base text-fg outline-none focus:border-accent transition-colors font-mono text-xs placeholder:text-fg-dim"
                />
                <p className="text-xs text-fg-dim mt-1.5">
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
            className="w-full bg-accent text-black font-bold py-3 rounded-xl text-lg transition-all hover:bg-accent-hover hover:shadow-[0_0_24px_rgba(0,212,170,0.15)] disabled:opacity-50 disabled:hover:shadow-none mt-1"
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

        {/* Live social proof */}
        <ServerStats />
      </div>
    </div>
  );
}

/**
 * Wait for OAuth result from popup window.
 * Tries BroadcastChannel, postMessage, and localStorage polling.
 */
function ServerStats() {
  const [stats, setStats] = useState<{ connections: number; channels: number } | null>(null);

  useEffect(() => {
    fetch('/api/v1/health')
      .then((r) => r.ok ? r.json() : null)
      .then((d) => d && setStats(d))
      .catch(() => {});
  }, []);

  if (!stats || stats.connections === 0) return null;

  return (
    <div className="text-center mt-3 animate-fadeIn">
      <div className="inline-flex items-center gap-2 bg-bg/60 rounded-full px-3 py-1.5 border border-border/50">
        <span className="w-2 h-2 rounded-full bg-success animate-pulse" />
        <span className="text-[11px] text-fg-dim">
          <span className="text-fg-muted font-medium">{stats.connections}</span> online · <span className="text-fg-muted font-medium">{stats.channels}</span> channels
        </span>
      </div>
    </div>
  );
}

function waitForOAuthResult(popup: Window | null): Promise<OAuthResultData | null> {
  return new Promise((resolve) => {
    let resolved = false;
    const done = (data: OAuthResultData | null) => {
      if (resolved) return;
      resolved = true;
      cleanup();
      // Close the popup from the main window (more reliable than self-close)
      try { popup?.close(); } catch { /* ignore */ }
      resolve(data);
    };

    // Method 1: BroadcastChannel (server callback sends type: 'freeq-oauth')
    let bc: BroadcastChannel | null = null;
    try {
      bc = new BroadcastChannel('freeq-oauth');
      bc.onmessage = (ev) => {
        if (ev.data?.type === 'freeq-oauth' && ev.data.result) done(ev.data.result);
      };
    } catch { /* BroadcastChannel not supported */ }

    // Method 2: postMessage from popup (server callback sends type: 'freeq-oauth')
    const onMessage = (ev: MessageEvent) => {
      if (ev.data?.type === 'freeq-oauth' && ev.data.result) done(ev.data.result);
    };
    window.addEventListener('message', onMessage);

    // Method 3: localStorage polling (fallback — always works)
    const poll = setInterval(() => {
      try {
        const raw = localStorage.getItem('freeq-oauth-result');
        if (raw) {
          localStorage.removeItem('freeq-oauth-result');
          done(JSON.parse(raw));
        }
      } catch { /* ignore */ }
    }, 200);

    // Timeout after 5 minutes
    const timeout = setTimeout(() => done(null), 5 * 60 * 1000);

    function cleanup() {
      bc?.close();
      window.removeEventListener('message', onMessage);
      clearInterval(poll);
      clearTimeout(timeout);
    }
  });
}
