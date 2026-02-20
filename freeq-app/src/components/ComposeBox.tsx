import { useState, useRef, useCallback, useEffect, type KeyboardEvent } from 'react';
import { useStore } from '../store';
import { sendMessage, joinChannel, partChannel, setTopic, setMode, kickUser, inviteUser, setAway, rawCommand, sendWhois } from '../irc/client';

export function ComposeBox() {
  const [text, setText] = useState('');
  const [history, setHistory] = useState<string[]>([]);
  const [historyPos, setHistoryPos] = useState(-1);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const activeChannel = useStore((s) => s.activeChannel);
  const channels = useStore((s) => s.channels);
  const ch = channels.get(activeChannel.toLowerCase());

  // Typing members
  const typingMembers = ch
    ? [...ch.members.values()].filter((m) => m.typing).map((m) => m.nick)
    : [];

  // Focus input on channel switch
  useEffect(() => {
    inputRef.current?.focus();
  }, [activeChannel]);

  const submit = useCallback(() => {
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
    // Reset textarea height
    if (inputRef.current) inputRef.current.style.height = 'auto';
  }, [text, activeChannel, ch]);

  const onKeyDown = (e: KeyboardEvent) => {
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

  // Auto-resize textarea
  const onInput = () => {
    const el = inputRef.current;
    if (el) {
      el.style.height = 'auto';
      el.style.height = Math.min(el.scrollHeight, 120) + 'px';
    }
  };

  const canSend = activeChannel !== 'server' || text.startsWith('/');

  return (
    <div className="border-t border-border bg-bg-secondary shrink-0">
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

      <div className="flex items-end gap-2 px-3 py-2">
        {/* Compose area */}
        <div className="flex-1 bg-bg-tertiary rounded-lg border border-border focus-within:border-accent/50 flex items-end">
          <textarea
            ref={inputRef}
            value={text}
            onChange={(e) => { setText(e.target.value); onInput(); }}
            onKeyDown={onKeyDown}
            placeholder={
              activeChannel === 'server'
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
          disabled={!text.trim() || !canSend}
          className={`w-9 h-9 rounded-lg flex items-center justify-center shrink-0 transition-all ${
            text.trim() && canSend
              ? 'bg-accent text-black hover:bg-accent-hover'
              : 'bg-bg-tertiary text-fg-dim cursor-not-allowed'
          }`}
        >
          <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor">
            <path d="M15.854 8.354a.5.5 0 000-.708L12.207 4l-.707.707L14.293 7.5H1v1h13.293l-2.793 2.793.707.707 3.647-3.646z"/>
          </svg>
        </button>
      </div>
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
