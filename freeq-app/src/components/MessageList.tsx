import { useEffect, useRef, useCallback } from 'react';
import { useStore, type Message } from '../store';
import { getNick, requestHistory } from '../irc/client';

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

// ── Linkify ──

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
      /`([^`]+)`/g,
      '<code class="bg-surface px-1 py-0.5 rounded text-[13px] font-mono">$1</code>',
    )
    .replace(
      /\*\*(.+?)\*\*/g,
      '<strong>$1</strong>',
    );
}

// ── Message grouping ──
// Consecutive messages from the same nick within 5 minutes are "grouped"

function isGrouped(msgs: Message[], i: number): boolean {
  if (i === 0) return false;
  const prev = msgs[i - 1];
  const curr = msgs[i];
  if (prev.isSystem || curr.isSystem) return false;
  if (prev.from !== curr.from) return false;
  if (curr.timestamp.getTime() - prev.timestamp.getTime() > 5 * 60 * 1000) return false;
  return true;
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

function FullMessage({ msg }: { msg: Message }) {
  const color = msg.isSelf ? '#b18cff' : nickColor(msg.from);
  const currentNick = getNick();
  const isMention = !msg.isSelf && msg.text.toLowerCase().includes(currentNick.toLowerCase());

  return (
    <div className={`group px-4 pt-2 pb-0.5 hover:bg-white/[0.015] flex gap-3 ${
      isMention ? 'bg-accent-dim border-l-2 border-accent' : ''
    }`}>
      {/* Avatar */}
      <div
        className="w-9 h-9 rounded-full flex items-center justify-center text-sm font-bold shrink-0 mt-0.5"
        style={{ backgroundColor: color + '20', color }}
      >
        {nickInitial(msg.from)}
      </div>

      {/* Content */}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="font-semibold text-sm" style={{ color }}>
            {msg.from}
          </span>
          <span className="text-[11px] text-fg-dim">
            {formatTime(msg.timestamp)}
          </span>
          {msg.editOf && (
            <span className="text-[10px] text-fg-dim">(edited)</span>
          )}
        </div>
        {msg.isAction ? (
          <div className="text-fg-muted italic text-sm mt-0.5">
            {msg.text}
          </div>
        ) : (
          <div
            className="text-sm leading-relaxed mt-0.5"
            dangerouslySetInnerHTML={{ __html: renderText(msg.text) }}
          />
        )}
        {/* Reactions */}
        {msg.reactions && msg.reactions.size > 0 && (
          <div className="flex gap-1 mt-1 flex-wrap">
            {[...msg.reactions.entries()].map(([emoji, nicks]) => (
              <span
                key={emoji}
                className="bg-surface hover:bg-surface-hover rounded-md px-2 py-0.5 text-xs cursor-default inline-flex items-center gap-1"
                title={[...nicks].join(', ')}
              >
                <span>{emoji}</span>
                <span className="text-fg-muted">{nicks.size}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Hover actions */}
      <div className="opacity-0 group-hover:opacity-100 shrink-0 flex items-start gap-0.5 -mt-1">
        <HoverButton emoji="😄" title="React" />
        <HoverButton emoji="↩" title="Reply" />
      </div>
    </div>
  );
}

function GroupedMessage({ msg }: { msg: Message }) {
  const currentNick = getNick();
  const isMention = !msg.isSelf && msg.text.toLowerCase().includes(currentNick.toLowerCase());

  return (
    <div className={`group px-4 py-0.5 hover:bg-white/[0.015] flex gap-3 ${
      isMention ? 'bg-accent-dim border-l-2 border-accent' : ''
    }`}>
      {/* Time (shows on hover) */}
      <span className="w-9 shrink-0 text-right text-[10px] text-fg-dim opacity-0 group-hover:opacity-100 leading-[22px]">
        {formatTime(msg.timestamp)}
      </span>

      {/* Content */}
      <div className="min-w-0 flex-1">
        {msg.isAction ? (
          <div className="text-fg-muted italic text-sm">
            {msg.text}
          </div>
        ) : (
          <div
            className="text-sm leading-relaxed"
            dangerouslySetInnerHTML={{ __html: renderText(msg.text) }}
          />
        )}
        {msg.reactions && msg.reactions.size > 0 && (
          <div className="flex gap-1 mt-1 flex-wrap">
            {[...msg.reactions.entries()].map(([emoji, nicks]) => (
              <span
                key={emoji}
                className="bg-surface hover:bg-surface-hover rounded-md px-2 py-0.5 text-xs cursor-default inline-flex items-center gap-1"
                title={[...nicks].join(', ')}
              >
                <span>{emoji}</span>
                <span className="text-fg-muted">{nicks.size}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      <div className="opacity-0 group-hover:opacity-100 shrink-0 flex items-start gap-0.5">
        <HoverButton emoji="😄" title="React" />
        <HoverButton emoji="↩" title="Reply" />
      </div>
    </div>
  );
}

function HoverButton({ emoji, title }: { emoji: string; title: string }) {
  return (
    <button
      className="w-7 h-7 rounded flex items-center justify-center text-xs hover:bg-surface text-fg-dim hover:text-fg-muted"
      title={title}
    >
      {emoji}
    </button>
  );
}

// ── Main export ──

export function MessageList() {
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const serverMessages = useStore((s) => s.serverMessages);
  const ref = useRef<HTMLDivElement>(null);
  const prevLenRef = useRef(0);

  const messages = activeChannel === 'server'
    ? serverMessages
    : channels.get(activeChannel.toLowerCase())?.messages || [];

  // Auto-scroll to bottom on new messages (if near bottom)
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

  return (
    <div ref={ref} className="flex-1 overflow-y-auto" onScroll={onScroll}>
      {messages.length === 0 && (
        <div className="flex flex-col items-center justify-center h-full text-fg-dim">
          <div className="text-4xl mb-3">💬</div>
          <div className="text-sm">
            {activeChannel === 'server' ? 'Server messages will appear here' : 'No messages yet'}
          </div>
          {activeChannel !== 'server' && (
            <div className="text-xs mt-1 text-fg-dim">
              Be the first to say something!
            </div>
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
              <GroupedMessage msg={msg} />
            ) : (
              <FullMessage msg={msg} />
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
