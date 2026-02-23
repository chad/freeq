import { useState, useEffect, useRef } from 'react';
import { useStore } from '../store';
import { joinChannel, partChannel, disconnect } from '../irc/client';
import { fetchProfile, getCachedProfile } from '../lib/profiles';

interface SidebarProps {
  onOpenSettings: () => void;
}

export function Sidebar({ onOpenSettings }: SidebarProps) {
  const channels = useStore((s) => s.channels);
  const activeChannel = useStore((s) => s.activeChannel);
  const setActive = useStore((s) => s.setActiveChannel);
  const serverMessages = useStore((s) => s.serverMessages);
  const connectionState = useStore((s) => s.connectionState);
  const nick = useStore((s) => s.nick);
  const authDid = useStore((s) => s.authDid);
  const [joinInput, setJoinInput] = useState('');
  const [showJoin, setShowJoin] = useState(false);

  const favorites = useStore((s) => s.favorites);
  useStore((s) => s.mutedChannels); // subscribe for re-render

  const allJoined = [...channels.values()].filter((ch) => ch.isJoined);
  const allChans = allJoined.filter((ch) => ch.name.startsWith('#') || ch.name.startsWith('&')).sort((a, b) => a.name.localeCompare(b.name));
  const favList = allChans.filter((ch) => favorites.has(ch.name.toLowerCase()));
  const chanList = allChans.filter((ch) => !favorites.has(ch.name.toLowerCase()));
  const dmList = allJoined.filter((ch) => !ch.name.startsWith('#') && !ch.name.startsWith('&') && ch.name !== 'server').sort((a, b) => a.name.localeCompare(b.name));

  const handleJoin = () => {
    const ch = joinInput.trim();
    if (ch) {
      joinChannel(ch.startsWith('#') ? ch : `#${ch}`);
      setJoinInput('');
      setShowJoin(false);
    }
  };

  return (
    <aside data-testid="sidebar" className="w-64 bg-bg-secondary flex flex-col shrink-0 overflow-hidden">
      {/* Brand */}
      <div className="h-14 flex items-center px-4 border-b border-border shrink-0 gap-2.5">
        <img src="/freeq.png" alt="" className="w-7 h-7" />
        <span className="text-accent font-bold text-xl tracking-tight">freeq</span>
        <span className={`ml-auto w-2 h-2 rounded-full ${
          connectionState === 'connected' ? 'bg-success' :
          connectionState === 'connecting' ? 'bg-warning animate-pulse' : 'bg-danger'
        }`} />
      </div>

      <nav className="flex-1 overflow-y-auto py-2 px-2">
        {/* Server */}
        <button
          onClick={() => setActive('server')}
          className={`w-full text-left px-3 py-2 rounded-lg text-[15px] flex items-center gap-2.5 mb-1 ${
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
          <span className="text-xs uppercase tracking-wider text-fg-dim font-bold">
            Channels
          </span>
          <div className="flex items-center gap-0.5">
            <button
              onClick={() => useStore.getState().setChannelListOpen(true)}
              className="text-fg-dim hover:text-fg-muted text-[10px] px-1"
              title="Browse channels"
            >
              ⊞
            </button>
            <button
              onClick={() => setShowJoin(!showJoin)}
              className="text-fg-dim hover:text-fg-muted text-lg leading-none px-1"
              title="Join channel"
            >
              +
            </button>
          </div>
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

        {/* Favorites */}
        {favList.length > 0 && (
          <>
            <div className="mt-3 mb-1 px-2">
              <span className="text-xs uppercase tracking-wider text-fg-dim font-bold flex items-center gap-1">
                <span className="text-warning text-[10px]">★</span> Favorites
              </span>
            </div>
            {favList.map((ch) => <ChannelButton key={ch.name} ch={ch as any} isActive={activeChannel.toLowerCase() === ch.name.toLowerCase()} onSelect={setActive} icon="#" />)}
          </>
        )}

        {chanList.map((ch) => <ChannelButton key={ch.name} ch={ch as any} isActive={activeChannel.toLowerCase() === ch.name.toLowerCase()} onSelect={setActive} icon="#" />)}

        {/* DMs */}
        {dmList.length > 0 && (
          <>
            <div className="mt-3 mb-1 px-2">
              <span className="text-xs uppercase tracking-wider text-fg-dim font-bold">
                Messages
              </span>
            </div>
            {dmList.map((ch) => <ChannelButton key={ch.name} ch={ch as any} isActive={activeChannel.toLowerCase() === ch.name.toLowerCase()} onSelect={setActive} icon="@" showPreview />)}
          </>
        )}
      </nav>

      {/* User footer */}
      <div className="border-t border-border px-3 py-3 shrink-0">
        <div className="flex items-center gap-2.5">
          <SelfAvatar nick={nick} did={authDid} />
          <div className="min-w-0 flex-1">
            <div className="text-[15px] font-semibold truncate">{nick}</div>
            {authDid && (
              <div className="text-xs text-fg-dim truncate" title={authDid}>
                {authDid}
              </div>
            )}
          </div>
          <button
            onClick={() => useStore.getState().setBookmarksPanelOpen(true)}
            className="text-fg-dim hover:text-fg-muted p-1"
            title="Bookmarks (⌘B)"
          >
            <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor">
              <path d="M2 2a2 2 0 012-2h8a2 2 0 012 2v13.5a.5.5 0 01-.777.416L8 13.101l-5.223 2.815A.5.5 0 012 15.5V2zm2-1a1 1 0 00-1 1v12.566l4.723-2.482a.5.5 0 01.554 0L13 14.566V2a1 1 0 00-1-1H4z"/>
            </svg>
          </button>
          <button
            onClick={onOpenSettings}
            className="text-fg-dim hover:text-fg-muted p-1"
            title="Settings"
          >
            <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 4.754a3.246 3.246 0 100 6.492 3.246 3.246 0 000-6.492zM5.754 8a2.246 2.246 0 114.492 0 2.246 2.246 0 01-4.492 0z"/>
              <path d="M9.796 1.343c-.527-1.79-3.065-1.79-3.592 0l-.094.319a.873.873 0 01-1.255.52l-.292-.16c-1.64-.892-3.433.902-2.54 2.541l.159.292a.873.873 0 01-.52 1.255l-.319.094c-1.79.527-1.79 3.065 0 3.592l.319.094a.873.873 0 01.52 1.255l-.16.292c-.892 1.64.901 3.434 2.541 2.54l.292-.159a.873.873 0 011.255.52l.094.319c.527 1.79 3.065 1.79 3.592 0l.094-.319a.873.873 0 011.255-.52l.292.16c1.64.893 3.434-.902 2.54-2.541l-.159-.292a.873.873 0 01.52-1.255l.319-.094c1.79-.527 1.79-3.065 0-3.592l-.319-.094a.873.873 0 01-.52-1.255l.16-.292c.893-1.64-.902-3.433-2.541-2.54l-.292.159a.873.873 0 01-1.255-.52l-.094-.319z"/>
            </svg>
          </button>
          <button
            onClick={disconnect}
            className="text-fg-dim hover:text-danger p-1"
            title="Disconnect"
          >
            <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="currentColor">
              <path d="M10 12.5a.5.5 0 01-.5.5h-8a.5.5 0 01-.5-.5v-9a.5.5 0 01.5-.5h8a.5.5 0 01.5.5v2a.5.5 0 001 0v-2A1.5 1.5 0 009.5 2h-8A1.5 1.5 0 000 3.5v9A1.5 1.5 0 001.5 14h8a1.5 1.5 0 001.5-1.5v-2a.5.5 0 00-1 0v2z"/>
              <path fillRule="evenodd" d="M15.854 8.354a.5.5 0 000-.708l-3-3a.5.5 0 00-.708.708L14.293 7.5H5.5a.5.5 0 000 1h8.793l-2.147 2.146a.5.5 0 00.708.708l3-3z"/>
            </svg>
          </button>
        </div>
      </div>
    </aside>
  );
}

function SelfAvatar({ nick, did }: { nick: string; did: string | null }) {
  const [avatarUrl, setAvatarUrl] = useState<string | null>(() => {
    if (!did) return null;
    return getCachedProfile(did)?.avatar || null;
  });

  useEffect(() => {
    if (did && !avatarUrl) {
      fetchProfile(did).then((p) => p?.avatar && setAvatarUrl(p.avatar));
    }
  }, [did]);

  if (avatarUrl) {
    return <img src={avatarUrl} alt="" className="w-9 h-9 rounded-full object-cover shrink-0" />;
  }
  return (
    <div className="w-9 h-9 rounded-full bg-surface flex items-center justify-center text-accent font-bold text-[15px] shrink-0">
      {(nick || '?')[0].toUpperCase()}
    </div>
  );
}

function ChannelButton({ ch, isActive, onSelect, icon, showPreview }: {
  ch: { name: string; mentionCount: number; unreadCount: number; messages: any[]; members: Map<string, any> };
  isActive: boolean;
  onSelect: (name: string) => void;
  icon: string;
  showPreview?: boolean;
}) {
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null);
  const isFav = useStore((s) => s.favorites.has(ch.name.toLowerCase()));
  const isMuted = useStore((s) => s.mutedChannels.has(ch.name.toLowerCase()));
  const hasMention = ch.mentionCount > 0;
  const hasUnread = ch.unreadCount > 0;

  // Last message preview for DMs
  const lastMsg = showPreview ? ch.messages.filter((m: any) => !m.isSystem).slice(-1)[0] : null;
  const preview = lastMsg ? `${lastMsg.from}: ${lastMsg.text}` : null;
  const lastTime = lastMsg ? formatSidebarTime(new Date(lastMsg.timestamp)) : null;

  return (
    <>
    <button
      onClick={() => onSelect(ch.name)}
      onContextMenu={(e) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY }); }}
      className={`w-full text-left px-3 py-2 rounded-lg flex items-center gap-2.5 ${
        isMuted ? 'opacity-40 ' : ''
      }${
        isActive
          ? 'bg-surface text-fg'
          : hasMention
            ? 'text-fg font-semibold hover:bg-bg-tertiary'
            : hasUnread
              ? 'text-fg-muted hover:bg-bg-tertiary'
              : 'text-fg-dim hover:text-fg-muted hover:bg-bg-tertiary'
      }`}
    >
      {/* Icon / DM avatar */}
      {showPreview ? (
        <div className="relative shrink-0">
          <div className="w-8 h-8 rounded-full bg-surface flex items-center justify-center text-accent font-bold text-sm">
            {(ch.name[0] || '?').toUpperCase()}
          </div>
          <OnlineDot nick={ch.name} />
        </div>
      ) : (
        <span className={`shrink-0 text-[15px] font-medium ${isActive ? 'text-accent' : 'opacity-50'}`}>{icon}</span>
      )}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-1">
          <span className="truncate text-[15px]">{ch.name.replace(/^[#&]/, '')}</span>
          {!showPreview && ch.members.size > 0 && (
            <span className="text-[10px] text-fg-dim ml-auto shrink-0">{ch.members.size}</span>
          )}
          {showPreview && lastTime && (
            <span className="text-[10px] text-fg-dim ml-auto shrink-0">{lastTime}</span>
          )}
        </div>
        {showPreview && preview && (
          <div className="text-xs text-fg-dim truncate mt-0.5">{preview.slice(0, 50)}</div>
        )}
      </div>
      {hasMention && (
        <span className="shrink-0 bg-danger text-white text-xs min-w-[20px] text-center px-1.5 py-0.5 rounded-full font-bold">
          {ch.mentionCount}
        </span>
      )}
      {!hasMention && hasUnread && (
        <span className="shrink-0 w-1.5 h-1.5 rounded-full bg-fg-muted" />
      )}
    </button>
    {ctxMenu && <SidebarContextMenu
      channel={ch.name}
      isFav={isFav}
      isMuted={isMuted}
      isChannel={ch.name.startsWith('#')}
      position={ctxMenu}
      onClose={() => setCtxMenu(null)}
    />}
    </>
  );
}

function SidebarContextMenu({ channel, isFav, isMuted, isChannel, position, onClose }: {
  channel: string;
  isFav: boolean;
  isMuted: boolean;
  isChannel: boolean;
  position: { x: number; y: number };
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="fixed z-50 bg-bg-secondary border border-border rounded-xl shadow-2xl py-1.5 min-w-[160px] animate-fadeIn"
      style={{ left: Math.min(position.x, window.innerWidth - 180), top: Math.min(position.y, window.innerHeight - 200) }}
    >
      {isChannel && (
        <button onClick={() => { useStore.getState().toggleFavorite(channel); onClose(); }}
          className="w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 hover:bg-bg-tertiary text-fg-muted hover:text-fg">
          <span className="w-5 text-center">{isFav ? '★' : '☆'}</span>
          {isFav ? 'Remove from Favorites' : 'Add to Favorites'}
        </button>
      )}
      <button onClick={() => { useStore.getState().toggleMuted(channel); onClose(); }}
        className="w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 hover:bg-bg-tertiary text-fg-muted hover:text-fg">
        <span className="w-5 text-center">{isMuted ? '🔔' : '🔇'}</span>
        {isMuted ? 'Unmute' : 'Mute notifications'}
      </button>
      <button onClick={() => {
          navigator.clipboard.writeText(`https://irc.freeq.at/join/${encodeURIComponent(channel)}`);
          onClose();
        }}
        className="w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 hover:bg-bg-tertiary text-fg-muted hover:text-fg">
        <span className="w-5 text-center">🔗</span>
        Copy invite link
      </button>
      <div className="h-px bg-border mx-2 my-1" />
      {isChannel && (
        <button onClick={() => { partChannel(channel); onClose(); }}
          className="w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 hover:bg-danger/10 text-danger">
          <span className="w-5 text-center">🚪</span>
          Leave channel
        </button>
      )}
    </div>
  );
}

/** Shows a green/yellow online dot for a DM contact. */
function OnlineDot({ nick }: { nick: string }) {
  const channels = useStore((s) => s.channels);
  // Check if this nick is online in any shared channel
  for (const [, ch] of channels) {
    const member = ch.members.get(nick.toLowerCase());
    if (member) {
      const isAway = member.away != null;
      return (
        <span className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-bg-secondary ${
          isAway ? 'bg-warning' : 'bg-success'
        }`} />
      );
    }
  }
  return null;
}

function formatSidebarTime(d: Date): string {
  const now = new Date();
  const diff = now.getTime() - d.getTime();
  if (diff < 60000) return 'now';
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m`;
  if (diff < 86400000) return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  if (diff < 604800000) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}
