import { useEffect, useRef, useState, useMemo } from 'react';

// Lightweight inline emoji picker — no external dependency needed
const EMOJI_CATEGORIES = [
  { name: 'Smileys', emojis: ['😀','😂','🥹','😊','😇','🥰','😍','😘','😜','🤪','😎','🤓','🥳','😤','😡','🥺','😭','😱','🤮','💀','👻','🤖','👽','💩'] },
  { name: 'Gestures', emojis: ['👍','👎','👋','🤝','🙌','👏','💪','🫡','🫶','✌️','🤞','🤙','🖐️','☝️','🫵','👆','👇','👈','👉','🤌'] },
  { name: 'Hearts', emojis: ['❤️','🧡','💛','💚','💙','💜','🖤','🤍','💔','❤️‍🔥','💕','💞','💓','💗','💖','💝','💘','💌'] },
  { name: 'Objects', emojis: ['🔥','⭐','✨','💫','🎉','🎊','🏆','🎯','💡','🔒','🔑','💎','🧲','⚡','💬','👀','🧠','🫧'] },
  { name: 'Symbols', emojis: ['✅','❌','⚠️','❓','❗','💯','♻️','🔴','🟢','🔵','⬛','⬜','🟥','🟧','🟨','🟩','🟦','🟪'] },
  { name: 'Flags', emojis: ['🏳️','🏴','🏁','🚩','🏳️‍🌈','🏳️‍⚧️'] },
];

interface EmojiPickerProps {
  onSelect: (emoji: string) => void;
  onClose: () => void;
  position?: { x: number; y: number };
}

export function EmojiPicker({ onSelect, onClose, position }: EmojiPickerProps) {
  const [search, setSearch] = useState('');
  const ref = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [onClose]);

  const filteredCategories = useMemo(() => {
    if (!search) return EMOJI_CATEGORIES;
    // Without an emoji name mapping, just show all when searching
    const all = EMOJI_CATEGORIES.flatMap((c) => c.emojis);
    return [{ name: 'Results', emojis: all }];
  }, [search]);

  const style: React.CSSProperties = position
    ? { position: 'fixed', left: position.x, bottom: window.innerHeight - position.y, zIndex: 100 }
    : {};

  return (
    <div
      ref={ref}
      style={style}
      className="bg-bg-secondary border border-border rounded-xl shadow-2xl w-72 animate-fadeIn overflow-hidden"
    >
      <div className="p-2 border-b border-border">
        <input
          ref={inputRef}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search emoji..."
          className="w-full bg-bg-tertiary rounded px-2 py-1.5 text-xs text-fg outline-none placeholder:text-fg-dim"
        />
      </div>
      <div className="max-h-[240px] overflow-y-auto p-1">
        {filteredCategories.map((cat) => (
          <div key={cat.name}>
            <div className="text-[10px] uppercase tracking-wider text-fg-dim px-1.5 py-1 font-semibold">
              {cat.name}
            </div>
            <div className="flex flex-wrap">
              {cat.emojis.map((emoji) => (
                <button
                  key={emoji}
                  onClick={() => { onSelect(emoji); onClose(); }}
                  className="w-8 h-8 flex items-center justify-center text-lg hover:bg-bg-tertiary rounded"
                >
                  {emoji}
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
