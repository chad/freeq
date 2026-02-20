import { useStore } from '../store';
import { sendWhois } from '../irc/client';

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

  if (!ch || activeChannel === 'server') return null;

  const members = [...ch.members.values()].sort((a, b) => {
    const wa = a.isOp ? 0 : a.isVoiced ? 1 : 2;
    const wb = b.isOp ? 0 : b.isVoiced ? 1 : 2;
    return wa - wb || a.nick.localeCompare(b.nick);
  });

  const ops = members.filter((m) => m.isOp);
  const voiced = members.filter((m) => !m.isOp && m.isVoiced);
  const regular = members.filter((m) => !m.isOp && !m.isVoiced);

  return (
    <aside className="w-52 bg-bg-secondary border-l border-border overflow-y-auto shrink-0 hidden lg:block">
      <div className="px-3 pt-4 pb-2">
        {ops.length > 0 && (
          <Section label={`Operators — ${ops.length}`}>
            {ops.map((m) => <MemberItem key={m.nick} member={m} />)}
          </Section>
        )}
        {voiced.length > 0 && (
          <Section label={`Voiced — ${voiced.length}`}>
            {voiced.map((m) => <MemberItem key={m.nick} member={m} />)}
          </Section>
        )}
        <Section label={`${ops.length > 0 || voiced.length > 0 ? 'Members' : 'Online'} — ${regular.length}`}>
          {regular.map((m) => <MemberItem key={m.nick} member={m} />)}
        </Section>
      </div>
    </aside>
  );
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="mb-3">
      <div className="text-[10px] uppercase tracking-widest text-fg-dim font-semibold mb-1.5 px-1">
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
}

function MemberItem({ member }: MemberItemProps) {
  const color = nickColor(member.nick);

  return (
    <button
      onClick={() => sendWhois(member.nick)}
      className="w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-sm hover:bg-bg-tertiary group"
      title={member.did || member.nick}
    >
      {/* Mini avatar */}
      <div
        className="w-6 h-6 rounded-full flex items-center justify-center text-[10px] font-bold shrink-0"
        style={{ backgroundColor: color + '20', color }}
      >
        {member.nick[0]?.toUpperCase()}
      </div>

      <div className="min-w-0 flex-1 flex items-center gap-1">
        {/* Prefix */}
        {member.isOp && <span className="text-success text-[10px] font-bold">@</span>}
        {!member.isOp && member.isVoiced && <span className="text-warning text-[10px] font-bold">+</span>}

        <span className={`truncate text-sm ${
          member.away ? 'text-fg-dim' : 'text-fg-muted group-hover:text-fg'
        }`}>
          {member.nick}
        </span>

        {member.typing && (
          <span className="text-accent text-[10px] ml-auto animate-pulse">typing</span>
        )}
      </div>

      {/* Away dot */}
      {member.away && !member.typing && (
        <span className="w-1.5 h-1.5 rounded-full bg-warning shrink-0 ml-auto" title={`Away: ${member.away}`} />
      )}

      {/* DID badge */}
      {member.did && !member.typing && !member.away && (
        <span className="text-[9px] text-accent opacity-0 group-hover:opacity-60 ml-auto" title={member.did}>✓</span>
      )}
    </button>
  );
}
