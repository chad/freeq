import { useEffect, useRef } from 'react';
import { useStore, type Message } from '../store';
import { getNick } from '../irc/client';

function formatTime(d: Date): string {
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

const NICK_COLORS = [
  '#ef4444', '#10b981', '#f59e0b', '#06b6d4', '#a855f7',
  '#14b8a6', '#f472b6', '#c084fc', '#22d3ee',
];

function nickColor(nick: string): string {
  let h = 0;
  for (let i = 0; i < nick.length; i++) h = nick.charCodeAt(i) + ((h << 5) - h);
  return NICK_COLORS[Math.abs(h) % NICK_COLORS.length];
}

function linkify(text: string): string {
  const escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  return escaped.replace(
    /(https?:\/\/[^\s<]+)/g,
    '<a href="$1" target="_blank" rel="noopener" class="text-accent hover:underline">$1</a>',
  );
}

function MessageRow({ msg }: { msg: Message }) {
  const currentNick = getNick();

  if (msg.isSystem) {
    return (
      <div className="px-4 py-0.5 text-fg-dim text-sm italic">
        {msg.text}
      </div>
    );
  }

  const isMention = !msg.isSelf && msg.text.toLowerCase().includes(currentNick.toLowerCase());

  return (
    <div
      className={`group px-4 py-1 hover:bg-white/[0.02] flex gap-2 ${
        isMention ? 'bg-accent/5 border-l-2 border-accent pl-3.5' : ''
      } ${msg.isAction ? 'italic text-fg-muted' : ''}`}
    >
      <span className="text-fg-dim text-[11px] w-10 text-right shrink-0 pt-0.5">
        {formatTime(msg.timestamp)}
      </span>
      <span
        className="font-semibold shrink-0 text-sm"
        style={{ color: msg.isSelf ? '#a855f7' : nickColor(msg.from) }}
      >
        {msg.isAction ? `* ${msg.from}` : msg.from}
      </span>
      <span
        className="text-sm break-words min-w-0 flex-1"
        dangerouslySetInnerHTML={{ __html: linkify(msg.text) }}
      />
      {msg.editOf && (
        <span className="text-fg-dim text-[10px] self-center">(edited)</span>
      )}
      {msg.tags?.['msgid'] && msg.tags?.['+freeq.at/sig'] && (
        <span className="text-success text-[10px] self-center" title="Signed">🔒</span>
      )}
      {/* Reactions */}
      {msg.reactions && msg.reactions.size > 0 && (
        <div className="flex gap-1 items-center ml-auto">
          {[...msg.reactions.entries()].map(([emoji, nicks]) => (
            <span
              key={emoji}
              className="bg-bg-tertiary rounded px-1.5 py-0.5 text-xs cursor-default"
              title={[...nicks].join(', ')}
            >
              {emoji} {nicks.size}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

export function MessageList() {
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const serverMessages = useStore((s) => s.serverMessages);
  const ref = useRef<HTMLDivElement>(null);

  const messages = activeChannel === 'server'
    ? serverMessages
    : channels.get(activeChannel.toLowerCase())?.messages || [];

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    const el = ref.current;
    if (el) {
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
      if (isNearBottom) {
        el.scrollTop = el.scrollHeight;
      }
    }
  }, [messages.length]);

  return (
    <div ref={ref} className="flex-1 overflow-y-auto py-2">
      {messages.length === 0 && (
        <div className="text-fg-dim text-sm text-center py-8">
          No messages yet
        </div>
      )}
      {messages.map((msg) => (
        <MessageRow key={msg.id} msg={msg} />
      ))}
    </div>
  );
}
