import { useState, useRef, useEffect } from 'react';
import { connect, joinChannel } from '../irc/client';
import { useStore } from '../store';

export function ConnectScreen() {
  const registered = useStore((s) => s.registered);
  const connectionState = useStore((s) => s.connectionState);
  const authError = useStore((s) => s.authError);

  const [server, setServer] = useState(() => {
    const loc = window.location;
    if (loc.hostname === 'app.freeq.at') return 'wss://irc.freeq.at/irc';
    const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${loc.host}/irc`;
  });
  const [nick, setNick] = useState(() => 'web' + Math.floor(Math.random() * 99999));
  const [channels, setChannels] = useState('#freeq');
  const [error, setError] = useState('');
  const [showAdvanced, setShowAdvanced] = useState(false);
  const joinedRef = useRef(false);
  const nickRef = useRef<HTMLInputElement>(null);

  useEffect(() => { nickRef.current?.focus(); }, []);

  // Auto-join channels once after registration
  useEffect(() => {
    if (registered && channels && !joinedRef.current) {
      joinedRef.current = true;
      channels.split(',').map((s) => s.trim()).filter(Boolean).forEach(joinChannel);
    }
  }, [registered, channels]);

  if (registered) return null;

  const doConnect = () => {
    if (!server) { setError('Enter a server URL'); return; }
    if (!nick.trim()) { setError('Enter a nickname'); return; }
    setError('');
    joinedRef.current = false;
    connect(server, nick.trim());
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

      <div className="bg-bg-secondary border border-border rounded-2xl p-8 w-[400px] max-w-[92vw] shadow-2xl relative animate-fadeIn">
        {/* Logo */}
        <div className="text-center mb-6">
          <h1 className="text-3xl font-bold tracking-tight">
            <span className="text-accent">free</span><span className="text-fg">q</span>
          </h1>
          <p className="text-fg-dim text-xs mt-1">
            IRC · AT Protocol · Open Identity
          </p>
        </div>

        <div className="space-y-3">
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
              onKeyDown={(e) => e.key === 'Enter' && doConnect()}
              className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
            />
          </div>

          {/* Channels */}
          <div>
            <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
              Auto-join channels
            </label>
            <input
              value={channels}
              onChange={(e) => setChannels(e.target.value)}
              placeholder="#freeq"
              onKeyDown={(e) => e.key === 'Enter' && doConnect()}
              className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors placeholder:text-fg-dim"
            />
          </div>

          {/* Advanced */}
          {showAdvanced && (
            <div className="animate-fadeIn">
              <label className="block text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5">
                Server
              </label>
              <input
                value={server}
                onChange={(e) => setServer(e.target.value)}
                placeholder="wss://irc.freeq.at/irc"
                onKeyDown={(e) => e.key === 'Enter' && doConnect()}
                className="w-full bg-bg border border-border rounded-lg px-3 py-2.5 text-sm text-fg outline-none focus:border-accent transition-colors font-mono text-xs placeholder:text-fg-dim"
              />
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
            onClick={doConnect}
            disabled={connecting}
            className="w-full bg-accent text-black font-bold py-2.5 rounded-lg transition-all hover:bg-accent-hover hover:shadow-[0_0_24px_rgba(0,212,170,0.15)] disabled:opacity-50 disabled:hover:shadow-none mt-1"
          >
            {connecting ? (
              <span className="flex items-center justify-center gap-2">
                <svg className="animate-spin w-4 h-4" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                </svg>
                Connecting...
              </span>
            ) : 'Connect'}
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
