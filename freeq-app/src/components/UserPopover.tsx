import { useState, useEffect } from 'react';
import { fetchProfile, type ATProfile } from '../lib/profiles';

interface UserPopoverProps {
  nick: string;
  did?: string;
  position: { x: number; y: number };
  onClose: () => void;
}

export function UserPopover({ nick, did, position, onClose }: UserPopoverProps) {
  const [profile, setProfile] = useState<ATProfile | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (did) {
      setLoading(true);
      fetchProfile(did).then((p) => {
        setProfile(p);
        setLoading(false);
      });
    }
  }, [did]);

  // Position the popover
  const style: React.CSSProperties = {
    position: 'fixed',
    left: Math.min(position.x, window.innerWidth - 300),
    top: Math.min(position.y, window.innerHeight - 300),
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
          {/* Name */}
          <div className="font-semibold text-fg">{profile?.displayName || nick}</div>
          <div className="text-sm text-fg-muted">{nick}</div>

          {/* DID / Handle */}
          {profile?.handle && (
            <div className="text-xs text-accent mt-1">@{profile.handle}</div>
          )}
          {did && (
            <div className="text-[10px] text-fg-dim mt-0.5 font-mono break-all">{did}</div>
          )}

          {/* Bio */}
          {profile?.description && (
            <div className="text-xs text-fg-muted mt-2 line-clamp-3">{profile.description}</div>
          )}

          {loading && (
            <div className="text-xs text-fg-dim mt-2">Loading profile...</div>
          )}

          {/* Actions */}
          <div className="flex gap-2 mt-3">
            <button className="flex-1 bg-bg-tertiary hover:bg-surface text-fg-muted hover:text-fg text-xs py-1.5 rounded-lg">
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
