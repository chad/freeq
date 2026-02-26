import { useStore } from '../store';

export function MotdBanner() {
  const motd = useStore((s) => s.motd);
  const dismissed = useStore((s) => s.motdDismissed);
  const dismiss = useStore((s) => s.dismissMotd);

  if (dismissed || motd.length === 0) return null;

  return (
    <div className="fixed inset-0 z-[160] flex items-center justify-center bg-black/50 backdrop-blur-sm animate-fadeIn" onClick={dismiss}>
      <div
        className="bg-bg-secondary border border-border rounded-2xl shadow-2xl w-[480px] max-w-[90vw] max-h-[80vh] overflow-hidden animate-fadeIn"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center gap-3 px-6 pt-6 pb-3">
          <img src="/freeq.png" alt="" className="w-10 h-10" />
          <div>
            <h2 className="text-lg font-bold text-fg">
              Welcome to <span className="text-accent">freeq</span>
            </h2>
            <p className="text-xs text-fg-dim">Message of the Day</p>
          </div>
          <div className="flex-1" />
          <button
            onClick={dismiss}
            className="text-fg-dim hover:text-fg-muted text-lg leading-none p-1"
          >
            ✕
          </button>
        </div>

        {/* Body */}
        <div className="px-6 pb-6 overflow-y-auto max-h-[50vh]">
          <div className="text-sm text-fg-muted leading-relaxed space-y-1 font-mono">
            {motd.map((line, i) => (
              <div key={i} className={line.trim() === '' ? 'h-2' : ''}>
                {line.trim() === '' ? null : formatMotdLine(line)}
              </div>
            ))}
          </div>
        </div>

        {/* Footer */}
        <div className="px-6 py-4 border-t border-border flex justify-end">
          <button
            onClick={dismiss}
            className="bg-accent text-black font-bold text-sm px-5 py-2 rounded-lg hover:bg-accent-hover transition-colors"
          >
            Let's go
          </button>
        </div>
      </div>
    </div>
  );
}

/** Render a MOTD line with basic formatting — channels as accent-colored, URLs as links */
function formatMotdLine(line: string) {
  const parts = line.split(/(#\S+|https?:\/\/\S+)/g);
  return parts.map((part, i) => {
    if (part.startsWith('#') && part.length > 1) {
      return <span key={i} className="text-accent font-semibold">{part}</span>;
    }
    if (part.startsWith('http')) {
      return <a key={i} href={part} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">{part}</a>;
    }
    return <span key={i}>{part}</span>;
  });
}
