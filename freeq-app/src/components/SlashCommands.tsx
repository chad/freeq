import { useMemo } from 'react';

interface Command {
  name: string;
  args: string;
  desc: string;
  category: 'chat' | 'channel' | 'user' | 'policy' | 'system';
}

const COMMANDS: Command[] = [
  { name: 'join', args: '#channel', desc: 'Join a channel', category: 'channel' },
  { name: 'part', args: '[#channel]', desc: 'Leave current or specified channel', category: 'channel' },
  { name: 'topic', args: 'text', desc: 'Set channel topic', category: 'channel' },
  { name: 'invite', args: 'nick', desc: 'Invite user to channel', category: 'channel' },
  { name: 'kick', args: 'nick [reason]', desc: 'Kick user from channel', category: 'channel' },
  { name: 'op', args: 'nick', desc: 'Give operator status', category: 'channel' },
  { name: 'deop', args: 'nick', desc: 'Remove operator status', category: 'channel' },
  { name: 'voice', args: 'nick', desc: 'Give voice status', category: 'channel' },
  { name: 'mode', args: '+mode [arg]', desc: 'Change channel or user mode', category: 'channel' },
  { name: 'msg', args: 'nick text', desc: 'Send a direct message', category: 'chat' },
  { name: 'me', args: 'action', desc: 'Send an action message', category: 'chat' },
  { name: 'whois', args: 'nick', desc: 'Look up user info', category: 'user' },
  { name: 'away', args: '[reason]', desc: 'Set away status (no reason = back)', category: 'user' },
  { name: 'policy', args: '#ch SET|INFO|ACCEPT|VERIFY|REQUIRE|CLEAR', desc: 'Channel policy management', category: 'policy' },
  { name: 'raw', args: 'IRC_LINE', desc: 'Send raw IRC command', category: 'system' },
  { name: 'help', args: '', desc: 'Show all commands', category: 'system' },
];

const CATEGORY_LABELS: Record<string, string> = {
  chat: '💬 Chat',
  channel: '📢 Channel',
  user: '👤 User',
  policy: '🛡️ Policy',
  system: '⚙️ System',
};

interface Props {
  filter: string;
  selected: number;
  onSelect: (cmd: string) => void;
}

export function SlashCommands({ filter, selected, onSelect }: Props) {
  const filtered = useMemo(() => {
    if (!filter) return COMMANDS;
    const q = filter.toLowerCase();
    return COMMANDS.filter(
      (c) => c.name.startsWith(q) || c.desc.toLowerCase().includes(q),
    );
  }, [filter]);

  if (filtered.length === 0) return null;

  // Group by category
  const grouped: { cat: string; cmds: (Command & { idx: number })[] }[] = [];
  let idx = 0;
  const catOrder = ['chat', 'channel', 'user', 'policy', 'system'];
  for (const cat of catOrder) {
    const cmds = filtered
      .filter((c) => c.category === cat)
      .map((c) => ({ ...c, idx: idx++ }));
    if (cmds.length > 0) grouped.push({ cat, cmds });
  }

  return (
    <div className="absolute bottom-full left-0 right-0 mx-3 mb-1 bg-bg-secondary border border-border rounded-xl shadow-2xl overflow-hidden z-20 max-h-72 overflow-y-auto animate-fadeIn">
      {grouped.map(({ cat, cmds }) => (
        <div key={cat}>
          <div className="px-3 py-1.5 text-[10px] uppercase tracking-wider text-fg-dim font-bold bg-bg-tertiary/50 sticky top-0">
            {CATEGORY_LABELS[cat] || cat}
          </div>
          {cmds.map((cmd) => (
            <button
              key={cmd.name}
              onClick={() => onSelect(cmd.name)}
              onMouseEnter={() => {}} // could setSelected
              className={`w-full text-left px-3 py-2 flex items-baseline gap-2 hover:bg-bg-tertiary transition-colors ${
                cmd.idx === selected ? 'bg-bg-tertiary' : ''
              }`}
            >
              <span className="text-accent font-mono text-sm font-medium">/{cmd.name}</span>
              {cmd.args && <span className="text-fg-dim text-xs font-mono">{cmd.args}</span>}
              <span className="text-fg-dim text-xs ml-auto shrink-0">{cmd.desc}</span>
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}

export function getCommandCount(filter: string): number {
  if (!filter) return COMMANDS.length;
  const q = filter.toLowerCase();
  return COMMANDS.filter(
    (c) => c.name.startsWith(q) || c.desc.toLowerCase().includes(q),
  ).length;
}
