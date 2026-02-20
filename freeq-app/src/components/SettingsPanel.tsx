import { useStore } from '../store';
import { requestPermission } from '../lib/notifications';

interface SettingsPanelProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsPanel({ open, onClose }: SettingsPanelProps) {
  const nick = useStore((s) => s.nick);
  const authDid = useStore((s) => s.authDid);
  const connectionState = useStore((s) => s.connectionState);

  if (!open) return null;

  return (
    <>
      <div className="fixed inset-0 z-40 bg-black/50 backdrop-blur-sm" onClick={onClose} />
      <div className="fixed right-0 top-0 bottom-0 z-50 w-80 bg-bg-secondary border-l border-border shadow-2xl animate-slideIn overflow-y-auto">
        <div className="p-4 border-b border-border flex items-center justify-between">
          <h2 className="font-semibold">Settings</h2>
          <button onClick={onClose} className="text-fg-dim hover:text-fg text-lg">✕</button>
        </div>

        <div className="p-4 space-y-6">
          {/* Account */}
          <Section title="Account">
            <InfoRow label="Nickname" value={nick} />
            <InfoRow label="Connection" value={connectionState} />
            {authDid && <InfoRow label="DID" value={authDid} mono />}
          </Section>

          {/* Notifications */}
          <Section title="Notifications">
            <button
              onClick={async () => {
                const ok = await requestPermission();
                if (ok) alert('Notifications enabled!');
                else alert('Permission denied. Enable in browser settings.');
              }}
              className="text-sm text-accent hover:text-accent-hover"
            >
              Enable browser notifications
            </button>
          </Section>

          {/* Keyboard shortcuts */}
          <Section title="Keyboard Shortcuts">
            <ShortcutRow keys="⌘ K" desc="Quick switcher" />
            <ShortcutRow keys="⌘ 1-9" desc="Switch channel" />
            <ShortcutRow keys="Esc" desc="Close panel" />
            <ShortcutRow keys="↑" desc="Edit last message" />
            <ShortcutRow keys="↑ ↓" desc="Input history" />
          </Section>

          {/* About */}
          <Section title="About">
            <p className="text-xs text-fg-dim">
              freeq — IRC with AT Protocol identity.
              <br />
              Open source at{' '}
              <a href="https://github.com/chad/freeq" target="_blank" className="text-accent hover:underline">
                github.com/chad/freeq
              </a>
            </p>
          </Section>
        </div>
      </div>
    </>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <h3 className="text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-2">{title}</h3>
      <div className="space-y-1.5">{children}</div>
    </div>
  );
}

function InfoRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between text-sm">
      <span className="text-fg-muted">{label}</span>
      <span className={`text-fg truncate max-w-[160px] ${mono ? 'font-mono text-xs' : ''}`} title={value}>
        {value}
      </span>
    </div>
  );
}

function ShortcutRow({ keys, desc }: { keys: string; desc: string }) {
  return (
    <div className="flex items-center justify-between text-sm">
      <span className="text-fg-muted">{desc}</span>
      <kbd className="text-[10px] text-fg-dim bg-bg-tertiary px-1.5 py-0.5 rounded font-mono">{keys}</kbd>
    </div>
  );
}
