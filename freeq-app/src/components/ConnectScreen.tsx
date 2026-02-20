import { useState } from 'react';
import { connect, joinChannel } from '../irc/client';
import { useStore } from '../store';

export function ConnectScreen() {
  const registered = useStore((s) => s.registered);
  const connectionState = useStore((s) => s.connectionState);

  const [server, setServer] = useState(() => {
    const loc = window.location;
    if (loc.hostname === 'app.freeq.at') return 'wss://irc.freeq.at/irc';
    const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${loc.host}/irc`;
  });
  const [nick, setNick] = useState(() => 'web' + Math.floor(Math.random() * 99999));
  const [channels, setChannels] = useState('#freeq');
  const [error, setError] = useState('');

  if (registered) {
    // Auto-join channels after registration
    if (channels) {
      setTimeout(() => {
        channels.split(',').map((s) => s.trim()).filter(Boolean).forEach(joinChannel);
      }, 100);
    }
    return null;
  }

  const doConnect = () => {
    if (!server) { setError('Enter a server URL'); return; }
    setError('');
    connect(server, nick || 'web' + Math.floor(Math.random() * 99999));
  };

  const connecting = connectionState === 'connecting' || connectionState === 'connected';

  return (
    <div className="flex-1 flex items-center justify-center bg-bg">
      <div className="bg-bg-secondary border border-border rounded-xl p-8 w-[420px] max-w-[95vw] shadow-2xl">
        <h1 className="text-2xl font-bold text-center mb-1">freeq</h1>
        <p className="text-fg-dim text-xs text-center mb-6">
          IRC with identity ·{' '}
          <a href="https://freeq.at" target="_blank" className="text-fg-dim hover:text-fg-muted underline">
            freeq.at
          </a>
        </p>

        <div className="space-y-3">
          <div>
            <label className="block text-[10px] uppercase tracking-wider text-fg-dim mb-1">Server</label>
            <input
              value={server}
              onChange={(e) => setServer(e.target.value)}
              placeholder="wss://irc.freeq.at/irc"
              className="w-full bg-bg border border-border rounded px-3 py-2 text-sm text-fg outline-none focus:border-accent"
            />
          </div>

          <div className="flex gap-3">
            <div className="flex-1">
              <label className="block text-[10px] uppercase tracking-wider text-fg-dim mb-1">Nickname</label>
              <input
                value={nick}
                onChange={(e) => setNick(e.target.value)}
                placeholder="nickname"
                className="w-full bg-bg border border-border rounded px-3 py-2 text-sm text-fg outline-none focus:border-accent"
              />
            </div>
            <div className="flex-1">
              <label className="block text-[10px] uppercase tracking-wider text-fg-dim mb-1">Channels</label>
              <input
                value={channels}
                onChange={(e) => setChannels(e.target.value)}
                placeholder="#chan1,#chan2"
                className="w-full bg-bg border border-border rounded px-3 py-2 text-sm text-fg outline-none focus:border-accent"
              />
            </div>
          </div>

          <button
            onClick={doConnect}
            disabled={connecting}
            className="w-full bg-accent text-black font-bold py-2.5 rounded transition-opacity hover:opacity-90 disabled:opacity-50"
          >
            {connecting ? 'Connecting...' : 'Connect'}
          </button>
        </div>

        {error && <p className="text-danger text-xs text-center mt-3">{error}</p>}

        <p className="text-fg-dim text-[10px] text-center mt-4">
          <a href="https://freeq.at/docs/" target="_blank" className="hover:text-fg-muted">Docs</a>
          {' · '}
          <a href="https://github.com/chad/freeq" target="_blank" className="hover:text-fg-muted">GitHub</a>
        </p>
      </div>
    </div>
  );
}
