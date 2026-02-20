/**
 * IRC client adapter.
 *
 * Handles CAP negotiation, SASL auth, and translates IRC events
 * into store actions. The React UI never sees IRC protocol.
 */

import { parse, prefixNick, format, type IRCMessage } from './parser';
import { Transport, type TransportState } from './transport';
import { useStore, type Message } from '../store';
import { notify } from '../lib/notifications';
import { prefetchProfiles } from '../lib/profiles';

// ── State ──

let transport: Transport | null = null;
let nick = '';
let ackedCaps = new Set<string>();

// SASL state (set before connect when doing OAuth)
let saslToken = '';
let saslDid = '';
let saslPdsUrl = '';
let saslMethod = '';

// Auto-join channels after registration
let autoJoinChannels: string[] = [];

// Background WHOIS lookups (suppress output for these)
const backgroundWhois = new Set<string>();

// ── Public API (called by UI) ──

export function connect(url: string, desiredNick: string, channels?: string[]) {
  nick = desiredNick;
  autoJoinChannels = channels || [];
  const store = useStore.getState();
  store.reset();

  transport = new Transport({
    url,
    onLine: handleLine,
    onStateChange: (s: TransportState) => {
      useStore.getState().setConnectionState(s);
      if (s === 'connected') {
        raw('CAP LS 302');
        raw(`NICK ${nick}`);
        raw(`USER ${nick} 0 * :freeq web app`);
      }
    },
  });
  transport.connect();
}

export function disconnect() {
  transport?.disconnect();
  transport = null;
  nick = '';
  ackedCaps = new Set();
  saslToken = '';
  saslDid = '';
  saslPdsUrl = '';
  saslMethod = '';
  useStore.getState().fullReset();
}

export function setSaslCredentials(token: string, did: string, pdsUrl: string, method: string) {
  saslToken = token;
  saslDid = did;
  saslPdsUrl = pdsUrl;
  saslMethod = method;
}

export function sendMessage(target: string, text: string) {
  raw(`PRIVMSG ${target} :${text}`);

  // Ensure DM buffer exists
  const isChannel = target.startsWith('#') || target.startsWith('&');
  if (!isChannel) {
    const store = useStore.getState();
    if (!store.channels.has(target.toLowerCase())) {
      store.addChannel(target);
    }
  }

  // If we have echo-message, server will echo it back.
  // Otherwise, add it locally.
  if (!ackedCaps.has('echo-message')) {
    useStore.getState().addMessage(target, {
      id: crypto.randomUUID(),
      from: nick,
      text,
      timestamp: new Date(),
      tags: {},
      isSelf: true,
    });
  }
}

export function sendEdit(target: string, originalMsgId: string, newText: string) {
  const line = format('PRIVMSG', [target, newText], { '+draft/edit': originalMsgId });
  raw(line);
}

export function sendDelete(target: string, msgId: string) {
  const line = format('TAGMSG', [target], { '+draft/delete': msgId });
  raw(line);
}

export function sendReaction(target: string, emoji: string, msgId?: string) {
  const tags: Record<string, string> = { '+react': emoji };
  if (msgId) tags['+reply'] = msgId;
  raw(format('TAGMSG', [target], tags));
}

export function joinChannel(channel: string) {
  raw(`JOIN ${channel}`);
}

export function partChannel(channel: string) {
  raw(`PART ${channel}`);
}

export function setTopic(channel: string, topic: string) {
  raw(`TOPIC ${channel} :${topic}`);
}

export function setMode(channel: string, mode: string, arg?: string) {
  raw(arg ? `MODE ${channel} ${mode} ${arg}` : `MODE ${channel} ${mode}`);
}

export function kickUser(channel: string, userNick: string, reason?: string) {
  raw(`KICK ${channel} ${userNick} :${reason || 'kicked'}`);
}

export function inviteUser(channel: string, userNick: string) {
  raw(`INVITE ${userNick} ${channel}`);
}

export function setAway(reason?: string) {
  raw(reason ? `AWAY :${reason}` : 'AWAY');
}

export function sendWhois(userNick: string) {
  raw(`WHOIS ${userNick}`);
}

export function requestHistory(channel: string, before?: string) {
  if (before) {
    raw(`CHATHISTORY BEFORE ${channel} timestamp=${before} 50`);
  } else {
    raw(`CHATHISTORY LATEST ${channel} * 50`);
  }
}

export function rawCommand(line: string) {
  raw(line);
}

export function getNick(): string {
  return nick;
}

// ── Internals ──

function raw(line: string) {
  transport?.send(line);
}

function handleLine(rawLine: string) {
  const msg = parse(rawLine);
  const store = useStore.getState();
  const from = prefixNick(msg.prefix);

  switch (msg.command) {
    // ── CAP negotiation ──
    case 'CAP':
      handleCap(msg);
      break;

    // ── SASL ──
    case 'AUTHENTICATE':
      handleAuthenticate(msg);
      break;
    case '900':
      store.setAuth(saslDid, msg.params[msg.params.length - 1]);
      if (saslDid) prefetchProfiles([saslDid]);
      break;
    case '903':
      raw('CAP END');
      break;
    case '904':
      store.setAuthError(msg.params[msg.params.length - 1] || 'SASL failed');
      raw('CAP END');
      break;

    case 'PING':
      raw(`PONG :${msg.params[0] || ''}`);
      break;

    // ── Registration ──
    case '001':
      nick = msg.params[0] || nick;
      store.setNick(nick);
      store.setRegistered(true);
      // Auto-join channels
      for (const ch of autoJoinChannels) {
        if (ch.trim()) raw(`JOIN ${ch.trim()}`);
      }
      autoJoinChannels = [];
      break;
    case '433': // Nick in use
      nick += '_';
      raw(`NICK ${nick}`);
      break;

    case 'NICK': {
      const newNick = msg.params[0];
      if (from === nick) {
        nick = newNick;
        store.setNick(nick);
      }
      store.renameUser(from, newNick);
      break;
    }

    case 'JOIN': {
      const channel = msg.params[0];
      const account = msg.params[1]; // extended-join
      if (from === nick) {
        store.addChannel(channel);
        store.setActiveChannel(channel);
      }
      const joinDid = account && account !== '*' ? account : undefined;
      store.addMember(channel, {
        nick: from,
        did: joinDid,
        isOp: false,
        isVoiced: false,
      });
      if (joinDid) prefetchProfiles([joinDid]);
      store.addSystemMessage(channel, `${from} joined`);
      break;
    }

    case 'PART': {
      const channel = msg.params[0];
      if (from === nick) {
        store.removeChannel(channel);
      } else {
        store.removeMember(channel, from);
        store.addSystemMessage(channel, `${from} left`);
      }
      break;
    }

    case 'QUIT': {
      const reason = msg.params[0] || '';
      store.removeUserFromAll(from, reason);
      break;
    }

    case 'KICK': {
      const channel = msg.params[0];
      const kicked = msg.params[1];
      const reason = msg.params[2] || '';
      if (kicked.toLowerCase() === nick.toLowerCase()) {
        store.removeChannel(channel);
        store.addSystemMessage('server', `Kicked from ${channel} by ${from}: ${reason}`);
      } else {
        store.removeMember(channel, kicked);
        store.addSystemMessage(channel, `${kicked} kicked by ${from}${reason ? `: ${reason}` : ''}`);
      }
      break;
    }

    // ── PRIVMSG / NOTICE ──
    case 'PRIVMSG': {
      const target = msg.params[0];
      const text = msg.params[1] || '';
      const isAction = text.startsWith('\x01ACTION ') && text.endsWith('\x01');
      // For channels, buffer = channel name. For DMs, buffer = the other person's nick.
      const isChannel = target.startsWith('#') || target.startsWith('&');
      const isSelf = from.toLowerCase() === nick.toLowerCase();
      const bufName = isChannel ? target : (isSelf ? target : from);

      // Handle edits
      const editOf = msg.tags['+draft/edit'];
      if (editOf) {
        store.editMessage(bufName, editOf, text, msg.tags['msgid']);
        break;
      }

      const message: Message = {
        id: msg.tags['msgid'] || crypto.randomUUID(),
        from,
        text: isAction ? text.slice(8, -1) : text,
        timestamp: msg.tags['time'] ? new Date(msg.tags['time']) : new Date(),
        tags: msg.tags,
        isAction,
        isSelf: isSelf,
        replyTo: msg.tags['+reply'],
      };

      // Ensure DM buffer exists
      if (!isChannel && !store.channels.has(bufName.toLowerCase())) {
        store.addChannel(bufName);
      }

      store.addMessage(bufName, message);

      // Mention detection + notification
      if (!message.isSelf && text.toLowerCase().includes(nick.toLowerCase())) {
        store.incrementMentions(bufName);
        notify(bufName, `${from}: ${text.slice(0, 100)}`);
      }
      break;
    }

    case 'NOTICE': {
      const target = msg.params[0];
      const text = msg.params[1] || '';
      const buf = target === '*' || target === nick ? 'server' : target;
      store.addSystemMessage(buf, `[${from || 'server'}] ${text}`);
      break;
    }

    // ── TAGMSG ──
    case 'TAGMSG': {
      const target = msg.params[0];
      // Handle deletes
      const deleteOf = msg.tags['+draft/delete'];
      if (deleteOf) {
        store.deleteMessage(target, deleteOf);
        break;
      }
      // Handle reactions — +reply tag references the target message
      const reaction = msg.tags['+react'];
      if (reaction) {
        const reactTarget = msg.tags['+reply'] || msg.tags['msgid'];
        if (reactTarget) {
          store.addReaction(target, reactTarget, reaction, from);
        }
      }
      // Handle typing
      const typing = msg.tags['+typing'];
      if (typing) {
        store.setTyping(target, from, typing === 'active');
      }
      break;
    }

    // ── TOPIC ──
    case 'TOPIC': {
      const channel = msg.params[0];
      const topic = msg.params[1] || '';
      store.setTopic(channel, topic, from);
      break;
    }
    case '332': {
      const channel = msg.params[1];
      const topic = msg.params[2] || '';
      store.setTopic(channel, topic);
      break;
    }

    // ── NAMES ──
    case '353': {
      const channel = msg.params[2];
      const nicks = (msg.params[3] || '').split(' ').filter(Boolean);
      for (const n of nicks) {
        const isOp = n.startsWith('@');
        const isVoiced = n.startsWith('+');
        const bare = n.replace(/^[@+]/, '');
        store.addMember(channel, { nick: bare, isOp, isVoiced });
      }
      break;
    }
    case '366': { // End of NAMES — WHOIS members to get DIDs for avatars
      const namesChannel = msg.params[1];
      const ch = store.channels.get(namesChannel?.toLowerCase());
      if (ch) {
        for (const m of ch.members.values()) {
          if (!m.did && m.nick !== nick) {
            backgroundWhois.add(m.nick.toLowerCase());
            raw(`WHOIS ${m.nick}`);
          }
        }
      }
      break;
    }

    // ── MODE ──
    case 'MODE': {
      const target = msg.params[0];
      if (target.startsWith('#') || target.startsWith('&')) {
        const mode = msg.params[1] || '';
        const arg = msg.params[2];
        store.handleMode(target, mode, arg, from);
        store.addSystemMessage(target, `${from} set mode ${mode}${arg ? ' ' + arg : ''}`);
      }
      break;
    }

    // ── AWAY ──
    case 'AWAY': {
      const reason = msg.params[0];
      store.setUserAway(from, reason || null);
      break;
    }

    // ── BATCH ──
    case 'BATCH': {
      const ref = msg.params[0];
      if (ref.startsWith('+')) {
        store.startBatch(ref.slice(1), msg.params[1] || '', msg.params[2] || '');
      } else if (ref.startsWith('-')) {
        store.endBatch(ref.slice(1));
      }
      break;
    }

    // ── INVITE ──
    case 'INVITE':
      if (msg.params.length >= 2) {
        store.addSystemMessage('server', `${from} invited you to ${msg.params[1]}`);
      }
      break;

    // ── Error numerics ──
    case '401': store.addSystemMessage('server', `No such nick: ${msg.params[1]}`); break;
    case '473': store.addSystemMessage('server', `Cannot join ${msg.params[1]} (invite only)`); break;
    case '474': store.addSystemMessage('server', `Cannot join ${msg.params[1]} (banned)`); break;
    case '475': store.addSystemMessage('server', `Cannot join ${msg.params[1]} (bad key)`); break;
    case '482': store.addSystemMessage(msg.params[1] || 'server', msg.params[2] || 'Not operator'); break;

    // ── WHOIS ──
    case '311': {
      const whoisNick = msg.params[1] || '';
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `WHOIS ${whoisNick}: ${msg.params[2]}@${msg.params[3]} (${msg.params[5] || msg.params[4]})`);
      }
      break;
    }
    case '312': { // RPL_WHOISSERVER
      const whoisNick = msg.params[1] || '';
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `  Server: ${msg.params[2]}`);
      }
      break;
    }
    case '318': { // RPL_ENDOFWHOIS
      const whoisNick = msg.params[1] || '';
      backgroundWhois.delete(whoisNick.toLowerCase());
      break;
    }
    case '330': { // RPL_WHOISACCOUNT (DID)
      const whoisNick = msg.params[1] || '';
      const did = msg.params[2] || '';
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `  DID: ${did}`);
      }
      if (whoisNick) {
        store.updateMemberDid(whoisNick, did);
      }
      if (did) {
        prefetchProfiles([did]);
      }
      break;
    }
    case '671': { // AT handle
      const whoisNick = msg.params[1] || '';
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `  Handle: ${msg.params[2]}`);
      }
      break;
    }

    // ── Channel list ──
    case '322': // RPL_LIST
      store.addSystemMessage('server', `  ${msg.params[1]} (${msg.params[2]}) ${msg.params[3] || ''}`);
      break;

    // ── Informational ──
    case '375': case '372':
      store.addSystemMessage('server', msg.params[msg.params.length - 1]);
      break;

    default:
      // Numeric replies → server buffer
      if (/^\d{3}$/.test(msg.command)) {
        store.addSystemMessage('server', msg.params.slice(1).join(' '));
      }
      break;
  }
}

function handleCap(msg: IRCMessage) {
  const sub = (msg.params[1] || '').toUpperCase();
  if (sub === 'LS') {
    const available = msg.params.slice(2).join(' ');
    const wantedCaps: string[] = [];
    const caps = [
      'message-tags', 'server-time', 'batch', 'multi-prefix',
      'echo-message', 'account-notify', 'extended-join', 'away-notify',
      'draft/chathistory',
    ];
    for (const c of caps) {
      if (available.includes(c)) wantedCaps.push(c);
    }
    if (saslToken && available.includes('sasl')) {
      wantedCaps.push('sasl');
    }
    if (wantedCaps.length) {
      raw(`CAP REQ :${wantedCaps.join(' ')}`);
    } else {
      raw('CAP END');
    }
  } else if (sub === 'ACK') {
    const caps = msg.params.slice(2).join(' ');
    for (const c of caps.split(' ')) ackedCaps.add(c);
    if (ackedCaps.has('sasl') && saslToken) {
      raw('AUTHENTICATE ATPROTO-CHALLENGE');
    } else {
      raw('CAP END');
    }
  } else if (sub === 'NAK') {
    raw('CAP END');
  }
}

function handleAuthenticate(msg: IRCMessage) {
  const param = msg.params[0] || '';
  if (param === '+' || !param) return;

  // Server sent the challenge — respond with our credentials
  const response = JSON.stringify({
    did: saslDid,
    method: saslMethod || 'pds-session',
    signature: saslToken,
    pds_url: saslPdsUrl,
  });
  const encoded = btoa(response)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');

  if (encoded.length <= 400) {
    raw(`AUTHENTICATE ${encoded}`);
  } else {
    for (let i = 0; i < encoded.length; i += 400) {
      raw(`AUTHENTICATE ${encoded.slice(i, i + 400)}`);
    }
    raw('AUTHENTICATE +');
  }
}
