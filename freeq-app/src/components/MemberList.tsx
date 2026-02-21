import { useState, useEffect } from 'react';
import { useStore } from '../store';
import { fetchProfile, getCachedProfile } from '../lib/profiles';
import { UserPopover } from './UserPopover';

const NICK_COLORS = [
  '#ff6eb4', '#00d4aa', '#ffb547', '#5c9eff', '#b18cff',
  '#ff9547', '#00c4ff', '#ff5c5c', '#7edd7e', '#ff85d0',
];

function nickColor(nick: string): string {
  let h = 0;
  for (let i = 0; i < nick.length; i++) h = nick.charCodeAt(i) + ((h << 5) - h);
  return NICK_COLORS[Math.abs(h) % NICK_COLORS.length];
}

export function MemberList() {
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const ch = channels.get(activeChannel.toLowerCase());
  const [popover, setPopover] = useState<{ nick: string; did?: string; pos: { x: number; y: number } } | null>(null);

  if (!ch || activeChannel === 'server') return null;

  const members = [...ch.members.values()].sort((a, b) => {
    const wa = a.isOp ? 0 : a.isVoiced ? 1 : 2;
    const wb = b.isOp ? 0 : b.isVoiced ? 1 : 2;
    return wa - wb || a.nick.localeCompare(b.nick);
  });

  const ops = members.filter((m) => m.isOp);
  const voiced = members.filter((m) => !m.isOp && m.isVoiced);
  const regular = members.filter((m) => !m.isOp && !m.isVoiced);

  const onMemberClick = (nick: string, did: string | undefined, e: React.MouseEvent) => {
    setPopover({ nick, did, pos: { x: e.clientX, y: e.clientY } });
  };

  return (
    <aside className="w-52 h-full bg-bg-secondary border-l border-border overflow-y-auto shrink-0">
      <div className="px-3 pt-4 pb-2">
        {ops.length > 0 && (
          <Section label={`Operators — ${ops.length}`}>
            {ops.map((m) => <MemberItem key={m.nick} member={m} onClick={onMemberClick} />)}
          </Section>
        )}
        {voiced.length > 0 && (
          <Section label={`Voiced — ${voiced.length}`}>
            {voiced.map((m) => <MemberItem key={m.nick} member={m} onClick={onMemberClick} />)}
          </Section>
        )}
        <Section label={`${ops.length > 0 || voiced.length > 0 ? 'Members' : 'Online'} — ${regular.length}`}>
          {regular.map((m) => <MemberItem key={m.nick} member={m} onClick={onMemberClick} />)}
        </Section>
      </div>

      {popover && (
        <UserPopover
          nick={popover.nick}
          did={popover.did}
          position={popover.pos}
          onClose={() => setPopover(null)}
        />
      )}
    </aside>
  );
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="mb-3">
      <div className="text-xs uppercase tracking-wider text-fg-dim font-bold mb-2 px-1">
        {label}
      </div>
      {children}
    </div>
  );
}

interface MemberItemProps {
  member: {
    nick: string;
    did?: string;
    isOp: boolean;
    isVoiced: boolean;
    away?: string | null;
    typing?: boolean;
  };
  onClick: (nick: string, did: string | undefined, e: React.MouseEvent) => void;
}

function MemberItem({ member, onClick }: MemberItemProps) {
  const color = nickColor(member.nick);

  return (
    <button
      onClick={(e) => onClick(member.nick, member.did, e)}
      className="w-full flex items-center gap-2.5 px-2 py-1.5 rounded-lg text-[15px] hover:bg-bg-tertiary group"
      title={member.did || member.nick}
    >
      <div className="relative">
        <MiniAvatar nick={member.nick} did={member.did} color={color} />
        {/* Presence dot */}
        <span className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-bg-secondary ${
          member.away ? 'bg-warning' : 'bg-success'
        }`} />
      </div>

      <div className="min-w-0 flex-1 flex items-center gap-1">
        {member.isOp && <span className="text-success text-xs font-bold">@</span>}
        {!member.isOp && member.isVoiced && <span className="text-warning text-xs font-bold">+</span>}

        <span className={`truncate text-[15px] ${
          member.away ? 'text-fg-dim' : 'text-fg-muted group-hover:text-fg'
        }`}>
          {member.nick}
        </span>

        {member.did && (
          <span className="text-accent text-xs" title="AT Protocol verified">✓</span>
        )}

        {member.typing && (
          <span className="text-accent text-xs ml-auto animate-pulse">typing</span>
        )}
      </div>
    </button>
  );
}

function MiniAvatar({ nick, did, color }: { nick: string; did?: string; color: string }) {
  const [avatarUrl, setAvatarUrl] = useState<string | null>(() => {
    if (!did) return null;
    return getCachedProfile(did)?.avatar || null;
  });

  useEffect(() => {
    if (did && !avatarUrl) {
      fetchProfile(did).then((p) => p?.avatar && setAvatarUrl(p.avatar));
    }
  }, [did]);

  if (avatarUrl) {
    return <img src={avatarUrl} alt="" className="w-8 h-8 rounded-full object-cover shrink-0" />;
  }

  return (
    <div
      className="w-8 h-8 rounded-full flex items-center justify-center text-xs font-bold shrink-0"
      style={{ backgroundColor: color + '20', color }}
    >
      {nick[0]?.toUpperCase()}
    </div>
  );
}
