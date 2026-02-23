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

// Channels we're currently in (for rejoin on reconnect)
let joinedChannels = new Set<string>();

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
        ackedCaps = new Set(); // reset caps for new connection
        raw('CAP LS 302');
        raw(`NICK ${nick}`);
        raw(`USER ${nick} 0 * :freeq web app`);
      }
    },
  });
  transport.connect();

  // Send QUIT when tab/window is closing to avoid ghost connections
  window.addEventListener('beforeunload', () => {
    if (transport) {
      try { transport.send('QUIT :Leaving'); } catch { /* ignore */ }
    }
  });
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
  joinedChannels.clear();
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

export function sendReply(target: string, replyToMsgId: string, text: string) {
  const line = format('PRIVMSG', [target, text], { '+reply': replyToMsgId });
  raw(line);

  // Ensure DM buffer exists
  const isChannel = target.startsWith('#') || target.startsWith('&');
  if (!isChannel) {
    const store = useStore.getState();
    if (!store.channels.has(target.toLowerCase())) {
      store.addChannel(target);
    }
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

    // ── ERROR (server closing link) ──
    case 'ERROR': {
      const reason = msg.params[0] || '';
      // If ghosted (same identity reconnected elsewhere), don't auto-reconnect
      if (reason.includes('same identity reconnected')) {
        transport?.disconnect(); // sets intentionalClose = true, prevents reconnect
        useStore.getState().fullReset();
      }
      break;
    }

    // ── Registration ──
    case '001': {
      const serverNick = msg.params[0] || nick;

      // If we were authenticated but server gave us a Guest nick,
      // it means our identity was lost (web-token consumed on previous session).
      // Disconnect cleanly instead of lingering as a ghost Guest.
      const wasAuthenticated = localStorage.getItem('freeq-handle');
      if (wasAuthenticated && /^Guest\d+$/i.test(serverNick)) {
        raw('QUIT :Session expired');
        transport?.disconnect();
        transport = null;
        // Don't reset to login screen — just stop reconnecting as a ghost
        return;
      }

      nick = serverNick;
      store.setNick(nick);
      store.setRegistered(true);
      // Auto-join channels (first connect uses autoJoinChannels, reconnects use joinedChannels)
      const toJoin = autoJoinChannels.length > 0
        ? autoJoinChannels
        : [...joinedChannels];
      for (const ch of toJoin) {
        if (ch.trim()) raw(`JOIN ${ch.trim()}`);
      }
      autoJoinChannels = [];
      break;
    }
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
        joinedChannels.add(channel.toLowerCase());
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
        joinedChannels.delete(channel.toLowerCase());
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
        joinedChannels.delete(channel.toLowerCase());
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
      const isMention = !message.isSelf && text.toLowerCase().includes(nick.toLowerCase());
      const isDM = !isChannel && !message.isSelf;
      if (isMention) {
        store.incrementMentions(bufName);
      }
      if (isDM) {
        store.incrementMentions(bufName);
      }
      if ((isMention || isDM) && !useStore.getState().mutedChannels.has(bufName.toLowerCase())) {
        notify(
          isDM ? `DM from ${from}` : bufName,
          `${from}: ${text.slice(0, 100)}`,
          () => useStore.getState().setActiveChannel(bufName),
        );
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
        const reactTarget = msg.tags['+reply'];
        if (reactTarget) {
          store.addReaction(target, reactTarget, reaction, from);
        } else {
          // No +reply — reaction to the channel generally.
          // Attach to the most recent non-system message.
          const ch = store.channels.get(target.toLowerCase());
          if (ch) {
            const lastMsg = [...ch.messages].reverse().find((m) => !m.isSystem && !m.deleted);
            if (lastMsg) {
              store.addReaction(target, lastMsg.id, reaction, from);
            }
          }
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
    case '366': { // End of NAMES — request history and WHOIS members for avatars
      const namesChannel = msg.params[1];
      // Fetch recent history for the channel
      requestHistory(namesChannel);
      const ch = store.channels.get(namesChannel?.toLowerCase());
      if (ch) {
        const toWhois: string[] = [];
        for (const m of ch.members.values()) {
          if (!m.did && m.nick.toLowerCase() !== nick.toLowerCase()) {
            toWhois.push(m.nick);
          }
        }
        // Stagger WHOIS to avoid flooding
        for (const n of toWhois) {
          backgroundWhois.add(n.toLowerCase());
          raw(`WHOIS ${n}`);
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
    case '477': {
      const ch = msg.params[1] || '';
      const reason = msg.params[2] || 'Policy acceptance required';
      store.addSystemMessage('server', `Cannot join ${ch}: ${reason}`);
      // Open the join gate modal if user has a DID (authenticated)
      if (useStore.getState().authDid) {
        useStore.getState().setJoinGateChannel(ch);
      }
      break;
    }
    case '482': store.addSystemMessage(msg.params[1] || 'server', msg.params[2] || 'Not operator'); break;

    // ── WHOIS ──
    case '311': { // RPL_WHOISUSER: nick user host * :realname
      const whoisNick = msg.params[1] || '';
      store.updateWhois(whoisNick, {
        user: msg.params[2],
        host: msg.params[3],
        realname: msg.params[5] || msg.params[4],
      });
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `WHOIS ${whoisNick}: ${msg.params[2]}@${msg.params[3]} (${msg.params[5] || msg.params[4]})`);
      }
      break;
    }
    case '312': { // RPL_WHOISSERVER
      const whoisNick = msg.params[1] || '';
      store.updateWhois(whoisNick, { server: msg.params[2] });
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
    case '319': { // RPL_WHOISCHANNELS
      const whoisNick = msg.params[1] || '';
      store.updateWhois(whoisNick, { channels: msg.params[2] });
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `  Channels: ${msg.params[2]}`);
      }
      break;
    }
    case '330': { // RPL_WHOISACCOUNT (DID)
      const whoisNick = msg.params[1] || '';
      const did = msg.params[2] || '';
      store.updateWhois(whoisNick, { did });
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
      const handle = msg.params[2] || '';
      store.updateWhois(whoisNick, { handle });
      if (!backgroundWhois.has(whoisNick.toLowerCase())) {
        store.addSystemMessage('server', `  Handle: ${handle}`);
      }
      break;
    }

    // ── Channel list ──
    case '321': // RPL_LISTSTART
      store.setChannelList([]);
      break;
    case '322': { // RPL_LIST
      const chName = msg.params[1] || '';
      const chCount = parseInt(msg.params[2] || '0', 10);
      const chTopic = msg.params[3] || '';
      store.addChannelListEntry({ name: chName, topic: chTopic, count: chCount });
      store.addSystemMessage('server', `  ${chName} (${chCount}) ${chTopic}`);
      break;
    }
    case '323': // RPL_LISTEND
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
