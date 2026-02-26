import { useStore } from '../store';

export function MotdBanner() {
  const motd = useStore((s) => s.motd);
  const dismissed = useStore((s) => s.motdDismissed);
  const dismiss = useStore((s) => s.dismissMotd);

  if (dismissed || motd.length === 0) return null;

  return (
    <div className="bg-accent/10 border-b border-accent/20 px-4 py-2.5 flex items-start gap-3 text-sm">
      <span className="shrink-0 text-accent mt-0.5">📢</span>
      <div className="flex-1 min-w-0 text-fg-muted leading-relaxed">
        {motd.map((line, i) => (
          <span key={i}>
            {line}
            {i < motd.length - 1 && <>{' · '}</>}
          </span>
        ))}
      </div>
      <button
        onClick={dismiss}
        className="shrink-0 text-fg-dim hover:text-fg-muted ml-2 p-0.5"
        title="Dismiss"
      >
        ✕
      </button>
    </div>
  );
}
