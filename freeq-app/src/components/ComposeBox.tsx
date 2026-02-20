import { useState, useRef, useCallback, type KeyboardEvent } from 'react';
import { useStore } from '../store';
import { sendMessage, joinChannel, partChannel, setTopic, setMode, kickUser, inviteUser, setAway, rawCommand, sendWhois } from '../irc/client';

export function ComposeBox() {
  const [text, setText] = useState('');
  const [history, setHistory] = useState<string[]>([]);
  const [historyPos, setHistoryPos] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);
  const activeChannel = useStore((s) => s.activeChannel);

  const submit = useCallback(() => {
    if (!text.trim()) return;
    setHistory((h) => [...h, text]);
    setHistoryPos(-1);

    if (text.startsWith('/')) {
      handleCommand(text, activeChannel);
    } else if (activeChannel !== 'server') {
      const ch = useStore.getState().channels.get(activeChannel.toLowerCase());
      sendMessage(ch?.name || activeChannel, text);
    }
    setText('');
  }, [text, activeChannel]);

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

  return (
    <div className="border-t border-border bg-bg-secondary flex items-center shrink-0">
      <input
        ref={inputRef}
        type="text"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder={activeChannel === 'server' ? 'Type a /command...' : 'Type a message...'}
        className="flex-1 bg-transparent px-4 py-3 text-sm text-fg outline-none placeholder:text-fg-dim"
        autoComplete="off"
        spellCheck
      />
      <button
        onClick={submit}
        className="px-3 py-2 text-accent opacity-50 hover:opacity-100 transition-opacity"
      >
        ↵
      </button>
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
      args.split(',').map((s) => s.trim()).filter(Boolean).forEach(joinChannel);
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
      if (mp[0] && mp[1]) {
        sendMessage(mp[0], mp.slice(1).join(' '));
      }
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
      store.addSystemMessage(activeChannel, '/join #channel · /part · /topic text · /kick user');
      store.addSystemMessage(activeChannel, '/op user · /deop user · /voice user · /invite user');
      store.addSystemMessage(activeChannel, '/whois user · /away reason · /me action');
      store.addSystemMessage(activeChannel, '/msg user text · /mode +o user · /raw IRC_LINE');
      break;
    default:
      rawCommand(`${cmd.toUpperCase()}${args ? ' ' + args : ''}`);
  }
}
