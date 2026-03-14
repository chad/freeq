/**
 * IRC client adapter.
 *
 * Handles CAP negotiation, SASL auth, and translates IRC events
 * into store actions. The React UI never sees IRC protocol.
 */

import { parse, prefixNick, format, type IRCMessage } from './parser';
import { Transport, type TransportState } from './transport';
import { useStore, type Message, type Member } from '../store';
import { notify } from '../lib/notifications';
import { prefetchProfiles } from '../lib/profiles';
import * as e2ee from '../lib/e2ee';
import * as signing from './signing';

// ── State ──

let transport: Transport | null = null;
let nick = '';
let lastUrl = '';
let ackedCaps = new Set<string>();

// SASL state (set before connect when doing OAuth)
let saslToken = '';
let saslDid = '';
let saslPdsUrl = '';
let saslMethod = '';
let skipBrokerRefresh = false; // Set when we already have a fresh token (e.g. from OAuth)
let guestFallbackCount = 0; // Track retries when authenticated user gets Guest nick

// Auto-join channels after registration
let autoJoinChannels: string[] = [];

// Channels we're currently in (for rejoin on reconnect)
let joinedChannels = new Set<string>();

const SAVED_CHANNELS_KEY = 'freeq-joined-channels';

function saveJoinedChannels() {
  try {
    localStorage.setItem(SAVED_CHANNELS_KEY, JSON.stringify([...joinedChannels]));
  } catch { /* quota exceeded, etc */ }
}

function loadSavedChannels(): string[] {
  try {
    const stored = localStorage.getItem(SAVED_CHANNELS_KEY);
    if (stored) return JSON.parse(stored);
  } catch { /* corrupted */ }
  return [];
}

// Background WHOIS lookups (suppress output for these)
const backgroundWhois = new Set<string>();

// ── Public API (called by UI) ──

export function connect(url: string, desiredNick: string, channels?: string[]) {
  // Clean up any existing transport to prevent duplicate connections
  if (transport) {
    try { transport.disconnect(); } catch { /* ignore */ }
    transport = null;
  }
  nick = desiredNick;
  lastUrl = url;
  autoJoinChannels = channels || [];
  const store = useStore.getState();
  store.reset();

  // Serialize async line handling to prevent race conditions.
  // Without this, BATCH end can be processed before async PRIVMSG handlers
  // (which await E2EE decryption) have finished adding messages to the batch.
  let lineQueue: Promise<void> = Promise.resolve();
  const serializedHandleLine = (line: string) => {
    lineQueue = lineQueue.then(() => handleLine(line)).catch((e) => console.error('[irc] line handler error:', e));
  };

  transport = new Transport({
    url,
    onLine: serializedHandleLine,
    onStateChange: (s: TransportState) => {
      useStore.getState().setConnectionState(s);
      if (s === 'connected') {
        ackedCaps = new Set(); // reset caps for new connection
        let registrationSent = false;
        const sendRegistration = (token?: string) => {
          if (registrationSent) return;
          registrationSent = true;
          if (token) saslToken = token;
          raw('CAP LS 302');
          raw(`NICK ${nick}`);
          raw(`USER ${nick} 0 * :freeq web app`);
        };

        // Safety net: if registration hasn't been sent in 8s, send it as guest
        const safetyTimer = setTimeout(() => {
          if (!registrationSent) {
            console.warn('[irc] Registration safety timeout — sending as guest');
            saslToken = '';
            saslDid = '';
            saslMethod = '';
            sendRegistration();
          }
        }, 8000);

        // If we have a broker token and SASL credentials, refresh the web-token
        // before registering (web-tokens are one-time use).
        // Skip if we already have a fresh token (e.g. just came from OAuth).
        const brokerToken = localStorage.getItem('freeq-broker-token');
        const brokerBase = localStorage.getItem('freeq-broker-base');
        if (skipBrokerRefresh && saslToken) {
          skipBrokerRefresh = false;
          clearTimeout(safetyTimer);
          sendRegistration();
        } else if (brokerToken && brokerBase && saslDid) {
          const ctrl = new AbortController();
          const tm = setTimeout(() => ctrl.abort(), 8000);
          // Broker /session returns 502 on first call due to DPoP nonce rotation —
          // retry up to 2 times on transient errors.
          const brokerBody = JSON.stringify({ broker_token: brokerToken });
          const doFetch = () => fetch(`${brokerBase}/session`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: brokerBody,
            signal: ctrl.signal,
          });
          const fetchWithRetry = async (): Promise<any> => {
            for (let attempt = 0; attempt < 3; attempt++) {
              try {
                const r = await doFetch();
                if (r.status === 502 && attempt < 2) {
                  await new Promise(resolve => setTimeout(resolve, 500 * (attempt + 1)));
                  continue;
                }
                if (r.status === 401) {
                  // Token genuinely invalid — clear it
                  localStorage.removeItem('freeq-broker-token');
                  throw new Error('broker token invalid');
                }
                if (!r.ok) throw new Error('broker refresh failed');
                return r.json();
              } catch (e: any) {
                if (e?.name === 'AbortError' || attempt >= 2) throw e;
                await new Promise(resolve => setTimeout(resolve, 500 * (attempt + 1)));
              }
            }
            throw new Error('broker fetch exhausted retries');
          };
          fetchWithRetry()
            .then((session: { token: string; nick: string; did: string; handle: string }) => {
              clearTimeout(tm);
              clearTimeout(safetyTimer);
              sendRegistration(session.token);
            })
            .catch(() => {
              clearTimeout(tm);
              clearTimeout(safetyTimer);
              // Broker refresh failed — still try with existing saslToken if we have one.
              // Only fall back to guest if we have no token at all.
              if (saslToken) {
                sendRegistration();
              } else {
                saslToken = '';
                saslDid = '';
                saslMethod = '';
                sendRegistration();
              }
            });
        } else {
          clearTimeout(safetyTimer);
          sendRegistration();
        }
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
  signing.resetSigning();
  useStore.getState().fullReset();
}

/** Force an immediate reconnect (e.g. from the reconnect button). */
export function reconnect() {
  if (!lastUrl || !nick) return;
  transport?.disconnect();
  transport = null;
  // Re-run connect with existing state — broker token refresh happens in onStateChange
  const channels = [...joinedChannels];
  const store = useStore.getState();
  store.reset();
  connect(lastUrl, nick, channels);
}

export function setSaslCredentials(token: string, did: string, pdsUrl: string, method: string) {
  saslToken = token;
  saslDid = did;
  saslPdsUrl = pdsUrl;
  saslMethod = method;
  // If we're given a fresh token, don't waste it by calling broker again
  if (token) skipBrokerRefresh = true;
}

/** Resolve a nick to a DID by searching member lists across all channels. */
function didForNick(targetNick: string): string | undefined {
  const store = useStore.getState();
  const lower = targetNick.toLowerCase();
  for (const ch of store.channels.values()) {
    const m = ch.members.get(lower);
    if (m?.did) return m.did;
  }
  return undefined;
}

/**
 * Cache of plaintext for outgoing encrypted messages, keyed by ciphertext.
 * Needed because echo-message returns our own ciphertext and we can't
 * decrypt what we encrypted (Double Ratchet is asymmetric: send ≠ recv chain).
 * Entries auto-expire after 60 seconds.
 */
const echoPlaintextCache = new Map<string, { plaintext: string; ts: number }>();
function cacheEchoPlaintext(ciphertext: string, plaintext: string) {
  echoPlaintextCache.set(ciphertext, { plaintext, ts: Date.now() });
  // Prune old entries
  if (echoPlaintextCache.size > 100) {
    const now = Date.now();
    for (const [k, v] of echoPlaintextCache) {
      if (now - v.ts > 60_000) echoPlaintextCache.delete(k);
    }
  }
}

/** Send a signed PRIVMSG (async — signs then sends) */
async function signedPrivmsg(target: string, text: string, extraTags?: Record<string, string>) {
  const sig = await signing.signMessage(target, text);
  const tags: Record<string, string> = { ...extraTags };
  if (sig) tags['+freeq.at/sig'] = sig;
  if (Object.keys(tags).length > 0) {
    const line = format('PRIVMSG', [target, text], tags);
    raw(line);
  } else {
    raw(`PRIVMSG ${target} :${text}`);
  }
}

export function sendMessage(target: string, text: string, multiline = false) {
  const isChannel = target.startsWith('#') || target.startsWith('&');

  // Encode multi-line: replace newlines with \n for IRC wire format
  const wireText = multiline ? text.replace(/\n/g, '\\n') : text;
  const extraTags: Record<string, string> = multiline ? { '+freeq.at/multiline': '' } : {};

  // Check if channel has E2EE key — if so, encrypt before sending
  if (e2ee.hasChannelKey(target)) {
    e2ee.encryptChannel(target, wireText).then((encrypted) => {
      if (encrypted) {
        cacheEchoPlaintext(encrypted, text);
        const tags: Record<string, string> = { '+encrypted': '', ...extraTags };
        const line = format('PRIVMSG', [target, encrypted], tags);
        raw(line);
      } else {
        signedPrivmsg(target, wireText, extraTags);
      }
    });
  }
  // DM to an AT-authenticated user: encrypt with Double Ratchet
  else if (!isChannel && e2ee.isE2eeReady()) {
    const remoteDid = didForNick(target);
    if (remoteDid) {
      const origin = window.location.origin;
      e2ee.encryptMessage(remoteDid, wireText, origin).then((encrypted) => {
        if (encrypted) {
          cacheEchoPlaintext(encrypted, text);
          const tags: Record<string, string> = { '+encrypted': '', ...extraTags };
          const line = format('PRIVMSG', [target, encrypted], tags);
          raw(line);
        } else {
          signedPrivmsg(target, wireText, extraTags);
        }
      });
    } else {
      signedPrivmsg(target, wireText, extraTags);
    }
  } else {
    signedPrivmsg(target, wireText, extraTags);
  }

  // Ensure DM buffer exists
  if (!isChannel) {
    const store = useStore.getState();
    if (!store.channels.has(target.toLowerCase())) {
      store.addChannel(target);
    }
  }

  // If we have echo-message, server will echo it back.
  // Otherwise, add it locally (show plaintext, not ciphertext).
  const willEncrypt = e2ee.hasChannelKey(target) || (!isChannel && e2ee.isE2eeReady() && !!didForNick(target));
  if (!ackedCaps.has('echo-message')) {
    useStore.getState().addMessage(target, {
      id: crypto.randomUUID(),
      from: nick,
      text,
      timestamp: new Date(),
      tags: {},
      isSelf: true,
      encrypted: willEncrypt,
    });
  }
}

export function sendReply(target: string, replyToMsgId: string, text: string, multiline = false) {
  const tags: Record<string, string> = { '+reply': replyToMsgId };
  if (multiline) tags['+freeq.at/multiline'] = '';
  const line = format('PRIVMSG', [target, text], tags);
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

export function sendEdit(target: string, originalMsgId: string, newText: string, multiline = false) {
  const tags: Record<string, string> = { '+draft/edit': originalMsgId };
  if (multiline) tags['+freeq.at/multiline'] = '';
  const line = format('PRIVMSG', [target, newText], tags);
  raw(line);
}

export function sendMarkdown(target: string, text: string) {
  const isMultiline = text.includes('\n');
  const wireText = isMultiline ? text.replace(/\n/g, '\\n') : text;
  const tags: Record<string, string> = { '+freeq.at/mime': 'text/markdown' };
  if (isMultiline) tags['+freeq.at/multiline'] = '';
  signedPrivmsg(target, wireText, tags);

  // Local echo if no echo-message cap
  if (!ackedCaps.has('echo-message')) {
    useStore.getState().addMessage(target, {
      id: crypto.randomUUID(),
      from: nick,
      text: wireText,
      timestamp: new Date(),
      tags,
      isSelf: true,
    });
  }
}

export function sendDelete(target: string, msgId: string) {
  useStore.getState().deleteMessage(target, msgId);
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
  // Optimistic removal — don't wait for server confirmation
  useStore.getState().removeChannel(channel);
  joinedChannels.delete(channel.toLowerCase());
  saveJoinedChannels();
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

let pendingAwayReason: string | null = null;
export function setAway(reason?: string) {
  pendingAwayReason = reason || null;
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

export function requestDmTargets(limit = 50) {
  raw(`CHATHISTORY TARGETS * * ${limit}`);
}

export function rawCommand(line: string) {
  raw(line);
}

export function getNick(): string {
  return nick;
}

export function pinMessage(channel: string, msgid: string) {
  raw(`PIN ${channel} ${msgid}`);
}

export function unpinMessage(channel: string, msgid: string) {
  raw(`UNPIN ${channel} ${msgid}`);
}

async function fetchPins(channel: string) {
  try {
    const name = channel.startsWith('#') ? channel.slice(1) : channel;
    const resp = await fetch(`${window.location.origin}/api/v1/channels/${encodeURIComponent(name)}/pins`);
    if (resp.ok) {
      const data = await resp.json();
      useStore.getState().setPins(channel, data.pins || []);
    }
  } catch { /* ignore */ }
}

// ── Internals ──

function raw(line: string) {
  transport?.send(line);
}

async function handleLine(rawLine: string) {
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
      if (saslDid) {
        prefetchProfiles([saslDid]);
        // Initialize E2EE keys for this DID
        const origin = window.location.origin;
        e2ee.initialize(saslDid, origin).catch((e) =>
          console.warn('[e2ee] Init failed:', e)
        );
      }
      break;
    case '903':
      // Register client message-signing key if we have a DID
      if (saslDid) {
        signing.setSigningDid(saslDid);
        signing.generateSigningKey().then((pubkey) => {
          if (pubkey) raw(`MSGSIG ${pubkey}`);
        });
      }
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
      // the web-token was consumed or expired. Retry with a fresh broker session.
      const wasAuthenticated = localStorage.getItem('freeq-handle');
      if (wasAuthenticated && /^Guest\d+$/i.test(serverNick)) {
        guestFallbackCount++;
        if (guestFallbackCount <= 3) {
          // Disconnect and let auto-reconnect retry with fresh broker session
          raw('QUIT :Retrying auth');
          return;
        }
        // After 3 retries, give up — but DON'T delete the broker token.
        // The broker token is a long-lived credential; the issue is likely
        // transient (server restart, DPoP nonce, etc.). Let the user retry
        // from the connect screen which will try broker again.
        guestFallbackCount = 0;
        raw('QUIT :Session expired');
        transport?.disconnect();
        transport = null;
        useStore.getState().fullReset();
        return;
      }
      guestFallbackCount = 0; // Reset on successful auth

      nick = serverNick;
      store.setNick(nick);
      store.setRegistered(true);
      // Auto-join channels:
      // 1. Explicit channels from connect() call (e.g. invite link)
      // 2. Channels from current session (reconnect)
      // 3. Saved channels from localStorage (fresh page load)
      // For DID-authenticated users, the server also auto-joins saved channels,
      // but sending JOIN for already-joined channels is harmless (server ignores).
      let toJoin = autoJoinChannels.length > 0
        ? autoJoinChannels
        : joinedChannels.size > 0
          ? [...joinedChannels]
          : loadSavedChannels();
      // Always include #freeq for new users (no saved channels)
      if (toJoin.length === 0) toJoin = ['#freeq'];
      // Ensure #freeq is always in the list
      if (!toJoin.some(ch => ch.toLowerCase().replace(/^#/, '') === 'freeq' || ch.toLowerCase() === '#freeq')) {
        toJoin.unshift('#freeq');
      }
      // The server auto-joins saved channels for DID users (registration.rs).
      // Only send JOIN for channels not already joined to avoid duplicate 366/CHATHISTORY.
      for (const ch of toJoin) {
        const name = ch.trim();
        if (name && !store.channels.has(name.toLowerCase())) {
          raw(`JOIN ${name}`);
        }
      }
      autoJoinChannels = [];
      // Fetch DM conversation list if authenticated
      if (saslDid) {
        requestDmTargets();
      }
      // Restore last active channel after joins complete
      const savedActive = localStorage.getItem('freeq-active-channel');
      if (savedActive && savedActive !== 'server') {
        setTimeout(() => {
          const ch = useStore.getState().channels.get(savedActive.toLowerCase());
          if (ch) useStore.getState().setActiveChannel(savedActive);
        }, 500);
      }
      break;
    }
    case '433': // Nick in use — append underscore and retry
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
        store.clearMembers(channel); // Clear stale members before NAMES reply arrives
        // Only auto-switch if no saved channel preference or still on server tab
        const savedActive = localStorage.getItem('freeq-active-channel');
        if (!savedActive || store.activeChannel === 'server') {
          store.setActiveChannel(channel);
        }
        joinedChannels.add(channel.toLowerCase());
        saveJoinedChannels();
        // Fetch pinned messages
        fetchPins(channel);
      }
      const joinDid = account && account !== '*' ? account : undefined;
      const actorClass = msg.tags?.['freeq.at/actor-class'] as Member['actorClass'] | undefined;
      store.addMember(channel, {
        nick: from,
        did: joinDid,
        // Don't override op/voice status — JOIN doesn't carry prefix info.
        // NAMES (353) or MODE will set these correctly.
        actorClass,
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
        saveJoinedChannels();
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
        saveJoinedChannels();
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

      // Decrypt E2EE messages
      let displayText = isAction ? text.slice(8, -1) : text;
      let isEncryptedMsg = false;

      // Check echo cache first — our own encrypted messages echoed back
      const cachedPlain = echoPlaintextCache.get(text);
      if (cachedPlain && isSelf) {
        displayText = cachedPlain.plaintext;
        isEncryptedMsg = true;
        echoPlaintextCache.delete(text);
      }
      // ENC1: channel passphrase-based encryption
      else if (e2ee.isENC1(text) && isChannel) {
        const plain = await e2ee.decryptChannel(target, text);
        if (plain !== null) {
          displayText = plain;
          isEncryptedMsg = true;
        } else {
          displayText = '🔒 [encrypted message — use /encrypt <passphrase> to decrypt]';
          isEncryptedMsg = true;
        }
      }
      // ENC3: DM Double Ratchet encryption (from someone else)
      else if (e2ee.isEncrypted(text) && !isChannel && !isSelf) {
        const remoteDid = didForNick(from);
        if (remoteDid) {
          const origin = window.location.origin;
          const plain = await e2ee.decryptMessage(remoteDid, text, origin);
          if (plain !== null) {
            displayText = plain;
            isEncryptedMsg = true;
          } else {
            displayText = '🔒 [encrypted DM — could not decrypt]';
            isEncryptedMsg = true;
          }
        } else {
          displayText = '🔒 [encrypted DM — unknown sender identity]';
          isEncryptedMsg = true;
        }
      }
      // ENC3: own echo that wasn't in cache (e.g. CHATHISTORY)
      else if (e2ee.isEncrypted(text) && !isChannel && isSelf) {
        displayText = '🔒 [encrypted message]';
        isEncryptedMsg = true;
      }
      if (msg.tags['+encrypted']) isEncryptedMsg = true;

      const message: Message = {
        id: msg.tags['msgid'] || crypto.randomUUID(),
        from,
        text: displayText,
        timestamp: msg.tags['time'] ? new Date(msg.tags['time']) : new Date(),
        tags: msg.tags,
        isAction,
        isSelf: isSelf,
        replyTo: msg.tags['+reply'],
        encrypted: isEncryptedMsg,
      };

      // Ensure DM buffer exists
      if (!isChannel && !store.channels.has(bufName.toLowerCase())) {
        store.addChannel(bufName);
      }

      // Background WHOIS for DM partners to learn their DID (enables E2EE)
      if (!isChannel && !isSelf && !didForNick(from) && !backgroundWhois.has(from.toLowerCase())) {
        backgroundWhois.add(from.toLowerCase());
        raw(`WHOIS ${from}`);
      }

      // If this message is part of a batch (CHATHISTORY), add to batch buffer
      const batchId = msg.tags['batch'];
      if (batchId && store.batches.has(batchId)) {
        store.addBatchMessage(batchId, message);
        break;
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
        // With multi-prefix, nicks can have multiple prefixes: @+nick, @%+nick, etc.
        // Strip all leading prefix chars to get bare nick.
        const prefixMatch = n.match(/^([@%+]+)/);
        const prefixes = prefixMatch ? prefixMatch[1] : '';
        const bare = n.slice(prefixes.length);
        const isOp = prefixes.includes('@');
        const isHalfop = prefixes.includes('%');
        const isVoiced = prefixes.includes('+');
        store.addMember(channel, { nick: bare, isOp, isHalfop, isVoiced });
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

    // ── RPL_NOWAWAY (306) / RPL_UNAWAY (305) — self away status ──
    case '306': {
      // "You have been marked as being away"
      const reason = pendingAwayReason || 'away';
      pendingAwayReason = null;
      store.setUserAway(nick, reason);
      store.addSystemMessage('server', `You are now away: ${reason}`);
      break;
    }
    case '305': {
      // "You are no longer marked as being away"
      pendingAwayReason = null;
      store.setUserAway(nick, null);
      store.addSystemMessage('server', 'You are no longer away');
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

    // ── CHATHISTORY (TARGETS response) ──
    case 'CHATHISTORY': {
      const sub = msg.params[0];
      if (sub === 'TARGETS' && msg.params[1]) {
        const targetNick = msg.params[1];
        store.addDmTarget(targetNick);
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
    case '401': {
      const failNick = msg.params[1];
      // DMs are persisted server-side for authenticated users, so the message
      // is stored even when the recipient is offline.  Only show in the DM
      // buffer (not the server tab) to avoid alarm.
      if (failNick && store.channels.has(failNick.toLowerCase())) {
        store.addSystemMessage(failNick, `${failNick} is offline — message saved, they'll see it next time they connect`);
      } else {
        store.addSystemMessage('server', `No such nick: ${failNick}`);
      }
      break;
    }
    case '404': { // ERR_CANNOTSENDTOCHAN
      const ch = msg.params[1] || '';
      const reason = msg.params[2] || 'Cannot send to channel';
      store.addSystemMessage(ch || 'server', reason);
      break;
    }
    case '473': {
      const ch = msg.params[1] || '';
      store.addSystemMessage(ch || 'server', `Cannot join ${ch} — channel is invite only (+i)`);
      break;
    }
    case '474': {
      const ch = msg.params[1] || '';
      store.addSystemMessage(ch || 'server', `Cannot join ${ch} — you are banned`);
      break;
    }
    case '475': {
      const ch = msg.params[1] || '';
      store.addSystemMessage(ch || 'server', `Cannot join ${ch} — incorrect channel key`);
      break;
    }
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
    case '375': case '372': {
      const motdLine = msg.params[msg.params.length - 1];
      store.addSystemMessage('server', motdLine);
      // 375 = MOTD start — clear previous MOTD lines (prevents duplication on reconnect)
      if (msg.command === '375') useStore.setState({ motd: [], motdDismissed: false });
      if (msg.command === '372') store.appendMotd(motdLine.replace(/^- ?/, ''));
      break;
    }

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
