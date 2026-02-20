import { useState } from 'react';
import { useStore } from '../store';
import { setTopic as sendTopic } from '../irc/client';

interface TopBarProps {
  onToggleSidebar?: () => void;
}

export function TopBar({ onToggleSidebar }: TopBarProps) {
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const [editing, setEditing] = useState(false);
  const [topicDraft, setTopicDraft] = useState('');

  const ch = channels.get(activeChannel.toLowerCase());
  const topic = ch?.topic || '';
  const memberCount = ch?.members.size || 0;
  const isChannel = activeChannel !== 'server';

  const startEdit = () => {
    setTopicDraft(topic);
    setEditing(true);
  };

  const submitTopic = () => {
    if (ch) sendTopic(ch.name, topicDraft);
    setEditing(false);
  };

  return (
    <header className="h-12 bg-bg-secondary border-b border-border flex items-center gap-3 px-4 shrink-0">
      {/* Mobile menu button */}
      <button
        onClick={onToggleSidebar}
        className="md:hidden text-fg-dim hover:text-fg-muted p-1 -ml-1 mr-1"
      >
        <svg className="w-5 h-5" viewBox="0 0 16 16" fill="currentColor">
          <path fillRule="evenodd" d="M2.5 12a.5.5 0 01.5-.5h10a.5.5 0 010 1H3a.5.5 0 01-.5-.5zm0-4a.5.5 0 01.5-.5h10a.5.5 0 010 1H3a.5.5 0 01-.5-.5zm0-4a.5.5 0 01.5-.5h10a.5.5 0 010 1H3a.5.5 0 01-.5-.5z"/>
        </svg>
      </button>

      {/* Channel name */}
      <div className="flex items-center gap-2 shrink-0">
        {isChannel && <span className="text-accent text-sm font-medium">#</span>}
        <span className="font-semibold text-sm text-fg">
          {isChannel ? (ch?.name || activeChannel).replace(/^#/, '') : 'Server'}
        </span>
      </div>

      {/* Separator */}
      {isChannel && <div className="w-px h-5 bg-border" />}

      {/* Topic (channels only) */}
      <div className="flex-1 min-w-0">
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
              className="w-full bg-transparent text-xs text-fg outline-none"
              placeholder="Set a topic..."
            />
          ) : (
            <button
              onClick={startEdit}
              className="text-xs text-fg-dim hover:text-fg-muted truncate block w-full text-left"
              title={topic || 'Click to set topic'}
            >
              {topic || 'Set a topic'}
            </button>
          )
        ) : (
          <span className="flex-1" />
        )}
      </div>

      {/* Member count */}
      {isChannel && memberCount > 0 && (
        <div className="flex items-center gap-1 text-fg-dim text-xs shrink-0">
          <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="currentColor">
            <path d="M8 8a3 3 0 100-6 3 3 0 000 6zM2 14s-1 0-1-1 1-4 7-4 7 3 7 4-1 1-1 1H2z"/>
          </svg>
          <span>{memberCount}</span>
        </div>
      )}
    </header>
  );
}
