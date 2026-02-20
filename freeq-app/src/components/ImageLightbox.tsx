import { useEffect } from 'react';
import { useStore } from '../store';

export function ImageLightbox() {
  const url = useStore((s) => s.lightboxUrl);
  const close = useStore((s) => s.setLightboxUrl);

  useEffect(() => {
    if (!url) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close(null);
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [url]);

  if (!url) return null;

  return (
    <div
      className="fixed inset-0 z-[200] bg-black/90 flex items-center justify-center animate-fadeIn cursor-zoom-out"
      onClick={() => close(null)}
    >
      <button
        className="absolute top-4 right-4 text-white/70 hover:text-white text-2xl w-10 h-10 flex items-center justify-center rounded-full bg-white/10 hover:bg-white/20"
        onClick={() => close(null)}
      >
        ✕
      </button>
      <img
        src={url}
        alt=""
        className="max-w-[90vw] max-h-[90vh] object-contain rounded-lg shadow-2xl cursor-default"
        onClick={(e) => e.stopPropagation()}
      />
      <a
        href={url}
        target="_blank"
        rel="noopener"
        className="absolute bottom-4 right-4 text-white/50 hover:text-white text-xs bg-white/10 hover:bg-white/20 px-3 py-1.5 rounded-lg"
        onClick={(e) => e.stopPropagation()}
      >
        Open original ↗
      </a>
    </div>
  );
}
