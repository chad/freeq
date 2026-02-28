import { useState, useEffect } from 'react';

interface BskyPost {
  text: string;
  author: { handle: string; displayName?: string; avatar?: string };
  createdAt: string;
  likeCount?: number;
  repostCount?: number;
  images?: { thumb: string; alt?: string }[];
}

export function BlueskyEmbed({ handle, rkey }: { handle: string; rkey: string }) {
  const [post, setPost] = useState<BskyPost | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        // Resolve handle to DID if needed, then fetch post
        const uri = `at://${handle}/app.bsky.feed.post/${rkey}`;
        const resp = await fetch(
          `https://public.api.bsky.app/xrpc/app.bsky.feed.getPostThread?uri=${encodeURIComponent(uri)}&depth=0`
        );
        if (!resp.ok) throw new Error('fetch failed');
        const data = await resp.json();
        const p = data.thread?.post;
        if (!p || cancelled) return;

        const images = p.embed?.images?.map((img: any) => ({
          thumb: img.thumb,
          alt: img.alt,
        })) || p.embed?.media?.images?.map((img: any) => ({
          thumb: img.thumb,
          alt: img.alt,
        })) || [];

        setPost({
          text: p.record?.text || '',
          author: {
            handle: p.author?.handle || handle,
            displayName: p.author?.displayName,
            avatar: p.author?.avatar,
          },
          createdAt: p.record?.createdAt || '',
          likeCount: p.likeCount,
          repostCount: p.repostCount,
          images,
        });
      } catch {
        if (!cancelled) setError(true);
      }
    })();
    return () => { cancelled = true; };
  }, [handle, rkey]);

  if (error || !post) return null;

  const time = post.createdAt ? new Date(post.createdAt).toLocaleDateString(undefined, {
    month: 'short', day: 'numeric', year: 'numeric'
  }) : '';

  return (
    <a
      href={`https://bsky.app/profile/${handle}/post/${rkey}`}
      target="_blank"
      rel="noopener noreferrer"
      className="mt-2 block max-w-sm rounded-xl border border-border bg-bg-tertiary hover:border-accent/30 transition-colors overflow-hidden"
    >
      {/* Author */}
      <div className="flex items-center gap-2 px-3 pt-3 pb-1">
        {post.author.avatar ? (
          <img src={post.author.avatar} alt="" className="w-5 h-5 rounded-full" />
        ) : (
          <div className="w-5 h-5 rounded-full bg-surface flex items-center justify-center text-[10px] text-fg-dim">
            {(post.author.handle[0] || '?').toUpperCase()}
          </div>
        )}
        <span className="text-xs font-semibold text-fg">{post.author.displayName || post.author.handle}</span>
        <span className="text-[10px] text-fg-dim">@{post.author.handle}</span>
      </div>

      {/* Text */}
      <div className="px-3 py-1 text-sm text-fg leading-relaxed line-clamp-4">
        {post.text}
      </div>

      {/* Images */}
      {post.images && post.images.length > 0 && (
        <div className="px-3 pb-2 flex gap-1">
          {post.images.slice(0, 4).map((img, i) => (
            <img key={i} src={img.thumb} alt={img.alt || ''} className="rounded-lg max-h-40 object-cover flex-1 min-w-0" />
          ))}
        </div>
      )}

      {/* Footer */}
      <div className="px-3 py-2 border-t border-border/50 flex items-center gap-3 text-[10px] text-fg-dim">
        <span className="flex items-center gap-1">
          <svg className="w-3 h-3" viewBox="0 0 16 16" fill="currentColor" opacity="0.5">
            <path d="M8 14s-5-3.5-5-7.5S5.5 1 8 4c2.5-3 5-2 5 2.5S8 14 8 14z"/>
          </svg>
          {post.likeCount || 0}
        </span>
        <span className="flex items-center gap-1">
          <svg className="w-3 h-3" viewBox="0 0 16 16" fill="currentColor" opacity="0.5">
            <path d="M2 5h3V2l5 5-5 5V9H2V5zm12 6h-3v3L6 9l5-5v3h3v4z"/>
          </svg>
          {post.repostCount || 0}
        </span>
        <span className="ml-auto">🦋 {time}</span>
      </div>
    </a>
  );
}
