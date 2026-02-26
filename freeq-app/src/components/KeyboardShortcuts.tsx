interface Props {
  open: boolean;
  onClose: () => void;
}

const shortcuts = [
  { section: 'Navigation', items: [
    { keys: ['⌘', 'K'], desc: 'Quick switcher' },
    { keys: ['⌘', '1-9'], desc: 'Switch to channel by position' },
    { keys: ['Alt', '↑/↓'], desc: 'Previous / next channel' },
    { keys: ['Esc'], desc: 'Close modal / cancel' },
  ]},
  { section: 'Messaging', items: [
    { keys: ['Enter'], desc: 'Send message' },
    { keys: ['Shift', 'Enter'], desc: 'New line' },
    { keys: ['↑'], desc: 'Edit last message' },
    { keys: ['Tab'], desc: 'Autocomplete nick' },
    { keys: ['@'], desc: 'Mention user' },
    { keys: ['#'], desc: 'Link channel' },
  ]},
  { section: 'Search & Discovery', items: [
    { keys: ['⌘', 'F'], desc: 'Search messages' },
    { keys: ['⌘', '/'], desc: 'Keyboard shortcuts' },
  ]},
  { section: 'Files', items: [
    { keys: ['⌘', 'V'], desc: 'Paste image to upload' },
    { keys: ['Drag'], desc: 'Drop file to upload' },
  ]},
];

export function KeyboardShortcuts({ open, onClose }: Props) {
  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60 backdrop-blur-sm" onClick={onClose}>
      <div className="bg-bg-secondary border border-border rounded-xl shadow-2xl w-full max-w-md overflow-hidden" onClick={(e) => e.stopPropagation()}>
        <div className="px-6 py-4 border-b border-border flex items-center justify-between">
          <h2 className="text-lg font-bold text-fg">Keyboard Shortcuts</h2>
          <button onClick={onClose} className="text-fg-dim hover:text-fg p-1">
            <svg className="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
              <path fillRule="evenodd" d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z" />
            </svg>
          </button>
        </div>
        <div className="px-6 py-4 max-h-96 overflow-y-auto space-y-5">
          {shortcuts.map((section) => (
            <div key={section.section}>
              <h3 className="text-xs uppercase tracking-wider text-fg-dim font-bold mb-2">{section.section}</h3>
              <div className="space-y-1.5">
                {section.items.map((item) => (
                  <div key={item.desc} className="flex items-center justify-between">
                    <span className="text-sm text-fg-muted">{item.desc}</span>
                    <div className="flex items-center gap-1">
                      {item.keys.map((key) => (
                        <kbd key={key} className="px-1.5 py-0.5 text-xs font-mono bg-bg-tertiary border border-border rounded text-fg-dim min-w-[24px] text-center">
                          {key}
                        </kbd>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
        <div className="px-6 py-3 border-t border-border text-center">
          <span className="text-xs text-fg-dim">Press <kbd className="px-1 py-0.5 text-xs font-mono bg-bg-tertiary border border-border rounded">Esc</kbd> to close</span>
        </div>
      </div>
    </div>
  );
}
