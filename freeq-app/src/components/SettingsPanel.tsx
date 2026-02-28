import { useStore } from '../store';
import { requestPermission } from '../lib/notifications';
import { getPreferences, setPreferences } from '../lib/db';
import { useState, useEffect } from 'react';

interface SettingsPanelProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsPanel({ open, onClose }: SettingsPanelProps) {
  const nick = useStore((s) => s.nick);
  const authDid = useStore((s) => s.authDid);
  const connectionState = useStore((s) => s.connectionState);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const density = useStore((s) => s.messageDensity);
  const setDensity = useStore((s) => s.setMessageDensity);
  const loadMedia = useStore((s) => s.loadExternalMedia);
  const setLoadMedia = useStore((s) => s.setLoadExternalMedia);

  const [notifs, setNotifs] = useState(true);
  const [sounds, setSounds] = useState(true);

  useEffect(() => {
    if (open) {
      getPreferences().then((p) => {
        setNotifs(p.notifications);
        setSounds(p.sounds);
      });
    }
  }, [open]);

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

          {/* Appearance */}
          <Section title="Appearance">
            <div className="flex items-center justify-between text-sm">
              <span className="text-fg-muted">Theme</span>
              <div className="flex gap-1 bg-bg rounded-lg p-0.5">
                <button
                  onClick={() => setTheme('dark')}
                  className={`px-2.5 py-1 text-xs rounded-md ${theme === 'dark' ? 'bg-surface text-fg' : 'text-fg-dim'}`}
                >
                  🌙 Dark
                </button>
                <button
                  onClick={() => setTheme('light')}
                  className={`px-2.5 py-1 text-xs rounded-md ${theme === 'light' ? 'bg-surface text-fg' : 'text-fg-dim'}`}
                >
                  ☀️ Light
                </button>
              </div>
            </div>
            <div className="flex items-center justify-between text-sm">
              <span className="text-fg-muted">Message density</span>
              <div className="flex gap-1 bg-bg rounded-lg p-0.5">
                {(['cozy', 'default', 'compact'] as const).map((d) => (
                  <button
                    key={d}
                    onClick={() => setDensity(d)}
                    className={`px-2 py-1 text-xs rounded-md capitalize ${density === d ? 'bg-surface text-fg' : 'text-fg-dim'}`}
                  >
                    {d}
                  </button>
                ))}
              </div>
            </div>
          </Section>

          {/* Notifications */}
          <Section title="Notifications">
            <Toggle
              label="Desktop notifications"
              checked={notifs}
              onChange={async (v) => {
                setNotifs(v);
                await setPreferences({ notifications: v });
                if (v) {
                  const ok = await requestPermission();
                  if (!ok) {
                    setNotifs(false);
                    await setPreferences({ notifications: false });
                  }
                }
              }}
            />
            <Toggle
              label="Sound effects"
              checked={sounds}
              onChange={async (v) => {
                setSounds(v);
                await setPreferences({ sounds: v });
              }}
            />
          </Section>

          {/* Privacy */}
          <Section title="Privacy">
            <Toggle
              label="Load external media"
              checked={loadMedia}
              onChange={setLoadMedia}
            />
            <p className="text-[11px] text-fg-dim leading-relaxed mt-1">
              When off, images from external URLs require a click to load. Prevents IP leakage via tracking pixels.
            </p>
          </Section>

          {/* Keyboard shortcuts */}
          <Section title="Keyboard Shortcuts">
            <ShortcutRow keys="⌘ K" desc="Quick switcher" />
            <ShortcutRow keys="⌘ F" desc="Search messages" />
            <ShortcutRow keys="⌥ 1-0" desc="Switch channel" />
            <ShortcutRow keys="Esc" desc="Close panel / cancel" />
            <ShortcutRow keys="↑" desc="Edit last message" />
            <ShortcutRow keys="Tab" desc="Autocomplete nick" />
          </Section>

          {/* About */}
          <Section title="About">
            <p className="text-xs text-fg-dim leading-relaxed">
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
      <div className="space-y-2">{children}</div>
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

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between text-sm">
      <span className="text-fg-muted">{label}</span>
      <button
        onClick={() => onChange(!checked)}
        className={`w-9 h-5 rounded-full relative transition-colors ${checked ? 'bg-accent' : 'bg-surface'}`}
      >
        <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-transform ${
          checked ? 'translate-x-4' : 'translate-x-0.5'
        }`} />
      </button>
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
