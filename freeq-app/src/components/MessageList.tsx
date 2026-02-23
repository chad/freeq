import { useEffect, useRef, useCallback, useState } from 'react';
import { useStore, type Message } from '../store';
import { getNick, requestHistory, sendReaction } from '../irc/client';
import { fetchProfile, getCachedProfile, type ATProfile } from '../lib/profiles';
import { EmojiPicker } from './EmojiPicker';
import { UserPopover } from './UserPopover';
import { BlueskyEmbed } from './BlueskyEmbed';
import { LinkPreview } from './LinkPreview';
import { MessageContextMenu } from './MessageContextMenu';

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

// Image URL patterns (CDN, direct links)
const IMAGE_URL_RE = /https?:\/\/[^\s<]+\.(?:jpg|jpeg|png|gif|webp)(?:\?[^\s<]*)?/gi;
const CDN_IMAGE_RE = /https?:\/\/cdn\.bsky\.app\/img\/[^\s<]+/gi;

function extractImageUrls(text: string): string[] {
  const urls: string[] = [];
  const matches = text.match(IMAGE_URL_RE) || [];
  const cdnMatches = text.match(CDN_IMAGE_RE) || [];
  const all = new Set([...matches, ...cdnMatches]);
  for (const u of all) urls.push(u);
  return urls;
}

/** Text WITHOUT image URLs (for display above images) */
function textWithoutImages(text: string, imageUrls: string[]): string {
  let result = text;
  for (const url of imageUrls) {
    result = result.replace(url, '').trim();
  }
  return result;
}

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

// ── Message content (text + inline images) ──

// Bluesky post URL pattern
const BSKY_POST_RE = /https?:\/\/bsky\.app\/profile\/([^/]+)\/post\/([a-zA-Z0-9]+)/;
// YouTube URL pattern  
const YT_RE = /(?:youtube\.com\/watch\?v=|youtu\.be\/)([a-zA-Z0-9_-]{11})/;

function MessageContent({ msg }: { msg: Message }) {
  const setLightbox = useStore((s) => s.setLightboxUrl);

  if (msg.isAction) {
    return <div className="text-fg-muted italic text-[15px] mt-0.5">{msg.text}</div>;
  }

  const imageUrls = extractImageUrls(msg.text);
  const cleanText = imageUrls.length > 0 ? textWithoutImages(msg.text, imageUrls) : msg.text;

  // Check for embeddable URLs
  const bskyMatch = msg.text.match(BSKY_POST_RE);
  const ytMatch = msg.text.match(YT_RE);

  return (
    <div className="mt-0.5">
      {/* Reply context */}
      {msg.replyTo && <ReplyBadge msgId={msg.replyTo} />}

      {cleanText && (
        <div
          className="text-[15px] leading-relaxed [&_pre]:my-1 [&_a]:break-all"
          dangerouslySetInnerHTML={{ __html: renderText(cleanText) }}
        />
      )}

      {/* Inline images */}
      {imageUrls.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-2">
          {imageUrls.map((url) => (
            <button key={url} onClick={() => setLightbox(url)} className="block cursor-zoom-in">
              <img
                src={url}
                alt=""
                className="max-w-sm max-h-80 rounded-lg border border-border object-contain bg-bg-tertiary hover:opacity-90 transition-opacity"
                loading="lazy"
                onError={(e) => {
                  const el = e.currentTarget;
                  el.style.display = 'none';
                }}
              />
            </button>
          ))}
        </div>
      )}

      {/* Bluesky post embed */}
      {bskyMatch && <BlueskyEmbed handle={bskyMatch[1]} rkey={bskyMatch[2]} />}

      {/* YouTube thumbnail */}
      {ytMatch && (
        <a
          href={`https://youtube.com/watch?v=${ytMatch[1]}`}
          target="_blank"
          rel="noopener"
          className="mt-2 block max-w-sm rounded-lg overflow-hidden border border-border hover:border-accent/50 transition-colors"
        >
          <img
            src={`https://img.youtube.com/vi/${ytMatch[1]}/mqdefault.jpg`}
            alt="YouTube video"
            className="w-full"
            loading="lazy"
          />
          <div className="bg-bg-tertiary px-3 py-1.5 text-xs text-fg-muted flex items-center gap-1">
            <span className="text-red-500">▶</span> YouTube
          </div>
        </a>
      )}

      {/* Link preview for other URLs (not images, Bluesky, or YouTube) */}
      {!bskyMatch && !ytMatch && imageUrls.length === 0 && (() => {
        const urlMatch = msg.text.match(/(https?:\/\/[^\s<]+)/);
        return urlMatch ? <LinkPreview url={urlMatch[1]} /> : null;
      })()}
    </div>
  );
}

/** Inline reply badge showing the original message */
function ReplyBadge({ msgId }: { msgId: string }) {
  const channels = useStore((s) => s.channels);
  const activeChannel = useStore((s) => s.activeChannel);
  const ch = channels.get(activeChannel.toLowerCase());
  const original = ch?.messages.find((m) => m.id === msgId);
  if (!original) return null;

  return (
    <div className="flex items-center gap-2 text-sm text-fg-dim mb-1.5 pl-2 border-l-2 border-accent/30">
      <span className="font-semibold text-fg-muted">{original.from}</span>
      <span className="truncate max-w-[300px]">{original.text}</span>
    </div>
  );
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

function Avatar({ nick, did, size = 40 }: { nick: string; did?: string; size?: number }) {
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
      <span className="text-xs text-fg-dim font-semibold">{formatDateSeparator(date)}</span>
      <div className="flex-1 border-t border-border" />
    </div>
  );
}

function SystemMessage({ msg }: { msg: Message }) {
  return (
    <div className="px-4 py-1 flex items-start gap-3">
      <span className="w-10 shrink-0" />
      <span className="text-fg-dim text-sm">
        <span className="opacity-60">—</span>{' '}
        <span dangerouslySetInnerHTML={{ __html: renderText(msg.text) }} />
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
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null);
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
    <div
      className={`group px-4 pt-3 pb-1 hover:bg-white/[0.02] flex gap-3 relative ${
        isMention ? 'bg-accent/[0.04] border-l-2 border-accent' : ''
      }`}
      onContextMenu={(e) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY }); }}
    >
      <div
        className="cursor-pointer mt-0.5"
        onClick={(e) => onNickClick(msg.from, member?.did, e)}
      >
        <Avatar nick={msg.from} did={member?.did} />
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <button
            className="font-semibold text-[15px] hover:underline"
            style={{ color }}
            onClick={(e) => onNickClick(msg.from, member?.did, e)}
          >
            {msg.from}
          </button>
          {member?.did && <VerifiedBadge />}
          {member?.away != null && (
            <span className="text-xs text-fg-dim bg-warning/10 text-warning px-1.5 py-0.5 rounded">away</span>
          )}
          <span className="text-xs text-fg-dim whitespace-nowrap cursor-default" title={msg.timestamp.toLocaleString([], { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })}>{formatTime(msg.timestamp)}</span>
          {msg.editOf && <span className="text-xs text-fg-dim">(edited)</span>}
        </div>
        <MessageContent msg={msg} />
        <Reactions msg={msg} channel={channel} />
      </div>

      {/* Message actions — hover on desktop, tap on mobile */}
      <div className="opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 absolute right-3 -top-3 flex items-center bg-bg-secondary border border-border rounded-lg shadow-lg overflow-hidden transition-opacity z-10">
        <HoverBtn emoji="↩️" title="Reply" onClick={() => {
          useStore.getState().setReplyTo({ msgId: msg.id, from: msg.from, text: msg.text, channel });
        }} />
        <HoverBtn emoji="🧵" title="View thread" onClick={() => {
          useStore.getState().openThread(msg.id, channel);
        }} />
        {msg.isSelf && !msg.isSystem && (
          <HoverBtn emoji="✏️" title="Edit" onClick={() => {
            useStore.getState().setEditingMsg({ msgId: msg.id, text: msg.text, channel });
          }} />
        )}
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

      {ctxMenu && (
        <MessageContextMenu
          msg={msg}
          channel={channel}
          position={ctxMenu}
          onClose={() => setCtxMenu(null)}
          onReply={() => useStore.getState().setReplyTo({ msgId: msg.id, from: msg.from, text: msg.text, channel })}
          onEdit={() => useStore.getState().setEditingMsg({ msgId: msg.id, text: msg.text, channel })}
          onThread={() => useStore.getState().openThread(msg.id, channel)}
          onReact={openEmojiPicker}
        />
      )}
    </div>
  );
}

function GroupedMessage({ msg, channel }: MessageProps) {
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [pickerPos, setPickerPos] = useState<{ x: number; y: number } | undefined>();
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null);
  const currentNick = getNick();
  const isMention = !msg.isSelf && msg.text.toLowerCase().includes(currentNick.toLowerCase());

  const openEmojiPicker = (e: React.MouseEvent) => {
    setPickerPos({ x: e.clientX, y: e.clientY });
    setShowEmojiPicker(true);
  };

  return (
    <div
      className={`group px-4 py-0.5 hover:bg-white/[0.02] flex gap-3 relative ${
        isMention ? 'bg-accent/[0.04] border-l-2 border-accent' : ''
      }`}
      onContextMenu={(e) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY }); }}
    >
      <span className="w-10 shrink-0 text-right text-[11px] text-fg-dim opacity-0 group-hover:opacity-100 leading-[24px] cursor-default" title={msg.timestamp.toLocaleString([], { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })}>
        {formatTime(msg.timestamp)}
      </span>
      <div className="min-w-0 flex-1">
        <MessageContent msg={msg} />
        <Reactions msg={msg} channel={channel} />
      </div>

      <div className="opacity-0 group-hover:opacity-100 absolute right-3 -top-3 flex items-center bg-bg-secondary border border-border rounded-lg shadow-lg overflow-hidden">
        <HoverBtn emoji="↩️" title="Reply" onClick={() => {
          useStore.getState().setReplyTo({ msgId: msg.id, from: msg.from, text: msg.text, channel });
        }} />
        {msg.isSelf && !msg.isSystem && (
          <HoverBtn emoji="✏️" title="Edit" onClick={() => {
            useStore.getState().setEditingMsg({ msgId: msg.id, text: msg.text, channel });
          }} />
        )}
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

      {ctxMenu && (
        <MessageContextMenu
          msg={msg}
          channel={channel}
          position={ctxMenu}
          onClose={() => setCtxMenu(null)}
          onReply={() => useStore.getState().setReplyTo({ msgId: msg.id, from: msg.from, text: msg.text, channel })}
          onEdit={() => useStore.getState().setEditingMsg({ msgId: msg.id, text: msg.text, channel })}
          onThread={() => useStore.getState().openThread(msg.id, channel)}
          onReact={(e: React.MouseEvent) => { setPickerPos({ x: e.clientX, y: e.clientY }); setShowEmojiPicker(true); }}
        />
      )}
    </div>
  );
}

/** Verification badge for AT Protocol-authenticated users */
function VerifiedBadge() {
  return (
    <span className="text-accent text-xs" title="AT Protocol verified identity">
      <svg className="w-3.5 h-3.5 inline -mt-0.5" viewBox="0 0 16 16" fill="currentColor">
        <path d="M8 0a8 8 0 100 16A8 8 0 008 0zm3.78 5.97l-4.5 5a.75.75 0 01-1.06.02l-2-1.86a.75.75 0 011.02-1.1l1.45 1.35 3.98-4.43a.75.75 0 011.11 1.02z"/>
      </svg>
    </span>
  );
}

function HoverBtn({ emoji, title, onClick }: { emoji: string; title: string; onClick: (e: React.MouseEvent) => void }) {
  return (
    <button
      className="w-9 h-9 flex items-center justify-center text-sm hover:bg-bg-tertiary text-fg-dim hover:text-fg-muted"
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
    <div className="flex gap-1.5 mt-1.5 flex-wrap">
      {[...msg.reactions.entries()].map(([emoji, nicks]) => {
        const isMine = nicks.has(myNick);
        return (
          <button
            key={emoji}
            onClick={() => sendReaction(channel, emoji, msg.id)}
            className={`rounded-lg px-2.5 py-1 text-sm inline-flex items-center gap-1.5 border ${
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
  const messages = useStore((s) => {
    if (s.activeChannel === 'server') return s.serverMessages;
    return s.channels.get(s.activeChannel.toLowerCase())?.messages || [];
  });
  const lastReadMsgId = useStore((s) => s.channels.get(s.activeChannel.toLowerCase())?.lastReadMsgId);
  const ref = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);
  const [showScrollBtn, setShowScrollBtn] = useState(false);
  const [popover, setPopover] = useState<{ nick: string; did?: string; pos: { x: number; y: number } } | null>(null);

  // Track whether user has scrolled up (unstick from bottom)
  const handleScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    stickToBottomRef.current = atBottom;
    setShowScrollBtn(!atBottom);
  }, []);

  // Scroll to bottom when messages change (if stuck to bottom)
  useEffect(() => {
    if (!stickToBottomRef.current) return;
    const scrollBottom = () => {
      if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
    };
    // Double RAF ensures layout is complete after React render
    requestAnimationFrame(() => requestAnimationFrame(scrollBottom));
  }, [messages.length, messages]);

  // Always scroll to bottom on channel switch
  useEffect(() => {
    stickToBottomRef.current = true;
    const scrollBottom = () => {
      if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
    };
    scrollBottom();
    requestAnimationFrame(() => requestAnimationFrame(scrollBottom));
    const t = setTimeout(scrollBottom, 150);
    return () => clearTimeout(t);
  }, [activeChannel]);

  // Combined scroll handler: track stick-to-bottom + load history on scroll-to-top
  const onScroll = useCallback(() => {
    handleScroll();
    const el = ref.current;
    if (!el || el.scrollTop > 50) return;
    if (activeChannel !== 'server' && messages.length > 0) {
      const oldest = messages[0];
      if (!oldest.isSystem) {
        requestHistory(activeChannel, oldest.timestamp.toISOString());
      }
    }
  }, [activeChannel, messages, handleScroll]);

  const onNickClick = useCallback((nick: string, did: string | undefined, e: React.MouseEvent) => {
    setPopover({ nick, did, pos: { x: e.clientX, y: e.clientY } });
  }, []);

  return (
    <div key={activeChannel} ref={ref} data-testid="message-list" className="flex-1 overflow-y-auto relative" onScroll={onScroll}>
      {messages.length === 0 && (
        <div className="flex flex-col items-center justify-center h-full text-fg-dim px-8">
          <img src="/freeq.png" alt="freeq" className="w-14 h-14 mb-4 opacity-20" />
          {activeChannel === 'server' ? (
            <>
              <div className="text-base text-fg-muted font-medium">Welcome to freeq</div>
              <div className="text-sm mt-1 text-center">Server messages and notices will appear here.</div>
              <div className="text-xs mt-3 text-center space-y-1">
                <div><kbd className="px-1.5 py-0.5 text-xs bg-bg-tertiary border border-border rounded font-mono">⌘K</kbd> Quick switch · <kbd className="px-1.5 py-0.5 text-xs bg-bg-tertiary border border-border rounded font-mono">⌘/</kbd> Shortcuts</div>
              </div>
            </>
          ) : activeChannel.startsWith('#') ? (
            <>
              <div className="text-lg text-fg-muted font-medium">Welcome to {activeChannel}</div>
              <div className="text-sm mt-1 text-center">This is the beginning of the channel. Say hello! 👋</div>
            </>
          ) : (
            <>
              <div className="text-lg text-fg-muted font-medium">Conversation with {activeChannel}</div>
              <div className="text-sm mt-1 text-center">Messages are end-to-end between you two.</div>
            </>
          )}
        </div>
      )}
      <div className="pb-2">
        {messages.map((msg, i) => (
          <div key={msg.id}>
            {lastReadMsgId && i > 0 && messages[i - 1].id === lastReadMsgId && !msg.isSelf && (
              <div className="flex items-center gap-3 px-4 my-3">
                <div className="flex-1 h-px bg-danger/40" />
                <span className="text-xs font-bold text-danger/70 uppercase tracking-wider">New</span>
                <div className="flex-1 h-px bg-danger/40" />
              </div>
            )}
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

      {/* Scroll to bottom button */}
      {showScrollBtn && (
        <button
          onClick={() => {
            if (ref.current) {
              ref.current.scrollTop = ref.current.scrollHeight;
              stickToBottomRef.current = true;
              setShowScrollBtn(false);
            }
          }}
          className="absolute bottom-4 left-1/2 -translate-x-1/2 bg-bg-secondary border border-border rounded-full px-4 py-2 shadow-xl flex items-center gap-2 text-sm text-fg-muted hover:text-fg hover:border-accent transition-all z-10 animate-fadeIn"
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="currentColor">
            <path fillRule="evenodd" d="M8 1a.5.5 0 01.5.5v11.793l3.146-3.147a.5.5 0 01.708.708l-4 4a.5.5 0 01-.708 0l-4-4a.5.5 0 01.708-.708L7.5 13.293V1.5A.5.5 0 018 1z"/>
          </svg>
          New messages
        </button>
      )}

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
