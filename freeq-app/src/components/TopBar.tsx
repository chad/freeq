import { useState } from 'react';
import { useStore } from '../store';
import { setTopic as sendTopic } from '../irc/client';

interface TopBarProps {
  onToggleSidebar?: () => void;
  onToggleMembers?: () => void;
  membersOpen?: boolean;
}

export function TopBar({ onToggleSidebar, onToggleMembers, membersOpen }: TopBarProps) {
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const [editing, setEditing] = useState(false);
  const [topicDraft, setTopicDraft] = useState('');

  const ch = channels.get(activeChannel.toLowerCase());
  const topic = ch?.topic || '';
  const memberCount = ch?.members.size || 0;
  const isChannel = activeChannel !== 'server' && activeChannel.startsWith('#');
  const isDM = activeChannel !== 'server' && !activeChannel.startsWith('#');
  const setChannelSettings = useStore((s) => s.setChannelSettingsOpen);

  const startEdit = () => {
    setTopicDraft(topic);
    setEditing(true);
  };

  const submitTopic = () => {
    if (ch) sendTopic(ch.name, topicDraft);
    setEditing(false);
  };

  return (
    <header className="h-14 bg-bg-secondary border-b border-border flex items-center gap-3 px-4 shrink-0">
      {/* Mobile menu button */}
      <button
        onClick={onToggleSidebar}
        className="md:hidden text-fg-dim hover:text-fg-muted p-1 -ml-1 mr-1"
      >
        <svg className="w-5 h-5" viewBox="0 0 16 16" fill="currentColor">
          <path fillRule="evenodd" d="M2.5 12a.5.5 0 01.5-.5h10a.5.5 0 010 1H3a.5.5 0 01-.5-.5zm0-4a.5.5 0 01.5-.5h10a.5.5 0 010 1H3a.5.5 0 01-.5-.5zm0-4a.5.5 0 01.5-.5h10a.5.5 0 010 1H3a.5.5 0 01-.5-.5z"/>
        </svg>
      </button>

      {/* Channel / DM name */}
      <div className="flex items-center gap-2 min-w-0 shrink">
        {isChannel && <span className="text-accent text-base font-bold shrink-0">#</span>}
        {isDM && <span className="text-fg-dim text-base shrink-0">💬</span>}
        <span className="font-bold text-base text-fg truncate">
          {isChannel ? (ch?.name || activeChannel).replace(/^#/, '') : isDM ? activeChannel : 'Server'}
        </span>
        {ch?.isEncrypted && (
          <span className="text-success text-xs shrink-0" title="End-to-end encrypted channel">🔒</span>
        )}
        {isDM && !ch?.isEncrypted && (() => {
          // Show lock if DM partner has a DID (E2EE capable)
          const partnerDid = ch?.members.values().next().value?.did;
          return partnerDid ? (
            <span className="text-success text-xs shrink-0" title="End-to-end encrypted DM">🔒</span>
          ) : null;
        })()}
      </div>

      {/* Identity stats */}
      {isChannel && ch && (() => {
        const verified = [...ch.members.values()].filter((m) => m.did).length;
        return verified > 0 ? (
          <span className="text-xs text-success/80 shrink-0 flex items-center gap-1" title={`${verified} AT Protocol verified`}>
            <svg className="w-3 h-3" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 0a8 8 0 100 16A8 8 0 008 0zm3.78 5.97l-4.5 5a.75.75 0 01-1.06.02l-2-1.86a.75.75 0 011.02-1.1l1.45 1.35 3.98-4.43a.75.75 0 011.11 1.02z"/>
            </svg>
            {verified}
          </span>
        ) : null;
      })()}

      {/* Separator */}
      {isChannel && <div className="w-px h-5 bg-border" />}

      {/* Topic (channels only, hidden on very small screens) */}
      <div className="flex-1 min-w-0 hidden sm:block">
        {isChannel ? (
          editing ? (
            <input
              value={topicDraft}
              onChange={(e) => setTopicDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') submitTopic();
                if (e.key === 'Escape') setEditing(false);
              }}
              onBlur={() => setEditing(false)}
              autoFocus
              className="w-full bg-transparent text-sm text-fg outline-none"
              placeholder="Set a topic..."
            />
          ) : (
            <button
              onClick={startEdit}
              className="text-sm text-fg-dim hover:text-fg-muted truncate block w-full text-left"
              title={topic || 'Click to set topic'}
            >
              {topic || 'Set a topic'}
            </button>
          )
        ) : (
          <span className="flex-1" />
        )}
      </div>

      {/* Settings gear */}
      {isChannel && (
        <button
          onClick={() => setChannelSettings(ch?.name || activeChannel)}
          className="text-fg-dim hover:text-fg-muted p-1.5 rounded-lg hover:bg-bg-tertiary transition-colors"
          title="Channel settings"
        >
          <svg className="w-4 h-4" viewBox="0 0 20 20" fill="currentColor">
            <path fillRule="evenodd" d="M11.49 3.17c-.38-1.56-2.6-1.56-2.98 0a1.532 1.532 0 01-2.286.948c-1.372-.836-2.942.734-2.106 2.106.54.886.061 2.042-.947 2.287-1.561.379-1.561 2.6 0 2.978a1.532 1.532 0 01.947 2.287c-.836 1.372.734 2.942 2.106 2.106a1.532 1.532 0 012.287.947c.379 1.561 2.6 1.561 2.978 0a1.533 1.533 0 012.287-.947c1.372.836 2.942-.734 2.106-2.106a1.533 1.533 0 01.947-2.287c1.561-.379 1.561-2.6 0-2.978a1.532 1.532 0 01-.947-2.287c.836-1.372-.734-2.942-2.106-2.106a1.532 1.532 0 01-2.287-.947zM10 13a3 3 0 100-6 3 3 0 000 6z" clipRule="evenodd" />
          </svg>
        </button>
      )}

      {/* Member list toggle */}
      {isChannel && (
        <button
          onClick={onToggleMembers}
          className={`flex items-center gap-1.5 text-sm shrink-0 px-2.5 py-1.5 rounded-lg hover:bg-bg-tertiary transition-colors ${
            membersOpen ? 'text-fg-muted' : 'text-fg-dim'
          }`}
          title={membersOpen ? 'Hide members' : 'Show members'}
        >
          <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="currentColor">
            <path d="M8 8a3 3 0 100-6 3 3 0 000 6zM2 14s-1 0-1-1 1-4 7-4 7 3 7 4-1 1-1 1H2z"/>
          </svg>
          {memberCount > 0 && <span>{memberCount}</span>}
        </button>
      )}
    </header>
  );
}
