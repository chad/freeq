import { useState, useEffect } from 'react';
import { fetchProfile, type ATProfile } from '../lib/profiles';
import { useStore } from '../store';
import { sendWhois } from '../irc/client';

interface UserPopoverProps {
  nick: string;
  did?: string;
  position: { x: number; y: number };
  onClose: () => void;
}

export function UserPopover({ nick, did, position, onClose }: UserPopoverProps) {
  const [profile, setProfile] = useState<ATProfile | null>(null);
  const [loading, setLoading] = useState(false);
  const setActive = useStore((s) => s.setActiveChannel);
  const addChannel = useStore((s) => s.addChannel);

  useEffect(() => {
    if (did) {
      setLoading(true);
      fetchProfile(did).then((p) => {
        setProfile(p);
        setLoading(false);
      });
    }
    // Also trigger WHOIS to get latest info
    sendWhois(nick);
  }, [did, nick]);

  const startDM = () => {
    // Create a DM buffer for this nick and switch to it
    addChannel(nick);
    setActive(nick);
    onClose();
  };

  // Position the popover, keeping it on screen
  const style: React.CSSProperties = {
    position: 'fixed',
    left: Math.min(position.x, window.innerWidth - 300),
    top: Math.min(position.y, window.innerHeight - 350),
    zIndex: 100,
  };

  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} />
      <div style={style} className="z-50 bg-bg-secondary border border-border rounded-xl shadow-2xl w-72 animate-fadeIn overflow-hidden">
        {/* Header */}
        <div className="h-16 bg-gradient-to-r from-accent/20 to-purple/20 relative">
          {profile?.avatar ? (
            <img
              src={profile.avatar}
              alt=""
              className="absolute -bottom-6 left-4 w-14 h-14 rounded-full border-4 border-bg-secondary object-cover"
            />
          ) : (
            <div className="absolute -bottom-6 left-4 w-14 h-14 rounded-full border-4 border-bg-secondary bg-surface flex items-center justify-center text-accent font-bold text-lg">
              {nick[0]?.toUpperCase()}
            </div>
          )}
        </div>

        <div className="pt-8 px-4 pb-4">
          {/* Display name */}
          <div className="font-semibold text-fg">
            {profile?.displayName || nick}
          </div>
          {/* Nick (if different from display name) */}
          {profile?.displayName && profile.displayName !== nick && (
            <div className="text-sm text-fg-muted">{nick}</div>
          )}
          {!profile?.displayName && (
            <div className="text-sm text-fg-muted">{nick}</div>
          )}

          {/* AT Handle */}
          {profile?.handle && (
            <div className="text-xs text-accent mt-1 flex items-center gap-1">
              <span>@{profile.handle}</span>
              <span className="text-success text-[10px]" title="Verified AT Protocol identity">✓</span>
            </div>
          )}

          {/* DID */}
          {did && (
            <div
              className="text-[10px] text-fg-dim mt-1 font-mono break-all cursor-pointer hover:text-fg-muted"
              onClick={() => navigator.clipboard.writeText(did)}
              title="Click to copy DID"
            >
              {did}
            </div>
          )}

          {/* Bio */}
          {profile?.description && (
            <div className="text-xs text-fg-muted mt-2 leading-relaxed line-clamp-3">
              {profile.description}
            </div>
          )}

          {loading && !profile && (
            <div className="text-xs text-fg-dim mt-2 flex items-center gap-1">
              <svg className="animate-spin w-3 h-3" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
              </svg>
              Loading profile...
            </div>
          )}

          {/* Actions */}
          <div className="flex gap-2 mt-3">
            <button
              onClick={startDM}
              className="flex-1 bg-accent/10 hover:bg-accent/20 text-accent text-xs py-1.5 rounded-lg font-medium"
            >
              Message
            </button>
            {profile?.handle && (
              <a
                href={`https://bsky.app/profile/${profile.handle}`}
                target="_blank"
                rel="noopener"
                className="flex-1 bg-bg-tertiary hover:bg-surface text-fg-muted hover:text-fg text-xs py-1.5 rounded-lg text-center"
              >
                Bluesky ↗
              </a>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
