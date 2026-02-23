import { useEffect, useRef } from 'react';
import { type Message } from '../store';
import { sendDelete } from '../irc/client';

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
    navigator.clipboard.writeText(msg.text);
    onClose();
  };

  const copyLink = () => {
    const url = `${window.location.origin}/#${channel}/${msg.id}`;
    navigator.clipboard.writeText(url);
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
