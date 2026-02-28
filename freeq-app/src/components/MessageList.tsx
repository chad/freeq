import { useEffect, useRef, useCallback, useState, useMemo, memo } from 'react';
import { useStore, type Message, type PinnedMessage } from '../store';
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

// Voice message pattern: 🎤 Voice message (0:05) https://...
const VOICE_MSG_RE = /🎤[^h]*(https?:\/\/\S+)/;
// Duration in voice message
const VOICE_DURATION_RE = /\((\d+:\d+)\)/;
// Video URL patterns
const VIDEO_URL_RE = /https?:\/\/[^\s<]+\.(?:mp4|mov|m4v|webm)(?:\?[^\s<]*)?/i;
// Audio URL patterns (file extension based)
const AUDIO_URL_RE = /https?:\/\/[^\s<]+\.(?:m4a|mp3|ogg|wav|aac)(?:\?[^\s<]*)?/i;
// PDS blob URL (for audio/video blobs)
const PDS_BLOB_RE = /https?:\/\/[^\s]+\/xrpc\/com\.atproto\.sync\.getBlob[^\s]*/i;
// Proxy blob URL with mime hint
const PROXY_VIDEO_RE = /https?:\/\/[^\s]+\/api\/v1\/blob\?[^\s]*mime=video%2F[^\s]*/i;
const PROXY_AUDIO_RE = /https?:\/\/[^\s]+\/api\/v1\/blob\?[^\s]*mime=audio%2F[^\s]*/i;

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

/** Parse text into typed segments for safe React rendering (no dangerouslySetInnerHTML). */
interface TextSegment {
  type: 'text' | 'link' | 'code' | 'codeblock' | 'bold' | 'italic' | 'strike';
  content: string;
  href?: string;
}

function parseTextSegments(text: string): TextSegment[] {
  const segments: TextSegment[] = [];
  // Tokenize by splitting on markdown patterns
  // Order matters: code blocks first, then inline code, then other formatting
  const patterns: { re: RegExp; type: TextSegment['type']; group: number }[] = [
    { re: /```([\s\S]*?)```/g, type: 'codeblock', group: 1 },
    { re: /`([^`]+)`/g, type: 'code', group: 1 },
    { re: /(https?:\/\/[^\s<]+)/g, type: 'link', group: 1 },
    { re: /\*\*(.+?)\*\*/g, type: 'bold', group: 1 },
    { re: /(?<!\*)\*([^*]+)\*(?!\*)/g, type: 'italic', group: 1 },
    { re: /~~(.+?)~~/g, type: 'strike', group: 1 },
  ];

  // Build a combined list of all matches with positions
  const matches: { start: number; end: number; type: TextSegment['type']; content: string; full: string }[] = [];
  for (const p of patterns) {
    p.re.lastIndex = 0;
    let m;
    while ((m = p.re.exec(text)) !== null) {
      matches.push({
        start: m.index,
        end: m.index + m[0].length,
        type: p.type,
        content: m[p.group],
        full: m[0],
      });
    }
  }

  // Sort by start position, remove overlapping
  matches.sort((a, b) => a.start - b.start);
  const filtered: typeof matches = [];
  let lastEnd = 0;
  for (const m of matches) {
    if (m.start >= lastEnd) {
      filtered.push(m);
      lastEnd = m.end;
    }
  }

  // Build segments
  let pos = 0;
  for (const m of filtered) {
    if (m.start > pos) {
      segments.push({ type: 'text', content: text.slice(pos, m.start) });
    }
    if (m.type === 'link') {
      segments.push({ type: 'link', content: m.content, href: m.content });
    } else {
      segments.push({ type: m.type, content: m.content });
    }
    pos = m.end;
  }
  if (pos < text.length) {
    segments.push({ type: 'text', content: text.slice(pos) });
  }

  return segments;
}

// ── Segment parse cache (avoids re-parsing on every render) ──

const _segmentCache = new Map<string, TextSegment[]>();
const SEGMENT_CACHE_MAX = 2000;

function parseTextSegmentsCached(text: string): TextSegment[] {
  const cached = _segmentCache.get(text);
  if (cached) return cached;
  const segments = parseTextSegments(text);
  if (_segmentCache.size >= SEGMENT_CACHE_MAX) {
    // Evict oldest half
    const keys = [..._segmentCache.keys()];
    for (let i = 0; i < keys.length / 2; i++) _segmentCache.delete(keys[i]);
  }
  _segmentCache.set(text, segments);
  return segments;
}

/** Render text segments as React elements (XSS-safe — no innerHTML). */
function renderTextSafe(text: string): React.ReactElement {
  const segments = parseTextSegmentsCached(text);
  return (
    <>
      {segments.map((seg, i) => {
        switch (seg.type) {
          case 'link':
            return <a key={i} href={seg.href} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline break-all">{seg.content}</a>;
          case 'codeblock':
            return <pre key={i} className="bg-surface rounded px-2 py-1.5 my-1 text-[13px] font-mono overflow-x-auto">{seg.content}</pre>;
          case 'code':
            return <code key={i} className="bg-surface px-1 py-0.5 rounded text-[13px] font-mono text-pink">{seg.content}</code>;
          case 'bold':
            return <strong key={i}>{seg.content}</strong>;
          case 'italic':
            return <em key={i}>{seg.content}</em>;
          case 'strike':
            return <del key={i} className="text-fg-dim">{seg.content}</del>;
          default:
            return <span key={i}>{seg.content}</span>;
        }
      })}
    </>
  );
}



// ── External image gating ──

/** Trusted domains that always load inline (our own infrastructure). */
function isTrustedImageUrl(url: string): boolean {
  try {
    const u = new URL(url);
    const h = u.hostname;
    return h === 'cdn.bsky.app' || h.endsWith('.bsky.app') || h.endsWith('.bsky.network')
      || h === 'freeq.at' || h.endsWith('.freeq.at') || h === 'localhost';
  } catch {
    return false;
  }
}

/** Image that respects the "Load external media" setting. */
function GatedImage({ url, onOpen }: { url: string; onOpen: () => void }) {
  const loadMedia = useStore((s) => s.loadExternalMedia);
  const [revealed, setRevealed] = useState(false);
  const trusted = isTrustedImageUrl(url);

  if (trusted || loadMedia || revealed) {
    return (
      <button onClick={onOpen} className="block cursor-zoom-in">
        <img
          src={url}
          alt=""
          className="max-w-sm max-h-80 rounded-lg border border-border object-contain bg-bg-tertiary hover:opacity-90 transition-opacity"
          loading="lazy"
          onError={(e) => { e.currentTarget.style.display = 'none'; }}
        />
      </button>
    );
  }

  return (
    <button
      onClick={() => setRevealed(true)}
      className="flex items-center gap-2 px-3 py-2 rounded-lg border border-border bg-bg-tertiary text-fg-dim text-sm hover:bg-surface hover:text-fg-muted transition-colors"
      title={url}
    >
      <span className="text-lg">🖼</span>
      <span>Click to load external image</span>
    </button>
  );
}

// ── Message content (text + inline images) ──

// Bluesky post URL pattern
const BSKY_POST_RE = /https?:\/\/bsky\.app\/profile\/([^/]+)\/post\/([a-zA-Z0-9]+)/;
// YouTube URL pattern  
const YT_RE = /(?:youtube\.com\/watch\?v=|youtu\.be\/)([a-zA-Z0-9_-]{11})/;

/** Inline audio player for voice messages and audio files */
function InlineAudioPlayer({ url, label }: { url: string; label?: string }) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [progress, setProgress] = useState(0);
  const [duration, setDuration] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(false);

  const toggle = () => {
    const el = audioRef.current;
    if (!el) return;
    if (playing) {
      el.pause();
      setPlaying(false);
      return;
    }
    setLoading(true);
    setError(false);
    el.play()
      .then(() => { setPlaying(true); setLoading(false); })
      .catch(() => { setError(true); setLoading(false); });
  };

  const fmt = (s: number) => {
    if (!s || !isFinite(s)) return '0:00';
    return `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;
  };

  return (
    <div className="mt-1.5 flex items-center gap-3 bg-bg-tertiary border border-border rounded-xl px-3 py-2.5 max-w-[300px]">
      <button
        onClick={toggle}
        disabled={loading}
        className={`flex-shrink-0 w-10 h-10 rounded-full flex items-center justify-center transition ${
          error ? 'bg-red-500 hover:bg-red-600' : 'bg-accent hover:brightness-110'
        }`}
      >
        {loading ? (
          <svg className="w-5 h-5 text-white animate-spin" fill="none" viewBox="0 0 24 24">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4"/>
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"/>
          </svg>
        ) : error ? (
          <svg className="w-4 h-4 text-white" fill="currentColor" viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z"/></svg>
        ) : playing ? (
          <svg className="w-4 h-4 text-white" fill="currentColor" viewBox="0 0 24 24"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>
        ) : (
          <svg className="w-4 h-4 text-white ml-0.5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
        )}
      </button>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-1.5 text-xs text-fg-muted mb-1">
          <svg className="w-3 h-3 text-accent" fill="currentColor" viewBox="0 0 24 24"><path d="M12 14c1.66 0 3-1.34 3-3V5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3zm-1-9c0-.55.45-1 1-1s1 .45 1 1v6c0 .55-.45 1-1 1s-1-.45-1-1V5z"/><path d="M17 11c0 2.76-2.24 5-5 5s-5-2.24-5-5H5c0 3.53 2.61 6.43 6 6.92V21h2v-3.08c3.39-.49 6-3.39 6-6.92h-2z"/></svg>
          <span className="font-medium text-fg-secondary">Voice message</span>
        </div>
        <div className="relative h-1 bg-bg-hover rounded-full overflow-hidden">
          <div
            className="absolute left-0 top-0 h-full bg-accent rounded-full transition-all"
            style={{ width: duration > 0 ? `${(progress / duration) * 100}%` : '0%' }}
          />
        </div>
        <div className="flex justify-between mt-1 text-[10px] text-fg-muted font-mono">
          <span>{fmt(playing ? progress : 0)}</span>
          <span>{label || fmt(duration)}</span>
        </div>
      </div>
      <audio
        ref={audioRef}
        src={url}
        preload="metadata"
        onLoadedMetadata={() => setDuration(audioRef.current?.duration || 0)}
        onTimeUpdate={() => setProgress(audioRef.current?.currentTime || 0)}
        onEnded={() => { setPlaying(false); setProgress(0); }}
        onError={() => { setError(true); setPlaying(false); setLoading(false); }}
      />
    </div>
  );
}

/** Inline video player */
function InlineVideoPlayer({ url }: { url: string }) {
  return (
    <div className="mt-1.5 max-w-sm">
      <video
        src={url}
        controls
        preload="metadata"
        className="rounded-lg border border-border max-h-72 bg-black"
        playsInline
      />
    </div>
  );
}

function MessageContent({ msg }: { msg: Message }) {
  const setLightbox = useStore((s) => s.setLightboxUrl);

  if (msg.isAction) {
    const color = msg.isSelf ? '#b18cff' : nickColor(msg.from);
    return (
      <div className="text-fg-muted italic text-[15px] mt-0.5">
        <span style={{ color }} className="font-semibold not-italic">{'* '}{msg.from}</span>{' '}{msg.text}
      </div>
    );
  }

  // Voice messages — check first before image extraction
  const voiceMatch = msg.text.match(VOICE_MSG_RE);
  if (voiceMatch) {
    const durationMatch = msg.text.match(VOICE_DURATION_RE);
    let audioUrl = voiceMatch[1];
    // Rewrite old cdn.bsky.app/img/ URLs to proxy through our server
    const cdnMatch = audioUrl.match(/cdn\.bsky\.app\/img\/[^/]+\/plain\/([^/]+)\/([^@\s]+)/);
    if (cdnMatch) {
      const pdsUrl = `https://bsky.social/xrpc/com.atproto.sync.getBlob?did=${cdnMatch[1]}&cid=${cdnMatch[2]}`;
      audioUrl = `/api/v1/blob?url=${encodeURIComponent(pdsUrl)}`;
    }
    // Proxy PDS blob URLs too
    if (audioUrl.includes('/xrpc/com.atproto.sync.getBlob')) {
      audioUrl = `/api/v1/blob?url=${encodeURIComponent(audioUrl)}`;
    }
    return (
      <div className="mt-0.5">
        {msg.replyTo && <ReplyBadge msgId={msg.replyTo} />}
        <InlineAudioPlayer url={audioUrl} label={durationMatch?.[1]} />
      </div>
    );
  }

  // Video URLs (file extension or proxy with video mime hint)
  const videoMatch = msg.text.match(VIDEO_URL_RE) || msg.text.match(PROXY_VIDEO_RE);
  if (videoMatch) {
    const cleanText = msg.text.replace(videoMatch[0], '').trim();
    return (
      <div className="mt-0.5">
        {msg.replyTo && <ReplyBadge msgId={msg.replyTo} />}
        {cleanText && <div className="text-[15px] leading-relaxed mb-1">{renderTextSafe(cleanText)}</div>}
        <InlineVideoPlayer url={videoMatch[0]} />
      </div>
    );
  }

  // Audio URLs (file extension, proxy with audio mime hint, or PDS blob)
  const audioMatch = msg.text.match(AUDIO_URL_RE) || msg.text.match(PROXY_AUDIO_RE) || msg.text.match(PDS_BLOB_RE);
  if (audioMatch && !msg.text.match(IMAGE_URL_RE) && !msg.text.match(CDN_IMAGE_RE)) {
    const cleanText = msg.text.replace(audioMatch[0], '').trim();
    return (
      <div className="mt-0.5">
        {msg.replyTo && <ReplyBadge msgId={msg.replyTo} />}
        {cleanText && <div className="text-[15px] leading-relaxed mb-1">{renderTextSafe(cleanText)}</div>}
        <InlineAudioPlayer url={audioMatch[0]} />
      </div>
    );
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
        <div className="text-[15px] leading-relaxed [&_pre]:my-1 [&_a]:break-all">
          {renderTextSafe(cleanText)}
        </div>
      )}

      {/* Inline images */}
      {imageUrls.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-2">
          {imageUrls.map((url) => (
            <GatedImage key={url} url={url} onOpen={() => setLightbox(url)} />
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
          rel="noopener noreferrer"
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
        // Skip blob proxy URLs, audio/video URLs — they're media, not web pages
        if (urlMatch && /\/api\/v1\/blob|\.(?:m4a|mp3|mp4|mov|webm|ogg|wav|aac)/i.test(urlMatch[1])) return null;
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
    <button
      onClick={() => useStore.getState().setScrollToMsgId(msgId)}
      className="flex items-center gap-2 text-sm text-fg-dim mb-1.5 pl-2 border-l-2 border-accent/30 hover:bg-accent/5 rounded-r cursor-pointer w-full text-left"
    >
      <span className="font-semibold text-fg-muted">{original.from}</span>
      <span className="truncate max-w-[300px]">{original.text}</span>
    </button>
  );
}

// ── Message grouping ──

function isGrouped(msgs: Message[], i: number): boolean {
  if (i === 0) return false;
  const prev = msgs[i - 1];
  const curr = msgs[i];
  if (prev.isSystem || curr.isSystem || prev.deleted || curr.deleted) return false;
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
        {renderTextSafe(msg.text)}
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
      className={`msg-full group px-4 pt-3 pb-1 hover:bg-white/[0.02] flex gap-3 relative ${
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
          {msg.tags['+freeq.at/sig'] && <SignedBadge />}
          {member?.away != null && (
            <span className="text-xs text-fg-dim bg-warning/10 text-warning px-1.5 py-0.5 rounded">away</span>
          )}
          <span className="text-xs text-fg-dim whitespace-nowrap cursor-default" title={msg.timestamp.toLocaleString([], { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })}>{formatTime(msg.timestamp)}</span>
          {msg.editOf && <span className="text-xs text-fg-dim">(edited)</span>}
          {msg.encrypted && <EncryptedBadge />}
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

/** Signed message badge — message has a server-attested cryptographic signature */
function SignedBadge() {
  const [showInfo, setShowInfo] = useState(false);
  return (
    <span className="relative inline-block">
      <button
        className="text-success text-xs opacity-60 hover:opacity-100 transition-opacity"
        onClick={(e) => { e.stopPropagation(); setShowInfo(!showInfo); }}
        title="Cryptographically signed message"
      >
        <svg className="w-3 h-3 inline -mt-0.5" viewBox="0 0 16 16" fill="currentColor">
          <path d="M8 1a2 2 0 00-2 2v3H5a2 2 0 00-2 2v5a2 2 0 002 2h6a2 2 0 002-2V8a2 2 0 00-2-2H10V3a2 2 0 00-2-2zm0 1.5a.5.5 0 01.5.5v3h-1V3a.5.5 0 01.5-.5z"/>
        </svg>
      </button>
      {showInfo && (
        <div className="absolute bottom-full left-0 mb-1 w-64 bg-bg-secondary border border-border rounded-lg shadow-xl p-3 z-50 animate-fadeIn"
             onClick={(e) => e.stopPropagation()}>
          <div className="text-xs font-semibold text-success mb-1 flex items-center gap-1">
            <svg className="w-3 h-3" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 1a2 2 0 00-2 2v3H5a2 2 0 00-2 2v5a2 2 0 002 2h6a2 2 0 002-2V8a2 2 0 00-2-2H10V3a2 2 0 00-2-2zm0 1.5a.5.5 0 01.5.5v3h-1V3a.5.5 0 01.5-.5z"/>
            </svg>
            Signed Message
          </div>
          <p className="text-[11px] text-fg-muted leading-relaxed">
            This message is cryptographically signed by the sender&apos;s verified identity.
            It cannot be forged or tampered with — the signature is tied to their AT Protocol DID.
          </p>
          <button
            className="text-[10px] text-fg-dim hover:text-fg-muted mt-1.5"
            onClick={() => setShowInfo(false)}
          >
            Dismiss
          </button>
        </div>
      )}
    </span>
  );
}

function EncryptedBadge() {
  const [showInfo, setShowInfo] = useState(false);
  return (
    <span className="relative inline-block">
      <button
        className="text-[10px] text-success hover:opacity-80 transition-opacity"
        onClick={(e) => { e.stopPropagation(); setShowInfo(!showInfo); }}
        title="End-to-end encrypted"
      >
        🔒
      </button>
      {showInfo && (
        <div className="absolute bottom-full left-0 mb-1 w-64 bg-bg-secondary border border-border rounded-lg shadow-xl p-3 z-50 animate-fadeIn"
             onClick={(e) => e.stopPropagation()}>
          <div className="text-xs font-semibold text-success mb-1">🔒 End-to-End Encrypted</div>
          <p className="text-[11px] text-fg-muted leading-relaxed">
            This message is end-to-end encrypted. Only you and the recipient can read it —
            the server only sees ciphertext. Uses the Double Ratchet protocol (like Signal)
            with forward secrecy.
          </p>
          <button
            className="text-[10px] text-fg-dim hover:text-fg-muted mt-1.5"
            onClick={() => setShowInfo(false)}
          >
            Dismiss
          </button>
        </div>
      )}
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

// ── Typing indicator ──

function TypingIndicatorBar({ channel }: { channel: string }) {
  const channels = useStore((s) => s.channels);
  const ch = channels.get(channel.toLowerCase());
  if (!ch) return null;

  const typers = [...ch.members.values()].filter((m) => m.typing).map((m) => m.nick);
  if (typers.length === 0) return null;

  const text = typers.length === 1
    ? `${typers[0]} is typing`
    : typers.length === 2
    ? `${typers[0]} and ${typers[1]} are typing`
    : `${typers[0]} and ${typers.length - 1} others are typing`;

  return (
    <div className="px-4 py-1.5 flex items-center gap-2 text-xs text-fg-dim animate-fadeIn" aria-live="polite" aria-atomic="true">
      <span className="flex gap-0.5">
        <span className="w-1.5 h-1.5 rounded-full bg-accent animate-bounce" style={{ animationDelay: '0ms' }} />
        <span className="w-1.5 h-1.5 rounded-full bg-accent animate-bounce" style={{ animationDelay: '150ms' }} />
        <span className="w-1.5 h-1.5 rounded-full bg-accent animate-bounce" style={{ animationDelay: '300ms' }} />
      </span>
      <span className="text-fg-muted">{text}</span>
    </div>
  );
}

// ── Main export ──

/** Pinned messages bar — shows at the top of the channel message area. */
function ChannelEmptyState({ channel }: { channel: string }) {
  const ch = useStore((s) => s.channels.get(channel.toLowerCase()));
  const topic = ch?.topic;
  const memberCount = ch?.members.size ?? 0;
  const isEncrypted = ch?.isEncrypted;

  return (
    <>
      <div className="text-3xl mb-2">👋</div>
      <div className="text-xl text-fg font-bold">Welcome to {channel}</div>

      {topic && (
        <div className="text-sm mt-2 text-center max-w-md leading-relaxed text-fg-muted">
          {topic}
        </div>
      )}

      {!topic && (
        <div className="text-sm mt-2 text-center max-w-xs leading-relaxed text-fg-dim">
          This is the beginning of <span className="text-accent font-medium">{channel}</span>.
        </div>
      )}

      {/* Channel features */}
      <div className="flex flex-wrap justify-center gap-2 mt-4 text-[11px]">
        {memberCount > 0 && (
          <span className="bg-bg-tertiary border border-border rounded-full px-2.5 py-1 text-fg-dim">
            👥 {memberCount} {memberCount === 1 ? 'member' : 'members'}
          </span>
        )}
        {isEncrypted && (
          <span className="bg-success/5 border border-success/20 rounded-full px-2.5 py-1 text-success">
            🔒 Encrypted
          </span>
        )}
        <span className="bg-bg-tertiary border border-border rounded-full px-2.5 py-1 text-fg-dim">
          ✍️ Messages are signed
        </span>
      </div>

      {/* Info cards */}
      <div className="grid gap-2 mt-5 max-w-sm w-full">
        <div className="bg-bg-tertiary/50 border border-border rounded-lg p-3 text-left">
          <div className="text-[11px] font-semibold text-fg-muted mb-0.5">🔐 Verified Identity</div>
          <div className="text-[11px] text-fg-dim leading-relaxed">
            Users with a <span className="text-accent">✓</span> next to their name are signed in with their AT Protocol (Bluesky) identity. Their messages are cryptographically signed and can&apos;t be forged.
          </div>
        </div>
        <div className="bg-bg-tertiary/50 border border-border rounded-lg p-3 text-left">
          <div className="text-[11px] font-semibold text-fg-muted mb-0.5">💬 Getting started</div>
          <div className="text-[11px] text-fg-dim leading-relaxed">
            Type a message below to start chatting. Use <kbd className="px-1 py-0.5 bg-bg border border-border rounded text-[10px] font-mono">/help</kbd> for commands, or right-click messages for actions.
          </div>
        </div>
      </div>

      <div className="flex gap-2 mt-4">
        <button onClick={() => {
          navigator.clipboard.writeText(`https://irc.freeq.at/join/${encodeURIComponent(channel)}`);
          import('./Toast').then(m => m.showToast('Invite link copied', 'success', 2000));
        }} className="text-xs bg-bg-tertiary border border-border rounded-lg px-3 py-1.5 text-fg-dim hover:text-fg hover:border-accent transition-colors">
          🔗 Copy invite link
        </button>
      </div>
    </>
  );
}

const EMPTY_PINS: PinnedMessage[] = [];

function PinnedBar({ pins, messages }: { pins: PinnedMessage[]; messages: Message[] }) {
  const [expanded, setExpanded] = useState(false);
  if (pins.length === 0) return null;

  // Find the actual message content for each pin
  const pinnedMsgs = pins.slice(0, expanded ? 10 : 1).map((pin) => {
    const msg = messages.find((m) => m.id === pin.msgid);
    return { ...pin, msg };
  });

  return (
    <div className="border-b border-border bg-bg-secondary/50 px-4 py-1.5 text-sm">
      <div className="flex items-center gap-2">
        <span className="text-accent text-xs">📌</span>
        {pinnedMsgs[0]?.msg ? (
          <button
            className="flex-1 text-left truncate text-fg-muted hover:text-fg transition-colors"
            onClick={() => {
              useStore.getState().setScrollToMsgId(pinnedMsgs[0].msgid);
            }}
          >
            <span className="font-semibold text-fg text-xs">{pinnedMsgs[0].msg.from}</span>
            <span className="ml-1.5 text-xs">{pinnedMsgs[0].msg.text.slice(0, 120)}{pinnedMsgs[0].msg.text.length > 120 ? '…' : ''}</span>
          </button>
        ) : (
          <span className="flex-1 text-fg-dim text-xs italic">Pinned message not in view</span>
        )}
        {pins.length > 1 && (
          <button
            className="text-[10px] text-fg-dim hover:text-fg shrink-0"
            onClick={() => setExpanded(!expanded)}
          >
            {expanded ? '▲' : `+${pins.length - 1} more`}
          </button>
        )}
      </div>
      {expanded && pinnedMsgs.slice(1).map((p) => (
        <div key={p.msgid} className="flex items-center gap-2 mt-1">
          <span className="text-accent text-xs">📌</span>
          {p.msg ? (
            <button
              className="flex-1 text-left truncate text-fg-muted hover:text-fg text-xs"
              onClick={() => useStore.getState().setScrollToMsgId(p.msgid)}
            >
              <span className="font-semibold text-fg">{p.msg.from}</span>
              <span className="ml-1.5">{p.msg.text.slice(0, 100)}</span>
            </button>
          ) : (
            <span className="flex-1 text-fg-dim text-xs italic">Message {p.msgid.slice(0, 8)}…</span>
          )}
        </div>
      ))}
    </div>
  );
}

export function MessageList() {
  const activeChannel = useStore((s) => s.activeChannel);
  const messages = useStore((s) => {
    if (s.activeChannel === 'server') return s.serverMessages;
    return s.channels.get(s.activeChannel.toLowerCase())?.messages || [];
  });
  const lastReadMsgId = useStore((s) => s.channels.get(s.activeChannel.toLowerCase())?.lastReadMsgId);
  const pins = useStore((s) => s.channels.get(s.activeChannel.toLowerCase())?.pins ?? EMPTY_PINS);
  const density = useStore((s) => s.messageDensity);
  const ref = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);
  const [showScrollBtn, setShowScrollBtn] = useState(false);
  const [newMsgCount, setNewMsgCount] = useState(0);
  const [popover, setPopover] = useState<{ nick: string; did?: string; pos: { x: number; y: number } } | null>(null);

  // Track whether user has scrolled up (unstick from bottom)
  const handleScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    stickToBottomRef.current = atBottom;
    setShowScrollBtn(!atBottom);
    if (atBottom) setNewMsgCount(0);
  }, []);

  // Scroll to bottom when messages change (if stuck to bottom), or count new messages
  const prevLenRef = useRef(messages.length);
  useEffect(() => {
    const added = messages.length - prevLenRef.current;
    prevLenRef.current = messages.length;
    if (!stickToBottomRef.current) {
      if (added > 0) setNewMsgCount((c) => c + added);
      return;
    }
    const scrollBottom = () => {
      if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
    };
    // Double RAF ensures layout is complete after React render
    requestAnimationFrame(() => requestAnimationFrame(scrollBottom));
  }, [messages.length, messages]);

  // Always scroll to bottom on channel switch
  // Multiple timers to catch: initial render, layout, CHATHISTORY load
  useEffect(() => {
    stickToBottomRef.current = true;
    setShowScrollBtn(false);
    setNewMsgCount(0);
    prevLenRef.current = 0;
    const scrollBottom = () => {
      if (ref.current) {
        ref.current.scrollTop = ref.current.scrollHeight;
        stickToBottomRef.current = true;
      }
    };
    scrollBottom();
    requestAnimationFrame(() => requestAnimationFrame(scrollBottom));
    const t1 = setTimeout(scrollBottom, 100);
    const t2 = setTimeout(scrollBottom, 300);
    const t3 = setTimeout(scrollBottom, 600); // after CHATHISTORY arrives
    const t4 = setTimeout(scrollBottom, 1200); // slow networks
    return () => { clearTimeout(t1); clearTimeout(t2); clearTimeout(t3); clearTimeout(t4); };
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

  // Scroll to a specific message (from search, reply click, etc.)
  const scrollToMsgId = useStore((s) => s.scrollToMsgId);
  const [highlightId, setHighlightId] = useState<string | null>(null);
  useEffect(() => {
    if (!scrollToMsgId) return;
    useStore.getState().setScrollToMsgId(null);
    // Wait for render, then scroll
    requestAnimationFrame(() => {
      const el = document.getElementById(`msg-${scrollToMsgId}`);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        setHighlightId(scrollToMsgId);
        setTimeout(() => setHighlightId(null), 2000);
      }
    });
  }, [scrollToMsgId]);

  // Show brief skeleton on channel switch while CHATHISTORY loads
  const [showSkeleton, setShowSkeleton] = useState(false);
  useEffect(() => {
    if (activeChannel === 'server') return;
    setShowSkeleton(true);
    const t = setTimeout(() => setShowSkeleton(false), 600);
    return () => clearTimeout(t);
  }, [activeChannel]);

  const onNickClick = useCallback((nick: string, did: string | undefined, e: React.MouseEvent) => {
    setPopover({ nick, did, pos: { x: e.clientX, y: e.clientY } });
  }, []);

  return (
    <div key={activeChannel} ref={ref} data-testid="message-list" role="log" aria-label={`Messages in ${activeChannel}`} aria-live="polite" className={`flex-1 overflow-y-auto relative ${
      density === 'compact' ? 'text-[14px] [&_.msg-full]:pt-1.5 [&_.msg-full]:pb-0' :
      density === 'cozy' ? 'text-[16px] [&_.msg-full]:pt-4 [&_.msg-full]:pb-2' : ''
    }`} onScroll={onScroll}>
      {activeChannel.startsWith('#') && pins.length > 0 && (
        <div className="sticky top-0 z-10">
          <PinnedBar pins={pins} messages={messages} />
        </div>
      )}
      {messages.length === 0 && showSkeleton && activeChannel !== 'server' && (
        <div className="px-4 pt-4 space-y-4 animate-pulse">
          {[...Array(6)].map((_, i) => (
            <div key={i} className="flex gap-3">
              <div className="w-10 h-10 rounded-full bg-surface shrink-0" />
              <div className="flex-1 space-y-2 pt-1">
                <div className="flex gap-2">
                  <div className="h-3 w-20 bg-surface rounded" />
                  <div className="h-3 w-12 bg-surface/50 rounded" />
                </div>
                <div className="h-3 bg-surface/70 rounded" style={{ width: `${40 + Math.random() * 50}%` }} />
              </div>
            </div>
          ))}
        </div>
      )}
      {messages.length === 0 && !showSkeleton && (
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
            <ChannelEmptyState channel={activeChannel} />
          ) : (
            <>
              <div className="text-3xl mb-2">💬</div>
              <div className="text-xl text-fg font-bold">Conversation with {activeChannel}</div>
              <div className="text-sm mt-2 text-center max-w-xs leading-relaxed text-fg-dim">
                Direct messages are private between you and <span className="text-fg-muted">{activeChannel}</span>.
              </div>
            </>
          )}
        </div>
      )}
      <div className="pb-2">
        {messages.map((msg, i) => {
          // Collapse consecutive join/part/quit system messages
          const isJoinPart = msg.isSystem && /^.+ (joined|left)$/.test(msg.text);
          if (isJoinPart) {
            // Skip if the previous message was also a join/part (we'll render them as a group)
            const prev = i > 0 ? messages[i - 1] : null;
            const prevIsJP = prev?.isSystem && /^.+ (joined|left)$/.test(prev.text);
            const next = i < messages.length - 1 ? messages[i + 1] : null;
            const nextIsJP = next?.isSystem && /^.+ (joined|left)$/.test(next.text);
            if (prevIsJP) return null; // skip — rendered by the first in the group
            if (nextIsJP) {
              // First in a group — collect all consecutive
              const group: Message[] = [msg];
              for (let j = i + 1; j < messages.length; j++) {
                const m = messages[j];
                if (m.isSystem && /^.+ (joined|left)$/.test(m.text)) group.push(m);
                else break;
              }
              const joins = group.filter(m => m.text.endsWith(' joined')).map(m => m.text.replace(' joined', ''));
              const parts = group.filter(m => m.text.endsWith(' left')).map(m => m.text.replace(' left', ''));
              const parts_list: string[] = [];
              if (joins.length > 0) parts_list.push(`${joins.slice(0, 3).join(', ')}${joins.length > 3 ? ` and ${joins.length - 3} more` : ''} joined`);
              if (parts.length > 0) parts_list.push(`${parts.slice(0, 3).join(', ')}${parts.length > 3 ? ` and ${parts.length - 3} more` : ''} left`);
              return (
                <div key={msg.id} id={`msg-${msg.id}`} className="px-4 py-0.5 flex items-start gap-3">
                  <span className="w-10 shrink-0" />
                  <span className="text-fg-dim text-xs opacity-60">— {parts_list.join('; ')}</span>
                </div>
              );
            }
          }
          return (
          <div key={msg.id} id={`msg-${msg.id}`} className={highlightId === msg.id ? 'bg-accent/10 transition-colors duration-1000' : ''}>
            {lastReadMsgId && i > 0 && messages[i - 1].id === lastReadMsgId && !msg.isSelf && (
              <div className="flex items-center gap-3 px-4 my-3" id="unread-marker">
                <div className="flex-1 h-px bg-danger/40" />
                <span className="text-xs font-bold text-danger/70 uppercase tracking-wider">New</span>
                <div className="flex-1 h-px bg-danger/40" />
              </div>
            )}
            {shouldShowDateSep(messages, i) && <DateSeparator date={msg.timestamp} />}
            {msg.deleted ? (
              <div className="px-4 py-0.5 text-xs italic text-[var(--text-muted)] opacity-50">
                Message from {msg.from} deleted
              </div>
            ) : msg.isSystem ? (
              <SystemMessage msg={msg} />
            ) : isGrouped(messages, i) ? (
              <GroupedMessage msg={msg} channel={activeChannel} onNickClick={onNickClick} />
            ) : (
              <FullMessage msg={msg} channel={activeChannel} onNickClick={onNickClick} />
            )}
          </div>
          );
        })}
        <TypingIndicatorBar channel={activeChannel} />
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
          {newMsgCount > 0 ? `${newMsgCount} new message${newMsgCount === 1 ? '' : 's'}` : 'Jump to bottom'}
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
