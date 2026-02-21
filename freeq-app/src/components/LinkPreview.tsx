import { useEffect, useState } from 'react';

interface OGData {
  title?: string;
  description?: string;
  image?: string;
  siteName?: string;
}

// Simple cache to avoid re-fetching
const ogCache = new Map<string, OGData | null>();

async function fetchOG(url: string): Promise<OGData | null> {
  if (ogCache.has(url)) return ogCache.get(url) || null;
  try {
    // Use a CORS proxy or server endpoint. For now, try fetching directly
    // (works for some sites) and fall back to allorigins proxy.
    const proxyUrl = `https://api.allorigins.win/raw?url=${encodeURIComponent(url)}`;
    const resp = await fetch(proxyUrl, { signal: AbortSignal.timeout(5000) });
    if (!resp.ok) { ogCache.set(url, null); return null; }
    const html = await resp.text();

    const get = (prop: string): string | undefined => {
      // Match both property="og:X" and name="og:X"
      const re = new RegExp(`<meta[^>]*(?:property|name)=["']${prop}["'][^>]*content=["']([^"']*)["']`, 'i');
      const re2 = new RegExp(`<meta[^>]*content=["']([^"']*)["'][^>]*(?:property|name)=["']${prop}["']`, 'i');
      return re.exec(html)?.[1] || re2.exec(html)?.[1];
    };

    const data: OGData = {
      title: get('og:title') || get('twitter:title'),
      description: get('og:description') || get('twitter:description') || get('description'),
      image: get('og:image') || get('twitter:image'),
      siteName: get('og:site_name'),
    };

    // Only cache if we got something useful
    if (data.title || data.description || data.image) {
      ogCache.set(url, data);
      return data;
    }
    ogCache.set(url, null);
    return null;
  } catch {
    ogCache.set(url, null);
    return null;
  }
}

export function LinkPreview({ url }: { url: string }) {
  const [data, setData] = useState<OGData | null>(ogCache.get(url) || null);
  const [loading, setLoading] = useState(!ogCache.has(url));

  useEffect(() => {
    if (ogCache.has(url)) { setData(ogCache.get(url) || null); setLoading(false); return; }
    let cancelled = false;
    fetchOG(url).then((d) => {
      if (!cancelled) { setData(d); setLoading(false); }
    });
    return () => { cancelled = true; };
  }, [url]);

  if (loading || !data) return null;
  if (!data.title && !data.image) return null;

  const domain = (() => {
    try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return ''; }
  })();

  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className="block mt-1 max-w-md border border-border rounded-lg overflow-hidden hover:border-border-bright transition-colors bg-bg-secondary"
    >
      {data.image && (
        <img
          src={data.image}
          alt=""
          className="w-full h-32 object-cover"
          loading="lazy"
          onError={(e) => (e.currentTarget.style.display = 'none')}
        />
      )}
      <div className="px-3 py-2">
        {data.siteName && (
          <div className="text-[10px] text-fg-dim uppercase tracking-wider">{data.siteName}</div>
        )}
        {data.title && (
          <div className="text-xs font-semibold text-accent truncate">{data.title}</div>
        )}
        {data.description && (
          <div className="text-[11px] text-fg-muted line-clamp-2 mt-0.5">{data.description}</div>
        )}
        <div className="text-[10px] text-fg-dim mt-1 truncate">{domain}</div>
      </div>
    </a>
  );
}
