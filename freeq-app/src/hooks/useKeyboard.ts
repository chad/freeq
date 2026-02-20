import { useEffect } from 'react';

type KeyHandler = () => void;

interface ShortcutMap {
  [key: string]: KeyHandler;
}

/**
 * Global keyboard shortcut handler.
 * Keys: "mod+k", "mod+1", "escape", etc.
 * "mod" = Cmd on Mac, Ctrl on others.
 */
export function useKeyboard(shortcuts: ShortcutMap, deps: unknown[] = []) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Don't intercept when typing in inputs (unless it's a mod+key combo)
      const target = e.target as HTMLElement;
      const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable;
      const hasMod = e.metaKey || e.ctrlKey;

      if (isInput && !hasMod) return;

      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push('mod');
      if (e.shiftKey) parts.push('shift');
      if (e.altKey) parts.push('alt');

      let key = e.key.toLowerCase();
      if (key === ' ') key = 'space';

      parts.push(key);
      const combo = parts.join('+');

      const handler = shortcuts[combo];
      if (handler) {
        e.preventDefault();
        e.stopPropagation();
        handler();
      }
    };

    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, deps);
}
