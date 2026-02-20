import { useState, useRef, useCallback, useEffect, useMemo, type KeyboardEvent, type DragEvent } from 'react';
import { useStore } from '../store';
import { sendMessage, joinChannel, partChannel, setTopic, setMode, kickUser, inviteUser, setAway, rawCommand, sendWhois } from '../irc/client';
import { EmojiPicker } from './EmojiPicker';

// Max file size: 10MB
const MAX_FILE_SIZE = 10 * 1024 * 1024;
const ALLOWED_TYPES = ['image/jpeg', 'image/png', 'image/gif', 'image/webp', 'video/mp4', 'video/webm', 'audio/mpeg', 'audio/ogg', 'application/pdf'];

interface PendingUpload {
  file: File;
  preview?: string;
  uploading: boolean;
  error?: string;
}

export function ComposeBox() {
  const [text, setText] = useState('');
  const [history, setHistory] = useState<string[]>([]);
  const [historyPos, setHistoryPos] = useState(-1);
  const [showEmoji, setShowEmoji] = useState(false);
  const [autocomplete, setAutocomplete] = useState<{ items: string[]; selected: number; startPos: number } | null>(null);
  const [pendingUpload, setPendingUpload] = useState<PendingUpload | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const emojiRef = useRef<HTMLButtonElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const authDid = useStore((s) => s.authDid);
  const ch = channels.get(activeChannel.toLowerCase());

  // Typing members
  const typingMembers = ch
    ? [...ch.members.values()].filter((m) => m.typing).map((m) => m.nick)
    : [];

  // Members for autocomplete
  const memberNicks = useMemo(() => {
    if (!ch) return [];
    return [...ch.members.values()].map((m) => m.nick).sort();
  }, [ch?.members]);

  // Focus input on channel switch
  useEffect(() => {
    inputRef.current?.focus();
  }, [activeChannel]);

  // Autocomplete logic
  const updateAutocomplete = (value: string, cursorPos: number) => {
    const before = value.slice(0, cursorPos);
    const atIdx = before.lastIndexOf('@');
    if (atIdx >= 0 && (atIdx === 0 || before[atIdx - 1] === ' ')) {
      const partial = before.slice(atIdx + 1).toLowerCase();
      if (partial.length > 0) {
        const matches = memberNicks.filter((n) => n.toLowerCase().startsWith(partial));
        if (matches.length > 0) {
          setAutocomplete({ items: matches.slice(0, 8), selected: 0, startPos: atIdx });
          return;
        }
      }
    }
    const hashIdx = before.lastIndexOf('#');
    if (hashIdx >= 0 && (hashIdx === 0 || before[hashIdx - 1] === ' ')) {
      const partial = before.slice(hashIdx + 1).toLowerCase();
      if (partial.length > 0) {
        const chanNames = [...channels.values()].map((c) => c.name).filter((n) => n.toLowerCase().includes(partial));
        if (chanNames.length > 0) {
          setAutocomplete({ items: chanNames.slice(0, 8), selected: 0, startPos: hashIdx });
          return;
        }
      }
    }
    setAutocomplete(null);
  };

  const acceptAutocomplete = (item: string) => {
    if (!autocomplete) return;
    const before = text.slice(0, autocomplete.startPos);
    const after = text.slice(inputRef.current?.selectionStart || text.length);
    const isChannel = item.startsWith('#');
    const newText = before + (isChannel ? item : `@${item}`) + ' ' + after;
    setText(newText);
    setAutocomplete(null);
    inputRef.current?.focus();
  };

  // ── File upload ──

  const handleFileSelect = useCallback((file: File) => {
    if (!authDid) {
      useStore.getState().addSystemMessage(activeChannel, 'File upload requires AT Protocol authentication');
      return;
    }
    if (file.size > MAX_FILE_SIZE) {
      useStore.getState().addSystemMessage(activeChannel, `File too large (max ${MAX_FILE_SIZE / 1024 / 1024}MB)`);
      return;
    }
    if (!ALLOWED_TYPES.includes(file.type) && !file.type.startsWith('image/')) {
      useStore.getState().addSystemMessage(activeChannel, `Unsupported file type: ${file.type}`);
      return;
    }

    const preview = file.type.startsWith('image/') ? URL.createObjectURL(file) : undefined;
    setPendingUpload({ file, preview, uploading: false });
  }, [authDid, activeChannel]);

  const cancelUpload = () => {
    if (pendingUpload?.preview) URL.revokeObjectURL(pendingUpload.preview);
    setPendingUpload(null);
  };

  const doUpload = useCallback(async () => {
    if (!pendingUpload || !authDid) return;
    setPendingUpload((p) => p ? { ...p, uploading: true, error: undefined } : null);

    try {
      const form = new FormData();
      form.append('file', pendingUpload.file);
      form.append('did', authDid);
      if (activeChannel !== 'server' && activeChannel.startsWith('#')) {
        form.append('channel', activeChannel);
      }
      if (text.trim()) {
        form.append('alt', text.trim());
      }

      const resp = await fetch('/api/v1/upload', { method: 'POST', body: form });
      if (!resp.ok) {
        const err = await resp.text();
        throw new Error(err);
      }

      const result = await resp.json();
      const target = ch?.name || activeChannel;
      if (target && target !== 'server') {
        // Send as PRIVMSG with the media URL (and alt text as message)
        const msgText = text.trim() ? `${text.trim()} ${result.url}` : result.url;
        sendMessage(target, msgText);
      }

      if (pendingUpload.preview) URL.revokeObjectURL(pendingUpload.preview);
      setPendingUpload(null);
      setText('');
    } catch (e: any) {
      setPendingUpload((p) => p ? { ...p, uploading: false, error: e.message || 'Upload failed' } : null);
    }
  }, [pendingUpload, authDid, activeChannel, text, ch]);

  // ── Drag & drop ──

  const onDragOver = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer?.types.includes('Files')) setDragOver(true);
  };

  const onDragLeave = (e: DragEvent) => {
    e.preventDefault();
    setDragOver(false);
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
    const file = e.dataTransfer?.files[0];
    if (file) handleFileSelect(file);
  };

  // ── Paste ──

  const onPaste = useCallback((e: React.ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of items) {
      if (item.kind === 'file') {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) handleFileSelect(file);
        return;
      }
    }
  }, [handleFileSelect]);

  const submit = useCallback(() => {
    // If there's a pending upload, do that instead of sending text
    if (pendingUpload && !pendingUpload.uploading) {
      doUpload();
      return;
    }

    const trimmed = text.trim();
    if (!trimmed) return;
    setHistory((h) => [...h.slice(-100), trimmed]);
    setHistoryPos(-1);

    if (trimmed.startsWith('/')) {
      handleCommand(trimmed, activeChannel);
    } else if (activeChannel !== 'server') {
      const target = ch?.name || activeChannel;
      sendMessage(target, trimmed);
    }
    setText('');
    setAutocomplete(null);
    if (inputRef.current) inputRef.current.style.height = 'auto';
  }, [text, activeChannel, ch, pendingUpload, doUpload]);

  const onKeyDown = (e: KeyboardEvent) => {
    // Tab completion
    if (e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      if (autocomplete) {
        acceptAutocomplete(autocomplete.items[autocomplete.selected]);
      } else {
        const el = inputRef.current;
        if (el) {
          const pos = el.selectionStart || 0;
          const before = text.slice(0, pos);
          const spIdx = before.lastIndexOf(' ');
          const partial = before.slice(spIdx + 1).toLowerCase();
          if (partial.length > 0) {
            const isAtPrefix = partial.startsWith('@');
            const search = isAtPrefix ? partial.slice(1) : partial;
            const match = memberNicks.find((n) => n.toLowerCase().startsWith(search));
            if (match) {
              const prefix = isAtPrefix ? '@' : '';
              const suffix = spIdx < 0 ? ': ' : ' ';
              const newText = before.slice(0, spIdx + 1) + prefix + match + suffix + text.slice(pos);
              setText(newText);
              setAutocomplete(null);
            }
          }
        }
      }
      return;
    }

    // Autocomplete navigation
    if (autocomplete) {
      if (e.key === 'Enter') {
        e.preventDefault();
        acceptAutocomplete(autocomplete.items[autocomplete.selected]);
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setAutocomplete({ ...autocomplete, selected: Math.min(autocomplete.selected + 1, autocomplete.items.length - 1) });
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setAutocomplete({ ...autocomplete, selected: Math.max(autocomplete.selected - 1, 0) });
        return;
      }
      if (e.key === 'Escape') {
        setAutocomplete(null);
        return;
      }
    }

    // Escape cancels pending upload
    if (e.key === 'Escape' && pendingUpload) {
      cancelUpload();
      return;
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    } else if (e.key === 'ArrowUp' && !text) {
      e.preventDefault();
      if (history.length > 0) {
        const pos = historyPos < 0 ? history.length - 1 : Math.max(0, historyPos - 1);
        setHistoryPos(pos);
        setText(history[pos] || '');
      }
    } else if (e.key === 'ArrowDown' && historyPos >= 0) {
      e.preventDefault();
      const pos = historyPos + 1;
      if (pos >= history.length) {
        setHistoryPos(-1);
        setText('');
      } else {
        setHistoryPos(pos);
        setText(history[pos] || '');
      }
    }
  };

  const onInput = () => {
    const el = inputRef.current;
    if (el) {
      el.style.height = 'auto';
      el.style.height = Math.min(el.scrollHeight, 120) + 'px';
      updateAutocomplete(el.value, el.selectionStart || 0);
    }
  };

  const canSend = activeChannel !== 'server' || text.startsWith('/');

  return (
    <div
      className={`border-t border-border bg-bg-secondary shrink-0 relative ${dragOver ? 'ring-2 ring-accent/50 ring-inset' : ''}`}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {/* Drag overlay */}
      {dragOver && (
        <div className="absolute inset-0 bg-accent/5 flex items-center justify-center z-30 pointer-events-none">
          <div className="bg-bg-secondary border-2 border-dashed border-accent rounded-xl px-6 py-4 text-accent font-medium">
            Drop file to upload
          </div>
        </div>
      )}

      {/* Typing indicator */}
      {typingMembers.length > 0 && (
        <div className="px-4 py-1 text-xs text-fg-dim animate-fadeIn">
          <span className="inline-flex gap-0.5 mr-1">
            <span className="w-1 h-1 bg-fg-dim rounded-full animate-bounce" style={{ animationDelay: '0ms' }} />
            <span className="w-1 h-1 bg-fg-dim rounded-full animate-bounce" style={{ animationDelay: '150ms' }} />
            <span className="w-1 h-1 bg-fg-dim rounded-full animate-bounce" style={{ animationDelay: '300ms' }} />
          </span>
          {typingMembers.length === 1
            ? `${typingMembers[0]} is typing`
            : `${typingMembers.slice(0, 3).join(', ')} are typing`}
        </div>
      )}

      {/* Pending upload preview */}
      {pendingUpload && (
        <div className="px-3 py-2 border-b border-border flex items-center gap-3 animate-fadeIn">
          {pendingUpload.preview ? (
            <img src={pendingUpload.preview} alt="" className="w-16 h-16 rounded-lg object-cover border border-border" />
          ) : (
            <div className="w-16 h-16 rounded-lg border border-border bg-bg-tertiary flex items-center justify-center text-fg-dim text-xl">
              📎
            </div>
          )}
          <div className="flex-1 min-w-0">
            <div className="text-sm text-fg truncate">{pendingUpload.file.name}</div>
            <div className="text-xs text-fg-dim">
              {(pendingUpload.file.size / 1024).toFixed(0)} KB · {pendingUpload.file.type}
            </div>
            {pendingUpload.error && (
              <div className="text-xs text-danger mt-0.5">{pendingUpload.error}</div>
            )}
          </div>
          {pendingUpload.uploading ? (
            <svg className="animate-spin w-5 h-5 text-accent shrink-0" viewBox="0 0 24 24">
              <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" fill="none" />
              <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
            </svg>
          ) : (
            <button onClick={cancelUpload} className="text-fg-dim hover:text-danger text-lg shrink-0 p-1" title="Cancel">
              ✕
            </button>
          )}
        </div>
      )}

      {/* Autocomplete dropdown */}
      {autocomplete && (
        <div className="absolute bottom-full left-3 mb-1 bg-bg-secondary border border-border rounded-lg shadow-2xl overflow-hidden animate-fadeIn z-20 min-w-[200px]">
          {autocomplete.items.map((item, i) => (
            <button
              key={item}
              onClick={() => acceptAutocomplete(item)}
              onMouseEnter={() => setAutocomplete({ ...autocomplete, selected: i })}
              className={`w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 ${
                i === autocomplete.selected ? 'bg-bg-tertiary text-fg' : 'text-fg-muted'
              }`}
            >
              {item.startsWith('#') ? (
                <span className="text-accent text-xs">#</span>
              ) : (
                <span className="text-purple text-xs">@</span>
              )}
              {item.replace(/^#/, '')}
            </button>
          ))}
        </div>
      )}

      <div className="flex items-end gap-2 px-3 py-2">
        {/* File upload button (only for AT-authenticated users) */}
        {authDid && activeChannel !== 'server' && (
          <>
            <button
              onClick={() => fileInputRef.current?.click()}
              className="w-9 h-9 rounded-lg flex items-center justify-center text-fg-dim hover:text-fg-muted hover:bg-bg-tertiary shrink-0"
              title="Upload file (or drag & drop, or paste)"
            >
              <svg className="w-4 h-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
                <path d="M14 10v3a1 1 0 01-1 1H3a1 1 0 01-1-1v-3M11 5L8 2M8 2L5 5M8 2v8" />
              </svg>
            </button>
            <input
              ref={fileInputRef}
              type="file"
              className="hidden"
              accept="image/*,video/mp4,video/webm,audio/mpeg,audio/ogg,application/pdf"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) handleFileSelect(file);
                e.target.value = '';
              }}
            />
          </>
        )}

        {/* Emoji button */}
        <button
          ref={emojiRef}
          onClick={() => setShowEmoji(!showEmoji)}
          className="w-9 h-9 rounded-lg flex items-center justify-center text-fg-dim hover:text-fg-muted hover:bg-bg-tertiary shrink-0"
          title="Emoji"
        >
          😊
        </button>

        {/* Compose area */}
        <div className="flex-1 bg-bg-tertiary rounded-lg border border-border focus-within:border-accent/50 flex items-end">
          <textarea
            ref={inputRef}
            value={text}
            onChange={(e) => { setText(e.target.value); onInput(); }}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            placeholder={
              pendingUpload
                ? 'Add a caption (optional)...'
                : activeChannel === 'server'
                  ? 'Type /help for commands...'
                  : `Message ${ch?.name || activeChannel}`
            }
            rows={1}
            className="flex-1 bg-transparent px-3 py-2 text-sm text-fg outline-none placeholder:text-fg-dim resize-none min-h-[36px] max-h-[120px] leading-relaxed"
            autoComplete="off"
            spellCheck
          />
        </div>

        {/* Send */}
        <button
          onClick={submit}
          disabled={(!text.trim() && !pendingUpload) || !canSend}
          className={`w-9 h-9 rounded-lg flex items-center justify-center shrink-0 ${
            (text.trim() || pendingUpload) && canSend
              ? 'bg-accent text-black hover:bg-accent-hover'
              : 'bg-bg-tertiary text-fg-dim cursor-not-allowed'
          }`}
          title={pendingUpload ? 'Upload' : 'Send'}
        >
          {pendingUpload ? (
            <svg className="w-4 h-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
              <path d="M14 10v3a1 1 0 01-1 1H3a1 1 0 01-1-1v-3M11 5L8 2M8 2L5 5M8 2v8" />
            </svg>
          ) : (
            <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor">
              <path d="M15.854 8.354a.5.5 0 000-.708L12.207 4l-.707.707L14.293 7.5H1v1h13.293l-2.793 2.793.707.707 3.647-3.646z"/>
            </svg>
          )}
        </button>
      </div>

      {/* Emoji picker */}
      {showEmoji && (
        <div className="absolute bottom-full left-3 mb-2 z-50">
          <EmojiPicker
            onSelect={(emoji) => {
              setText((t) => t + emoji);
              setShowEmoji(false);
              inputRef.current?.focus();
            }}
            onClose={() => setShowEmoji(false)}
          />
        </div>
      )}
    </div>
  );
}

function handleCommand(text: string, activeChannel: string) {
  const sp = text.indexOf(' ');
  const cmd = (sp > 0 ? text.slice(1, sp) : text.slice(1)).toLowerCase();
  const args = sp > 0 ? text.slice(sp + 1) : '';
  const store = useStore.getState();
  const target = activeChannel !== 'server'
    ? store.channels.get(activeChannel.toLowerCase())?.name || activeChannel
    : '';

  switch (cmd) {
    case 'join': case 'j':
      args.split(',').map((s) => s.trim()).filter(Boolean).forEach((c) =>
        joinChannel(c.startsWith('#') ? c : `#${c}`)
      );
      break;
    case 'part': case 'leave':
      partChannel(args || target);
      break;
    case 'topic': case 't':
      if (target) setTopic(target, args);
      break;
    case 'mode': case 'm':
      if (args) rawCommand(`MODE ${args.startsWith('#') ? '' : target + ' '}${args}`);
      else if (target) rawCommand(`MODE ${target}`);
      break;
    case 'kick': case 'k': {
      const kp = args.split(' ');
      if (kp[0] && target) kickUser(target, kp[0], kp.slice(1).join(' ') || undefined);
      break;
    }
    case 'op': if (args && target) setMode(target, '+o', args); break;
    case 'deop': if (args && target) setMode(target, '-o', args); break;
    case 'voice': if (args && target) setMode(target, '+v', args); break;
    case 'invite': if (args && target) inviteUser(target, args); break;
    case 'away': setAway(args || undefined); break;
    case 'whois': case 'wi': if (args) sendWhois(args); break;
    case 'msg': case 'query': {
      const mp = args.split(' ');
      if (mp[0] && mp[1]) sendMessage(mp[0], mp.slice(1).join(' '));
      break;
    }
    case 'me': case 'action':
      if (target) rawCommand(`PRIVMSG ${target} :\x01ACTION ${args}\x01`);
      break;
    case 'raw': case 'quote':
      rawCommand(args);
      break;
    case 'help':
      store.addSystemMessage(activeChannel, '── Commands ──');
      store.addSystemMessage(activeChannel, '/join #channel  ·  /part  ·  /topic text');
      store.addSystemMessage(activeChannel, '/kick user  ·  /op user  ·  /voice user  ·  /invite user');
      store.addSystemMessage(activeChannel, '/whois user  ·  /away reason  ·  /me action');
      store.addSystemMessage(activeChannel, '/msg user text  ·  /mode +o user  ·  /raw IRC_LINE');
      break;
    default:
      rawCommand(`${cmd.toUpperCase()}${args ? ' ' + args : ''}`);
  }
}
