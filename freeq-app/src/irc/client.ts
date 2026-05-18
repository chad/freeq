/**
 * IRC client bridge — thin wrapper that connects @freeq/sdk to the Zustand store.
 *
 * Components import from here and get the same API as before.
 * Internally, all protocol handling is delegated to the SDK's FreeqClient.
 */

import { FreeqClient, format } from '@freeq/sdk';
import { useStore } from '../store';
import { notify } from '../lib/notifications';
import { prefetchProfiles } from '@freeq/sdk';

// ── Singleton SDK client ──

let client: FreeqClient | null = null;

const SAVED_CHANNELS_KEY = 'freeq-joined-channels';

function saveJoinedChannels() {
  try {
    if (client) {
      localStorage.setItem(SAVED_CHANNELS_KEY, JSON.stringify([...client.joinedChannels]));
    }
  } catch { /* quota exceeded, etc */ }
}

/** Get the underlying SDK client (for advanced usage). */
export function getClient(): FreeqClient | null {
  return client;
}

/**
 * Test-only seam. Lets unit tests inject a stub FreeqClient (typically
 * something that only implements `raw`/`nick`) so they can assert on the
 * raw IRC lines our AV functions send without bringing up a real SDK.
 *
 * Never call this from production code.
 */
export function __setClientForTests(c: FreeqClient | null): void {
  client = c;
}

/** Test-only: reset the per-call instance suffix so tests can start fresh. */
export function __resetAvInstanceForTests(): void {
  currentAvInstance = null;
  pendingAvStart = null;
}

// ── Public API (same signatures as before) ──

export function connect(url: string, desiredNick: string, channels?: string[]) {
  if (client) {
    try { client.disconnect(); } catch { /* ignore */ }
    client = null;
  }

  const store = useStore.getState();
  store.reset();

  client = new FreeqClient({
    url,
    nick: desiredNick,
    channels,
    brokerUrl: localStorage.getItem('freeq-broker-base') || undefined,
    brokerToken: localStorage.getItem('freeq-broker-token') || undefined,
    skipInitialBrokerRefresh: !!saslState.skipBrokerRefresh,
  });

  // Set SASL credentials if we have them
  if (saslState.token) {
    client.setSaslCredentials({
      token: saslState.token,
      did: saslState.did,
      pdsUrl: saslState.pdsUrl,
      method: saslState.method,
    });
  }

  // Provide nick→DID resolver for E2EE
  client.nickToDid = (targetNick: string) => {
    const s = useStore.getState();
    const lower = targetNick.toLowerCase();
    for (const ch of s.channels.values()) {
      const m = ch.members.get(lower);
      if (m?.did) return m.did;
    }
    return undefined;
  };

  wireEvents(client);
  client.connect();

  // Send QUIT when tab/window is closing
  window.addEventListener('beforeunload', () => {
    if (client) {
      try { client.quit('Leaving'); } catch { /* ignore */ }
    }
  });
}

export function disconnect() {
  client?.disconnect();
  client = null;
  saslState = { token: '', did: '', pdsUrl: '', method: '', skipBrokerRefresh: false };
  // Clear persistent-login material so ConnectScreen doesn't immediately
  // re-auth the user via broker session refresh after a deliberate logout.
  try {
    localStorage.removeItem('freeq-broker-token');
    localStorage.removeItem('freeq-oauth-result');
    localStorage.removeItem('freeq-oauth-pending');
  } catch { /* ignore */ }
  useStore.getState().fullReset();
}

export function reconnect() {
  if (!client) return;
  const channels = [...(client.joinedChannels)];
  const opts = client['opts']; // access private opts for url/nick
  client.disconnect();
  client = null;
  const store = useStore.getState();
  store.reset();
  connect(opts.url, opts.nick, channels);
}

// SASL state (set before connect)
let saslState = { token: '', did: '', pdsUrl: '', method: '', skipBrokerRefresh: false };

export function setSaslCredentials(token: string, did: string, pdsUrl: string, method: string) {
  saslState = { token, did, pdsUrl, method, skipBrokerRefresh: !!token };
  if (client) {
    client.setSaslCredentials({ token, did, pdsUrl, method });
  }
}

export function sendMessage(target: string, text: string, multiline = false) {
  client?.sendMessage(target, text, multiline);
  // Ensure DM buffer exists
  const isChannel = target.startsWith('#') || target.startsWith('&');
  if (!isChannel) {
    const store = useStore.getState();
    if (!store.channels.has(target.toLowerCase())) {
      store.addChannel(target);
    }
  }
}

export function sendReply(target: string, replyToMsgId: string, text: string, multiline = false) {
  client?.sendReply(target, replyToMsgId, text, multiline);
  const isChannel = target.startsWith('#') || target.startsWith('&');
  if (!isChannel) {
    const store = useStore.getState();
    if (!store.channels.has(target.toLowerCase())) {
      store.addChannel(target);
    }
  }
}

export function sendEdit(target: string, originalMsgId: string, newText: string, multiline = false) {
  client?.sendEdit(target, originalMsgId, newText, multiline);
}

export function sendMarkdown(target: string, text: string) {
  client?.sendMarkdown(target, text);
}

export function sendDelete(target: string, msgId: string) {
  client?.sendDelete(target, msgId);
}

export function sendReaction(target: string, emoji: string, msgId?: string) {
  client?.sendReaction(target, emoji, msgId);
}

export function sendUnreact(target: string, emoji: string, msgId: string) {
  client?.sendUnreact(target, emoji, msgId);
}

export function joinChannel(channel: string) {
  client?.join(channel);
  useStore.getState().addChannel(channel);
  useStore.getState().setActiveChannel(channel);
}

export function partChannel(channel: string) {
  client?.part(channel);
  useStore.getState().removeChannel(channel);
  saveJoinedChannels();
}

export function setTopic(channel: string, topic: string) {
  client?.setTopic(channel, topic);
}

export function setMode(channel: string, mode: string, arg?: string) {
  client?.setMode(channel, mode, arg);
}

export function kickUser(channel: string, userNick: string, reason?: string) {
  client?.kick(channel, userNick, reason);
}

export function inviteUser(channel: string, userNick: string) {
  client?.invite(channel, userNick);
}

export function setAway(reason?: string) {
  client?.setAway(reason);
}

export function sendWhois(userNick: string) {
  client?.whois(userNick);
}

export function requestHistory(channel: string, beforeTimestamp?: string) {
  if (!client) return;
  if (beforeTimestamp) {
    client.requestHistory({ target: channel, mode: 'before', timestamp: beforeTimestamp });
  } else {
    client.requestHistory({ target: channel, mode: 'latest' });
  }
}

export function requestDmTargets(limit = 50) {
  client?.requestHistoryTargets(limit);
}

export function rawCommand(line: string) {
  client?.raw(line);
}

export function getNick(): string {
  return client?.nick ?? '';
}

export function pinMessage(channel: string, msgid: string) {
  client?.pin(channel, msgid);
}

export function unpinMessage(channel: string, msgid: string) {
  client?.unpin(channel, msgid);
}

// ── AV Session ──

let pendingAvStart: { channel: string; did: string } | null = null;

/// Per-call random suffix. Sent on every av-start/av-join/av-leave as the
/// `+freeq.at/av-instance` tag and used to build the MoQ broadcast path
/// (`{sessionId}/{nick}~{instance}`), so two devices signed in as the same
/// DID get distinct participant slots and broadcasts.
let currentAvInstance: string | null = null;

function generateAvInstanceId(): string {
  // 4 random bytes → 8 lowercase hex chars. Plenty of entropy for
  // collision avoidance within a session; short enough that broadcast
  // paths stay readable in logs.
  const buf = new Uint8Array(4);
  crypto.getRandomValues(buf);
  return Array.from(buf, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function getAvInstanceId(): string | null {
  return currentAvInstance;
}

// Per-channel in-flight guard. We only want to suppress *concurrent*
// startAvSession invocations for the same channel (rapid double-clicks
// while the discovery fetch is in flight); the previous guard checked
// store.avAudioActive which would stick true after any teardown blip
// and silently no-op every future button click on that user's session.
const _startInFlight = new Set<string>();

export async function startAvSession(channel: string, title?: string) {
  const store = useStore.getState();
  if (!store.authDid) {
    store.addSystemMessage(channel, 'You must be signed in with AT Protocol to start a voice session.');
    return;
  }
  if (store.connectionState !== 'connected') {
    store.addSystemMessage(channel, 'Cannot start voice session: not connected to server.');
    return;
  }
  const key = channel.toLowerCase();
  if (_startInFlight.has(key)) return;
  _startInFlight.add(key);

  try {
    try {
      const resp = await fetch(`/api/v1/channels/${encodeURIComponent(channel)}/sessions`);
      if (resp.ok) {
        const data = await resp.json();
        if (data.active && data.active.state === 'Active') {
          store.addSystemMessage(channel, `Joining existing voice session (${data.active.participant_count} participants)`);
          joinAvSession(channel, data.active.id);
          store.setAvAudioActive(true);
          return;
        }
      }
    } catch (e) {
      console.warn('[av] Failed to check existing sessions:', e);
    }

    store.addSystemMessage(channel, 'Starting voice session...');
    pendingAvStart = { channel, did: store.authDid };
    currentAvInstance = generateAvInstanceId();
    const tags: Record<string, string> = {
      '+freeq.at/av-start': '',
      '+freeq.at/av-instance': currentAvInstance,
    };
    if (title) tags['+freeq.at/av-title'] = title;
    client?.raw(format('TAGMSG', [channel], tags));
    store.setAvAudioActive(true);
  } finally {
    _startInFlight.delete(key);
  }
}

export function joinAvSession(channel: string, sessionId?: string) {
  // Without a sessionId there's no way for the server to route the join,
  // so don't send a half-formed TAGMSG that just looks like noise. Callers
  // that don't know the session ID yet should call `startAvSession`
  // (which discovers and joins as appropriate).
  if (!sessionId) return;
  if (!currentAvInstance) currentAvInstance = generateAvInstanceId();
  const tags: Record<string, string> = {
    '+freeq.at/av-join': '',
    '+freeq.at/av-instance': currentAvInstance,
    '+freeq.at/av-id': sessionId,
  };
  useStore.getState().setActiveAvSession(sessionId);
  client?.raw(format('TAGMSG', [channel], tags));
}

export function leaveAvSession(channel: string, sessionId: string) {
  const tags: Record<string, string> = {
    '+freeq.at/av-leave': '',
    '+freeq.at/av-id': sessionId,
  };
  if (currentAvInstance) tags['+freeq.at/av-instance'] = currentAvInstance;
  client?.raw(format('TAGMSG', [channel], tags));
  currentAvInstance = null;
  useStore.getState().setActiveAvSession(null);
}

export function endAvSession(channel: string, sessionId: string) {
  const tags: Record<string, string> = {
    '+freeq.at/av-end': '',
    '+freeq.at/av-id': sessionId,
  };
  client?.raw(format('TAGMSG', [channel], tags));
}

export function sendAvSignal(targetNick: string, data: string) {
  const encoded = encodeURIComponent(data);
  const MAX_CHUNK = 4000;
  if (encoded.length <= MAX_CHUNK) {
    client?.raw(format('TAGMSG', [targetNick], { '+freeq.at/av-signal': encoded }));
  } else {
    const id = Math.random().toString(36).slice(2, 8);
    const chunks = Math.ceil(encoded.length / MAX_CHUNK);
    for (let i = 0; i < chunks; i++) {
      const chunk = encoded.slice(i * MAX_CHUNK, (i + 1) * MAX_CHUNK);
      client?.raw(format('TAGMSG', [targetNick], {
        '+freeq.at/av-signal': chunk,
        '+freeq.at/av-chunk': `${id}:${i}:${chunks}`,
      }));
    }
  }
}

// ── Event wiring: SDK events → Zustand store ──

/**
 * Test-only export of the event wiring. Production callers should
 * use {@link connect}, which wires events as part of bringing up the
 * real SDK client. Tests use this to drive synthetic events through
 * a stub `FreeqClient` and assert on the resulting store state.
 */
export function __wireEventsForTests(c: FreeqClient): void {
  wireEvents(c);
}

function wireEvents(c: FreeqClient) {
  const s = () => useStore.getState();

  c.on('connectionStateChanged', (state) => {
    s().setConnectionState(state);
  });

  c.on('registered', (nick) => {
    s().setNick(nick);
    s().setRegistered(true);
    s().setConnectedServer(c['opts'].url);

    // Restore last active channel after joins complete
    const savedActive = localStorage.getItem('freeq-active-channel');
    if (savedActive && savedActive !== 'server') {
      setTimeout(() => {
        const ch = useStore.getState().channels.get(savedActive.toLowerCase());
        if (ch) useStore.getState().setActiveChannel(savedActive);
      }, 500);
    }
  });

  c.on('nickChanged', (nick) => {
    s().setNick(nick);
  });

  c.on('authenticated', (did, message) => {
    s().setAuth(did, message);
    if (did) prefetchProfiles([did]);
  });

  c.on('authError', (error) => {
    s().setAuthError(error);
  });

  c.on('channelJoined', (channel) => {
    s().addChannel(channel);
    s().clearMembers(channel);
    const savedActive = localStorage.getItem('freeq-active-channel');
    if (!savedActive || s().activeChannel === 'server') {
      s().setActiveChannel(channel);
    }
    saveJoinedChannels();
  });

  c.on('channelLeft', (channel) => {
    s().removeChannel(channel);
    saveJoinedChannels();
  });

  c.on('memberJoined', (channel, member) => {
    if (channel) s().addMember(channel, member);
    if (member.did) prefetchProfiles([member.did]);
  });

  c.on('memberLeft', (channel, nick) => {
    s().removeMember(channel, nick);
  });

  c.on('userQuit', (nick, reason) => {
    s().removeUserFromAll(nick, reason);
  });

  c.on('userRenamed', (oldNick, newNick) => {
    s().renameUser(oldNick, newNick);
  });

  c.on('userAway', (nick, reason) => {
    s().setUserAway(nick, reason);
  });

  c.on('typing', (channel, nick, isTyping) => {
    s().setTyping(channel, nick, isTyping);
  });

  c.on('topicChanged', (channel, topic, setBy) => {
    s().setTopic(channel, topic, setBy);
  });

  c.on('modeChanged', (channel, mode, arg, setBy) => {
    s().handleMode(channel, mode, arg, setBy);
  });

  c.on('membersList', (channel, members) => {
    for (const m of members) {
      s().addMember(channel, m);
    }
  });

  c.on('memberDid', (nick, did) => {
    s().updateMemberDid(nick, did);
  });

  c.on('message', (channel, message) => {
    // Prefetch avatar by DID if available (from account-tag)
    if (message.tags?.account) prefetchProfiles([message.tags.account]);

    // Ensure DM buffer exists
    const isChannel = channel.startsWith('#') || channel.startsWith('&');
    if (!isChannel && !useStore.getState().channels.has(channel.toLowerCase())) {
      s().addChannel(channel);
    }
    s().addMessage(channel, message as import('../store').Message);

    // Mention/DM notification
    const isMention = !message.isSelf && message.text.toLowerCase().includes(c.nick.toLowerCase());
    const isDM = !isChannel && !message.isSelf;
    if (isMention) s().incrementMentions(channel);
    if (isDM) s().incrementMentions(channel);
    if ((isMention || isDM) && !useStore.getState().mutedChannels.has(channel.toLowerCase())) {
      notify(
        isDM ? `DM from ${message.from}` : channel,
        `${message.from}: ${message.text.slice(0, 100)}`,
        () => useStore.getState().setActiveChannel(channel),
      );
    }
  });

  c.on('messageEdited', (channel, originalMsgId, newText, newMsgId, isStreaming) => {
    // Ensure DM buffer exists
    const isChannel = channel.startsWith('#') || channel.startsWith('&');
    if (!isChannel && !useStore.getState().channels.has(channel.toLowerCase())) {
      s().addChannel(channel);
    }
    s().editMessage(channel, originalMsgId, newText, newMsgId, isStreaming);
  });

  c.on('messageDeleted', (channel, msgId) => {
    s().deleteMessage(channel, msgId);
  });

  c.on('reactionAdded', (channel, msgId, emoji, fromNick) => {
    s().addReaction(channel, msgId, emoji, fromNick);
  });

  c.on('reactionRemoved', (channel, msgId, emoji, fromNick) => {
    s().removeReaction(channel, msgId, emoji, fromNick);
  });

  c.on('systemMessage', (target, text) => {
    // Skip internal mention markers
    if (target === '__mention__') return;
    s().addSystemMessage(target, text);
  });

  c.on('historyBatch', (channel, messages) => {
    // Prefetch avatars by DID for history messages
    const dids = messages.map((m: any) => m.tags?.account).filter(Boolean);
    if (dids.length) prefetchProfiles(dids);

    useStore.getState().mergeHistory(
      channel,
      messages as import('../store').Message[],
    );
  });

  c.on('dmTarget', (nick) => {
    s().addDmTarget(nick);
  });

  c.on('channelListEntry', (entry) => {
    s().addChannelListEntry(entry);
  });

  c.on('pins', (channel, pins) => {
    s().setPins(channel, pins);
  });

  c.on('pinAdded', (channel, msgid, pinnedBy) => {
    s().addPin(channel, msgid, pinnedBy);
  });

  c.on('pinRemoved', (channel, msgid) => {
    s().removePin(channel, msgid);
  });

  c.on('whois', (nick, info) => {
    s().updateWhois(nick, info);
  });

  c.on('motdStart', () => {
    useStore.setState({ motd: [], motdDismissed: false });
  });

  c.on('motd', (line) => {
    s().appendMotd(line);
  });

  c.on('avSessionUpdate', (session) => {
    const sess = session as import('../store').AvSession;
    s().updateAvSession(sess);
    if (
      pendingAvStart &&
      sess.channel === pendingAvStart.channel &&
      sess.createdBy === pendingAvStart.did &&
      !s().activeAvSession
    ) {
      s().setActiveAvSession(sess.id);
      pendingAvStart = null;
    }
    // When the session we're currently in ends (anyone ran `/av end`, or
    // the host disconnected and the server reaped it), tear the panel
    // down on our side too. Without this the leaver hears nothing but
    // the remaining device still shows an "active call" UI.
    if (sess.state === 'ended' && s().activeAvSession === sess.id) {
      s().setAvAudioActive(false);
      s().setAvCameraOn(false);
      s().setActiveAvSession(null);
      currentAvInstance = null;
    }
  });

  c.on('avSessionRemoved', (id) => {
    // If the SDK reaped a session we were still in (e.g. ended before we
    // saw the `state: 'ended'` update), close the panel as a safety net.
    if (s().activeAvSession === id) {
      s().setAvAudioActive(false);
      s().setAvCameraOn(false);
      s().setActiveAvSession(null);
      currentAvInstance = null;
    }
    s().removeAvSession(id);
  });

  c.on('avTicket', (sessionId, ticket) => {
    const session = s().avSessions.get(sessionId);
    if (session) {
      s().updateAvSession({ ...session, irohTicket: ticket } as import('../store').AvSession);
    }
  });

  c.on('joinGateRequired', (channel) => {
    if (useStore.getState().authDid) {
      s().setJoinGateChannel(channel);
    }
  });

  c.on('userKicked', (channel, kicked, _by, _reason) => {
    s().removeMember(channel, kicked);
  });

  c.on('error', (message) => {
    if (message.includes('same identity reconnected')) {
      useStore.getState().fullReset();
    }
  });

  // Handle '001' guest fallback detection — broker token refresh
  // This is handled within the SDK now; the app just needs to handle the
  // registered event, which we do above.
}
