import { useEffect, useRef, useCallback, useState } from 'react';
import { useStore, type Message } from '../store';
import { getNick, requestHistory, sendReaction } from '../irc/client';
import { fetchProfile, getCachedProfile, type ATProfile } from '../lib/profiles';
import { EmojiPicker } from './EmojiPicker';
import { UserPopover } from './UserPopover';

// ── Colors ──

const NICK_COLORS = [
  '#ff6eb4', '#00d4aa', '#ffb547', '#5c9eff', '#b18cff',
  '#ff9547', '#00c4ff', '#ff5c5c', '#7edd7e', '#ff85d0',
];

function nickColor(nick: string): string {
  let h = 0;
  for (let i = 0; i < nick.length; i++) h = nick.charCodeAt(i) + ((h << 5) - h);
  return NICK_COLORS[Math.abs(h) % NICK_COLORS.length];
}

function nickInitial(nick: string): string {
  return (nick[0] || '?').toUpperCase();
}

// ── Time formatting ──

function formatTime(d: Date): string {
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function formatDateSeparator(d: Date): string {
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return 'Today';
  if (d.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return d.toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric' });
}

function shouldShowDateSep(msgs: Message[], i: number): boolean {
  if (i === 0) return true;
  const prev = msgs[i - 1];
  const curr = msgs[i];
  if (prev.isSystem || curr.isSystem) return false;
  return prev.timestamp.toDateString() !== curr.timestamp.toDateString();
}

// ── Linkify + markdown-lite ──

function renderText(text: string): string {
  const escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  return escaped
    .replace(
      /(https?:\/\/[^\s<]+)/g,
      '<a href="$1" target="_blank" rel="noopener" class="text-accent hover:underline break-all">$1</a>',
    )
    .replace(
      /```([\s\S]*?)```/g,
      '<pre class="bg-surface rounded px-2 py-1.5 my-1 text-[13px] font-mono overflow-x-auto">$1</pre>',
    )
    .replace(
      /`([^`]+)`/g,
      '<code class="bg-surface px-1 py-0.5 rounded text-[13px] font-mono text-pink">$1</code>',
    )
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/(?<!\*)\*([^*]+)\*(?!\*)/g, '<em>$1</em>')
    .replace(/~~(.+?)~~/g, '<del class="text-fg-dim">$1</del>');
}

// ── Message grouping ──

function isGrouped(msgs: Message[], i: number): boolean {
  if (i === 0) return false;
  const prev = msgs[i - 1];
  const curr = msgs[i];
  if (prev.isSystem || curr.isSystem) return false;
  if (prev.from !== curr.from) return false;
  if (curr.timestamp.getTime() - prev.timestamp.getTime() > 5 * 60 * 1000) return false;
  return true;
}

// ── Avatar component with AT profile support ──

function Avatar({ nick, did, size = 36 }: { nick: string; did?: string; size?: number }) {
  const [profile, setProfile] = useState<ATProfile | null>(
    did ? getCachedProfile(did) : null
  );

  useEffect(() => {
    if (did && !profile) {
      fetchProfile(did).then((p) => p && setProfile(p));
    }
  }, [did]);

  const color = nickColor(nick);

  if (profile?.avatar) {
    return (
      <img
        src={profile.avatar}
        alt=""
        className="rounded-full object-cover shrink-0"
        style={{ width: size, height: size }}
      />
    );
  }

  return (
    <div
      className="rounded-full flex items-center justify-center font-bold shrink-0"
      style={{
        width: size,
        height: size,
        backgroundColor: color + '20',
        color,
        fontSize: size * 0.4,
      }}
    >
      {nickInitial(nick)}
    </div>
  );
}

// ── Components ──

function DateSeparator({ date }: { date: Date }) {
  return (
    <div className="flex items-center gap-3 py-3 px-4">
      <div className="flex-1 border-t border-border" />
      <span className="text-[11px] text-fg-dim font-medium">{formatDateSeparator(date)}</span>
      <div className="flex-1 border-t border-border" />
    </div>
  );
}

function SystemMessage({ msg }: { msg: Message }) {
  return (
    <div className="px-4 py-0.5 flex items-start gap-2">
      <span className="w-10 shrink-0" />
      <span className="text-fg-dim text-[13px]">
        <span className="opacity-60">—</span> {msg.text}
      </span>
    </div>
  );
}

interface MessageProps {
  msg: Message;
  channel: string;
  onNickClick: (nick: string, did: string | undefined, e: React.MouseEvent) => void;
}

function FullMessage({ msg, channel, onNickClick }: MessageProps) {
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [pickerPos, setPickerPos] = useState<{ x: number; y: number } | undefined>();
  const color = msg.isSelf ? '#b18cff' : nickColor(msg.from);
  const currentNick = getNick();
  const isMention = !msg.isSelf && msg.text.toLowerCase().includes(currentNick.toLowerCase());

  // Find DID for this user from channel members
  const channels = useStore.getState().channels;
  const ch = channels.get(channel.toLowerCase());
  const member = ch?.members.get(msg.from.toLowerCase());

  const openEmojiPicker = (e: React.MouseEvent) => {
    setPickerPos({ x: e.clientX, y: e.clientY });
    setShowEmojiPicker(true);
  };

  return (
    <div className={`group px-4 pt-2 pb-0.5 hover:bg-white/[0.015] flex gap-3 relative ${
      isMention ? 'bg-accent/[0.04] border-l-2 border-accent' : ''
    }`}>
      <div
        className="cursor-pointer mt-0.5"
        onClick={(e) => onNickClick(msg.from, member?.did, e)}
      >
        <Avatar nick={msg.from} did={member?.did} />
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <button
            className="font-semibold text-sm hover:underline"
            style={{ color }}
            onClick={(e) => onNickClick(msg.from, member?.did, e)}
          >
            {msg.from}
          </button>
          <span className="text-[11px] text-fg-dim">{formatTime(msg.timestamp)}</span>
          {msg.editOf && <span className="text-[10px] text-fg-dim">(edited)</span>}
        </div>
        {msg.isAction ? (
          <div className="text-fg-muted italic text-sm mt-0.5">{msg.text}</div>
        ) : (
          <div
            className="text-sm leading-relaxed mt-0.5 [&_pre]:my-1 [&_a]:break-all"
            dangerouslySetInnerHTML={{ __html: renderText(msg.text) }}
          />
        )}
        <Reactions msg={msg} channel={channel} />
      </div>

      {/* Hover actions */}
      <div className="opacity-0 group-hover:opacity-100 absolute right-3 -top-3 flex items-center bg-bg-secondary border border-border rounded-lg shadow-lg overflow-hidden">
        <HoverBtn emoji="😄" title="Add reaction" onClick={openEmojiPicker} />

      </div>

      {showEmojiPicker && pickerPos && (
        <div className="fixed z-50" style={{ left: pickerPos.x - 140, top: pickerPos.y - 280 }}>
          <EmojiPicker
            onSelect={(emoji) => {
              sendReaction(channel, emoji, msg.id);
              setShowEmojiPicker(false);
            }}
            onClose={() => setShowEmojiPicker(false)}
          />
        </div>
      )}
    </div>
  );
}

function GroupedMessage({ msg, channel }: MessageProps) {
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [pickerPos, setPickerPos] = useState<{ x: number; y: number } | undefined>();
  const currentNick = getNick();
  const isMention = !msg.isSelf && msg.text.toLowerCase().includes(currentNick.toLowerCase());

  const openEmojiPicker = (e: React.MouseEvent) => {
    setPickerPos({ x: e.clientX, y: e.clientY });
    setShowEmojiPicker(true);
  };

  return (
    <div className={`group px-4 py-0.5 hover:bg-white/[0.015] flex gap-3 relative ${
      isMention ? 'bg-accent/[0.04] border-l-2 border-accent' : ''
    }`}>
      <span className="w-9 shrink-0 text-right text-[10px] text-fg-dim opacity-0 group-hover:opacity-100 leading-[22px]">
        {formatTime(msg.timestamp)}
      </span>
      <div className="min-w-0 flex-1">
        {msg.isAction ? (
          <div className="text-fg-muted italic text-sm">{msg.text}</div>
        ) : (
          <div
            className="text-sm leading-relaxed [&_pre]:my-1 [&_a]:break-all"
            dangerouslySetInnerHTML={{ __html: renderText(msg.text) }}
          />
        )}
        <Reactions msg={msg} channel={channel} />
      </div>

      <div className="opacity-0 group-hover:opacity-100 absolute right-3 -top-3 flex items-center bg-bg-secondary border border-border rounded-lg shadow-lg overflow-hidden">
        <HoverBtn emoji="😄" title="Add reaction" onClick={openEmojiPicker} />

      </div>

      {showEmojiPicker && pickerPos && (
        <div className="fixed z-50" style={{ left: pickerPos.x - 140, top: pickerPos.y - 280 }}>
          <EmojiPicker
            onSelect={(emoji) => {
              sendReaction(channel, emoji, msg.id);
              setShowEmojiPicker(false);
            }}
            onClose={() => setShowEmojiPicker(false)}
          />
        </div>
      )}
    </div>
  );
}

function HoverBtn({ emoji, title, onClick }: { emoji: string; title: string; onClick: (e: React.MouseEvent) => void }) {
  return (
    <button
      className="w-8 h-8 flex items-center justify-center text-xs hover:bg-bg-tertiary text-fg-dim hover:text-fg-muted"
      title={title}
      onClick={onClick}
    >
      {emoji}
    </button>
  );
}

function Reactions({ msg, channel }: { msg: Message; channel: string }) {
  if (!msg.reactions || msg.reactions.size === 0) return null;
  const myNick = getNick();
  return (
    <div className="flex gap-1 mt-1 flex-wrap">
      {[...msg.reactions.entries()].map(([emoji, nicks]) => {
        const isMine = nicks.has(myNick);
        return (
          <button
            key={emoji}
            onClick={() => sendReaction(channel, emoji, msg.id)}
            className={`rounded-md px-2 py-0.5 text-xs inline-flex items-center gap-1 border ${
              isMine
                ? 'bg-accent/10 border-accent/30 text-accent'
                : 'bg-surface border-transparent hover:border-border-bright text-fg-muted'
            }`}
            title={[...nicks].join(', ')}
          >
            <span>{emoji}</span>
            <span>{nicks.size}</span>
          </button>
        );
      })}
    </div>
  );
}

// ── Main export ──

export function MessageList() {
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const serverMessages = useStore((s) => s.serverMessages);
  const ref = useRef<HTMLDivElement>(null);
  const prevLenRef = useRef(0);
  const [popover, setPopover] = useState<{ nick: string; did?: string; pos: { x: number; y: number } } | null>(null);

  const messages = activeChannel === 'server'
    ? serverMessages
    : channels.get(activeChannel.toLowerCase())?.messages || [];

  // Auto-scroll
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (messages.length > prevLenRef.current) {
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 150;
      if (isNearBottom || prevLenRef.current === 0) {
        requestAnimationFrame(() => { el.scrollTop = el.scrollHeight; });
      }
    }
    prevLenRef.current = messages.length;
  }, [messages.length]);

  // Scroll to bottom on channel switch
  useEffect(() => {
    prevLenRef.current = 0;
    requestAnimationFrame(() => {
      if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
    });
  }, [activeChannel]);

  // Load history on scroll to top
  const onScroll = useCallback(() => {
    const el = ref.current;
    if (!el || el.scrollTop > 50) return;
    if (activeChannel !== 'server' && messages.length > 0) {
      const oldest = messages[0];
      if (!oldest.isSystem) {
        requestHistory(activeChannel, oldest.timestamp.toISOString());
      }
    }
  }, [activeChannel, messages]);

  const onNickClick = useCallback((nick: string, did: string | undefined, e: React.MouseEvent) => {
    setPopover({ nick, did, pos: { x: e.clientX, y: e.clientY } });
  }, []);

  return (
    <div ref={ref} className="flex-1 overflow-y-auto" onScroll={onScroll}>
      {messages.length === 0 && (
        <div className="flex flex-col items-center justify-center h-full text-fg-dim">
          <div className="text-4xl mb-3">💬</div>
          <div className="text-sm">
            {activeChannel === 'server' ? 'Server messages will appear here' : 'No messages yet'}
          </div>
          {activeChannel !== 'server' && (
            <div className="text-xs mt-1 text-fg-dim">Be the first to say something!</div>
          )}
        </div>
      )}
      <div className="pb-2">
        {messages.map((msg, i) => (
          <div key={msg.id}>
            {shouldShowDateSep(messages, i) && <DateSeparator date={msg.timestamp} />}
            {msg.isSystem ? (
              <SystemMessage msg={msg} />
            ) : isGrouped(messages, i) ? (
              <GroupedMessage msg={msg} channel={activeChannel} onNickClick={onNickClick} />
            ) : (
              <FullMessage msg={msg} channel={activeChannel} onNickClick={onNickClick} />
            )}
          </div>
        ))}
      </div>

      {popover && (
        <UserPopover
          nick={popover.nick}
          did={popover.did}
          position={popover.pos}
          onClose={() => setPopover(null)}
        />
      )}
    </div>
  );
}
