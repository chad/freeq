import { useStore } from '../store';

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
    <aside className="w-44 bg-bg-secondary border-l border-border overflow-y-auto shrink-0 hidden lg:block">
      <div className="px-3 pt-3 pb-1">
        <div className="text-[10px] uppercase tracking-wider text-fg-dim font-semibold mb-2">
          Members ({members.length})
        </div>

        {ops.length > 0 && (
          <>
            <div className="text-[10px] uppercase text-fg-dim mb-1">Operators</div>
            {ops.map((m) => <MemberItem key={m.nick} member={m} />)}
          </>
        )}
        {voiced.length > 0 && (
          <>
            <div className="text-[10px] uppercase text-fg-dim mb-1 mt-2">Voiced</div>
            {voiced.map((m) => <MemberItem key={m.nick} member={m} />)}
          </>
        )}
        {regular.length > 0 && (
          <>
            {(ops.length > 0 || voiced.length > 0) && (
              <div className="text-[10px] uppercase text-fg-dim mb-1 mt-2">Members</div>
            )}
            {regular.map((m) => <MemberItem key={m.nick} member={m} />)}
          </>
        )}
      </div>
    </aside>
  );
}

function MemberItem({ member }: { member: { nick: string; isOp: boolean; isVoiced: boolean; away?: string | null; typing?: boolean } }) {
  return (
    <div className="flex items-center gap-1.5 px-1 py-0.5 rounded text-sm text-fg-muted hover:bg-bg-tertiary/50 cursor-default">
      {member.isOp && <span className="text-success font-bold text-xs">@</span>}
      {!member.isOp && member.isVoiced && <span className="text-warning font-bold text-xs">+</span>}
      {!member.isOp && !member.isVoiced && <span className="w-2.5" />}
      <span className={`truncate ${member.away ? 'opacity-50' : ''}`}>
        {member.nick}
      </span>
      {member.typing && <span className="text-accent text-xs animate-pulse">···</span>}
      {member.away && <span className="text-fg-dim text-[10px] ml-auto">away</span>}
    </div>
  );
}
