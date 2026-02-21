import { useStore, type Message } from '../store';
import { sendReply } from '../irc/client';
import { useState, useRef, useEffect } from 'react';

interface ThreadViewProps {
  rootMsgId: string;
  channel: string;
  onClose: () => void;
}

/** Collect a reply chain: root message + all replies referencing it */
function collectThread(messages: Message[], rootMsgId: string): Message[] {
  const root = messages.find((m) => m.id === rootMsgId);
  if (!root) return [];
  const replies = messages.filter((m) => m.replyTo === rootMsgId);
  return [root, ...replies];
}

export function ThreadView({ rootMsgId, channel, onClose }: ThreadViewProps) {
  const ch = useStore((s) => s.channels.get(channel.toLowerCase()));
  const [text, setText] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  const thread = ch ? collectThread(ch.messages, rootMsgId) : [];

  useEffect(() => {
    inputRef.current?.focus();
  }, [rootMsgId]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [thread.length]);

  const handleSend = () => {
    if (!text.trim()) return;
    sendReply(channel, rootMsgId, text.trim());
    setText('');
  };

  if (thread.length === 0) return null;

  return (
    <div className="w-80 border-l border-border flex flex-col bg-bg-secondary shrink-0">
      {/* Header */}
      <div className="h-12 border-b border-border flex items-center justify-between px-4 shrink-0">
        <span className="text-sm font-semibold text-fg">Thread</span>
        <button onClick={onClose} className="text-fg-dim hover:text-fg-muted">
          <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor">
            <path d="M3.72 3.72a.75.75 0 011.06 0L8 6.94l3.22-3.22a.75.75 0 111.06 1.06L9.06 8l3.22 3.22a.75.75 0 11-1.06 1.06L8 9.06l-3.22 3.22a.75.75 0 01-1.06-1.06L6.94 8 3.72 4.78a.75.75 0 010-1.06z"/>
          </svg>
        </button>
      </div>

      {/* Messages */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {thread.map((msg, i) => (
          <div key={msg.id} className={i === 0 ? 'pb-3 border-b border-border' : ''}>
            <div className="flex items-center gap-2">
              <span className="text-xs font-semibold text-fg">{msg.from}</span>
              <span className="text-[10px] text-fg-dim">
                {msg.timestamp.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
              </span>
            </div>
            <div className="text-sm text-fg-muted mt-0.5">{msg.text}</div>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Reply input */}
      <div className="border-t border-border p-3">
        <div className="flex gap-2">
          <input
            ref={inputRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleSend()}
            placeholder="Reply in thread..."
            className="flex-1 bg-bg border border-border rounded-lg px-3 py-2 text-sm text-fg outline-none focus:border-accent"
          />
          <button
            onClick={handleSend}
            disabled={!text.trim()}
            className="px-3 py-2 bg-accent text-black rounded-lg text-sm font-medium disabled:opacity-30"
          >
            Send
          </button>
        </div>
        <div className="text-[10px] text-fg-dim mt-1">
          {thread.length - 1} {thread.length === 2 ? 'reply' : 'replies'}
        </div>
      </div>
    </div>
  );
}
