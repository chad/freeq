import { useEffect, useRef } from 'react';
import { useStore, type Message } from '../store';
import { sendDelete, pinMessage, unpinMessage } from '../irc/client';
import { showToast } from './Toast';

interface Props {
  msg: Message;
  channel: string;
  position: { x: number; y: number };
  onClose: () => void;
  onReply: () => void;
  onEdit: () => void;
  onThread: () => void;
  onReact: (e: React.MouseEvent) => void;
}

export function MessageContextMenu({ msg, channel, position, onClose, onReply, onEdit, onThread, onReact }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    document.addEventListener('mousedown', handler);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', handler);
      document.removeEventListener('keydown', esc);
    };
  }, [onClose]);

  const copyText = () => {
    navigator.clipboard.writeText(msg.text); showToast('Copied to clipboard', 'success', 2000);
    onClose();
  };

  const copyLink = () => {
    const url = `${window.location.origin}/#${channel}/${msg.id}`;
    navigator.clipboard.writeText(url); showToast('Link copied', 'success', 2000);
    onClose();
  };

  const handleDelete = () => {
    if (confirm('Delete this message?')) {
      sendDelete(channel, msg.id);
    }
    onClose();
  };

  // Position on screen
  const style: React.CSSProperties = {
    position: 'fixed',
    left: Math.min(position.x, window.innerWidth - 200),
    top: Math.min(position.y, window.innerHeight - 300),
    zIndex: 100,
  };

  return (
    <div ref={ref} style={style} className="bg-bg-secondary border border-border rounded-xl shadow-2xl py-1.5 min-w-[180px] animate-fadeIn">
      <MenuItem icon="↩️" label="Reply" onClick={() => { onReply(); onClose(); }} />
      <MenuItem icon="🧵" label="View Thread" onClick={() => { onThread(); onClose(); }} />
      <MenuItem icon="😄" label="Add Reaction" onClick={(e) => { onReact(e); onClose(); }} />
      <div className="h-px bg-border mx-2 my-1" />
      <MenuItem icon="📋" label="Copy Text" onClick={copyText} />
      <MenuItem icon="🔗" label="Copy Link" onClick={copyLink} />
      {msg.id && <MenuItem icon="🆔" label="Copy Message ID" onClick={() => {
        navigator.clipboard.writeText(msg.id);
        showToast('Message ID copied', 'success', 2000);
        onClose();
      }} />}
      <MenuItem icon="🔖" label="Bookmark" onClick={() => {
        useStore.getState().addBookmark(channel, msg.id, msg.from, msg.text, msg.timestamp);
        onClose();
      }} />
      {channel.startsWith('#') && (() => {
        const ch = useStore.getState().channels.get(channel.toLowerCase());
        const isPinned = ch?.pins.some(p => p.msgid === msg.id);
        return (
          <MenuItem icon="📌" label={isPinned ? "Unpin Message" : "Pin Message"} onClick={() => {
            if (isPinned) {
              unpinMessage(channel, msg.id);
              useStore.getState().setPins(channel, (ch?.pins || []).filter(p => p.msgid !== msg.id));
            } else {
              pinMessage(channel, msg.id);
              const now = Math.floor(Date.now() / 1000);
              useStore.getState().setPins(channel, [{ msgid: msg.id, pinned_by: 'you', pinned_at: now }, ...(ch?.pins || [])]);
            }
            onClose();
          }} />
        );
      })()}
      <MenuItem icon="🦋" label="Share to Bluesky" onClick={() => {
        const bskyUrl = `https://bsky.app/intent/compose?text=${encodeURIComponent(`"${msg.text.slice(0, 200)}" — ${msg.from} on freeq`)}`;
        window.open(bskyUrl, '_blank');
        onClose();
      }} />
      {msg.isSelf && !msg.isSystem && (
        <>
          <div className="h-px bg-border mx-2 my-1" />
          <MenuItem icon="✏️" label="Edit" onClick={() => { onEdit(); onClose(); }} />
          <MenuItem icon="🗑️" label="Delete" onClick={handleDelete} danger />
        </>
      )}
    </div>
  );
}

function MenuItem({ icon, label, onClick, danger }: { icon: string; label: string; onClick: (e: React.MouseEvent) => void; danger?: boolean }) {
  return (
    <button
      onClick={onClick}
      className={`w-full text-left px-3 py-1.5 text-sm flex items-center gap-2.5 hover:bg-bg-tertiary transition-colors ${
        danger ? 'text-danger hover:bg-danger/10' : 'text-fg-muted hover:text-fg'
      }`}
    >
      <span className="text-sm w-5 text-center">{icon}</span>
      {label}
    </button>
  );
}
