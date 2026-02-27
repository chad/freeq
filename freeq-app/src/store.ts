import { create } from 'zustand';
import type { TransportState } from './irc/transport';
import { setLastReadMsgId } from './lib/db';

// ── Types ──

export interface Message {
  id: string;
  from: string;
  text: string;
  timestamp: Date;
  tags: Record<string, string>;
  isAction?: boolean;
  isSelf?: boolean;
  isSystem?: boolean;
  replyTo?: string;
  editOf?: string;
  deleted?: boolean;
  reactions?: Map<string, Set<string>>; // emoji → nicks
  encrypted?: boolean; // true if this message was E2EE encrypted
}

export interface Member {
  nick: string;
  did?: string;
  handle?: string;
  displayName?: string;
  avatarUrl?: string;
  isOp: boolean;
  isHalfop: boolean;
  isVoiced: boolean;
  away?: string | null;
  typing?: boolean;
}

export interface PinnedMessage {
  msgid: string;
  pinned_by: string;
  pinned_at: number;
}

export interface Channel {
  name: string;
  topic: string;
  topicSetBy?: string;
  members: Map<string, Member>;
  messages: Message[];
  modes: Set<string>;
  isEncrypted: boolean; // true if +E mode or all DMs with this user are encrypted
  unreadCount: number;
  mentionCount: number;
  lastReadMsgId?: string; // last message seen when channel was active
  isJoined: boolean;
  pins: PinnedMessage[];
}

interface Batch {
  type: string;
  target: string;
  messages: Message[];
}

export interface WhoisInfo {
  nick: string;
  user?: string;
  host?: string;
  realname?: string;
  server?: string;
  did?: string;
  handle?: string;
  channels?: string;
  fetchedAt: number;
}

export interface ReplyContext {
  msgId: string;
  from: string;
  text: string;
  channel: string;
}

export interface EditContext {
  msgId: string;
  text: string;
  channel: string;
}

export interface ChannelListEntry {
  name: string;
  topic: string;
  count: number;
}

export interface Store {
  // Connection
  connectionState: TransportState;
  nick: string;
  registered: boolean;
  authDid: string | null;
  authMessage: string | null;
  authError: string | null;
  motd: string[];
  motdDismissed: boolean;

  // Channels & DMs
  channels: Map<string, Channel>;
  activeChannel: string;
  serverMessages: Message[];

  // Active batches
  batches: Map<string, Batch>;

  // WHOIS cache
  whoisCache: Map<string, WhoisInfo>;

  // UI state
  replyTo: ReplyContext | null;
  editingMsg: EditContext | null;
  theme: 'dark' | 'light';
  messageDensity: 'default' | 'compact' | 'cozy';
  favorites: Set<string>; // lowercase channel names
  mutedChannels: Set<string>; // lowercase channel names
  bookmarks: { channel: string; msgId: string; from: string; text: string; timestamp: Date }[];
  bookmarksPanelOpen: boolean;
  searchOpen: boolean;
  scrollToMsgId: string | null;
  searchQuery: string;
  channelListOpen: boolean;
  channelList: ChannelListEntry[];
  lightboxUrl: string | null;
  threadMsgId: string | null;
  threadChannel: string | null;

  // Actions — connection
  setConnectionState: (state: TransportState) => void;
  setNick: (nick: string) => void;
  setRegistered: (v: boolean) => void;
  setAuth: (did: string, message: string) => void;
  setAuthError: (error: string) => void;
  appendMotd: (line: string) => void;
  dismissMotd: () => void;
  reset: () => void;
  fullReset: () => void;

  // Actions — channels
  addChannel: (name: string) => void;
  removeChannel: (name: string) => void;
  setActiveChannel: (name: string) => void;
  setTopic: (channel: string, topic: string, setBy?: string) => void;

  // Actions — members
  clearMembers: (channel: string) => void;
  addMember: (channel: string, member: Partial<Member> & { nick: string }) => void;
  removeMember: (channel: string, nick: string) => void;
  removeUserFromAll: (nick: string, reason: string) => void;
  renameUser: (oldNick: string, newNick: string) => void;
  setUserAway: (nick: string, reason: string | null) => void;
  setTyping: (channel: string, nick: string, typing: boolean) => void;
  updateMemberDid: (nick: string, did: string) => void;
  handleMode: (channel: string, mode: string, arg: string | undefined, setBy: string) => void;

  // Actions — messages
  addMessage: (channel: string, msg: Message) => void;
  addSystemMessage: (channel: string, text: string) => void;
  editMessage: (channel: string, originalMsgId: string, newText: string, newMsgId?: string) => void;
  deleteMessage: (channel: string, msgId: string) => void;
  addReaction: (channel: string, msgId: string, emoji: string, fromNick: string) => void;
  incrementMentions: (channel: string) => void;
  clearUnread: (channel: string) => void;

  // Actions — batches
  startBatch: (id: string, type: string, target: string) => void;
  addBatchMessage: (id: string, msg: Message) => void;
  endBatch: (id: string) => void;

  // Actions — whois
  updateWhois: (nick: string, info: Partial<WhoisInfo>) => void;

  // Actions — UI
  setReplyTo: (ctx: ReplyContext | null) => void;
  setEditingMsg: (ctx: EditContext | null) => void;
  setTheme: (theme: 'dark' | 'light') => void;
  setMessageDensity: (d: 'default' | 'compact' | 'cozy') => void;
  toggleFavorite: (channel: string) => void;
  toggleMuted: (channel: string) => void;
  isFavorite: (channel: string) => boolean;
  isMuted: (channel: string) => boolean;
  addBookmark: (channel: string, msgId: string, from: string, text: string, timestamp: Date) => void;
  removeBookmark: (msgId: string) => void;
  setBookmarksPanelOpen: (open: boolean) => void;
  setSearchOpen: (open: boolean) => void;
  setScrollToMsgId: (id: string | null) => void;
  setPins: (channel: string, pins: PinnedMessage[]) => void;
  setSearchQuery: (query: string) => void;
  setChannelListOpen: (open: boolean) => void;
  setChannelList: (list: ChannelListEntry[]) => void;
  addChannelListEntry: (entry: ChannelListEntry) => void;
  setLightboxUrl: (url: string | null) => void;
  openThread: (msgId: string, channel: string) => void;
  closeThread: () => void;

  // Join gate
  joinGateChannel: string | null;
  setJoinGateChannel: (channel: string | null) => void;

  // Channel settings
  channelSettingsOpen: string | null;
  setChannelSettingsOpen: (channel: string | null) => void;
}

function getOrCreateChannel(channels: Map<string, Channel>, name: string): Channel {
  const key = name.toLowerCase();
  let ch = channels.get(key);
  if (!ch) {
    ch = {
      name,
      topic: '',
      members: new Map(),
      messages: [],
      modes: new Set(),
      isEncrypted: false,
      unreadCount: 0,
      mentionCount: 0,
      isJoined: false,
      pins: [],
    };
    channels.set(key, ch);
  }
  return ch;
}

export const useStore = create<Store>((set, get) => ({
  // Initial state
  connectionState: 'disconnected',
  nick: '',
  registered: false,
  authDid: null,
  authMessage: null,
  authError: null,
  motd: [],
  motdDismissed: false,
  channels: new Map(),
  activeChannel: 'server',
  serverMessages: [],
  batches: new Map(),
  whoisCache: new Map(),
  replyTo: null,
  editingMsg: null,
  theme: (localStorage.getItem('freeq-theme') as 'dark' | 'light') || 'dark',
  messageDensity: (localStorage.getItem('freeq-density') as 'default' | 'compact' | 'cozy') || 'default',
  favorites: new Set(JSON.parse(localStorage.getItem('freeq-favorites') || '[]')),
  mutedChannels: new Set(JSON.parse(localStorage.getItem('freeq-muted') || '[]')),
  bookmarks: JSON.parse(localStorage.getItem('freeq-bookmarks') || '[]').map((b: any) => ({ ...b, timestamp: new Date(b.timestamp) })),
  bookmarksPanelOpen: false,
  searchOpen: false,
  scrollToMsgId: null,
  searchQuery: '',
  channelListOpen: false,
  channelList: [],
  lightboxUrl: null,
  threadMsgId: null,
  threadChannel: null,
  joinGateChannel: null,
  channelSettingsOpen: null,

  // Connection
  setConnectionState: (state) => set({ connectionState: state }),
  setNick: (nick) => set({ nick }),
  setRegistered: (v) => set({ registered: v }),
  setAuth: (did, message) => set({ authDid: did, authMessage: message, authError: null }),
  appendMotd: (line) => set((s) => ({ motd: [...s.motd, line] })),
  dismissMotd: () => set({ motdDismissed: true }),
  setAuthError: (error) => set({ authError: error }),
  reset: () => set({
    connectionState: 'disconnected',
    registered: false,
    channels: new Map(),
    activeChannel: 'server',
    serverMessages: [],
    batches: new Map(),
    motd: [],
    motdDismissed: false,
  }),
  fullReset: () => set((s) => ({
    connectionState: 'disconnected',
    nick: '',
    registered: false,
    authDid: null,
    authMessage: null,
    authError: null,
    channels: new Map(),
    activeChannel: 'server',
    serverMessages: [],
    batches: new Map(),
    whoisCache: new Map(),
    replyTo: null,
    editingMsg: null,
    searchOpen: false,
    searchQuery: '',
    channelListOpen: false,
    channelList: [],
    lightboxUrl: null,
    threadMsgId: null,
    threadChannel: null,
    joinGateChannel: null,
    channelSettingsOpen: null,
    theme: s.theme, messageDensity: s.messageDensity, favorites: s.favorites, mutedChannels: s.mutedChannels, bookmarks: s.bookmarks, bookmarksPanelOpen: false, // preserve across reconnects
  })),

  // Channels
  addChannel: (name) => set((s) => {
    const channels = new Map(s.channels);
    const ch = getOrCreateChannel(channels, name);
    ch.isJoined = true;
    channels.set(name.toLowerCase(), ch);
    return { channels };
  }),

  removeChannel: (name) => set((s) => {
    const channels = new Map(s.channels);
    channels.delete(name.toLowerCase());
    const activeChannel = s.activeChannel.toLowerCase() === name.toLowerCase() ? 'server' : s.activeChannel;
    return { channels, activeChannel };
  }),

  setActiveChannel: (name) => set((s) => {
    const channels = new Map(s.channels);
    // Mark last-read on the channel we're leaving
    const oldCh = channels.get(s.activeChannel.toLowerCase());
    if (oldCh && oldCh.messages.length > 0) {
      const lastMsg = oldCh.messages[oldCh.messages.length - 1];
      oldCh.lastReadMsgId = lastMsg.id;
      channels.set(s.activeChannel.toLowerCase(), oldCh);
    }
    // Clear unread on the channel we're entering
    const ch = channels.get(name.toLowerCase());
    if (ch) {
      ch.unreadCount = 0;
      ch.mentionCount = 0;
      channels.set(name.toLowerCase(), { ...ch });
    }
    return { activeChannel: name, channels };
  }),

  setTopic: (channel, topic, setBy) => set((s) => {
    const channels = new Map(s.channels);
    const ch = getOrCreateChannel(channels, channel);
    ch.topic = topic;
    if (setBy) ch.topicSetBy = setBy;
    channels.set(channel.toLowerCase(), ch);
    return { channels };
  }),

  // Members
  clearMembers: (channel) => set((s) => {
    const key = channel.toLowerCase();
    const ch = s.channels.get(key);
    if (ch) ch.members = new Map();
    return {};
  }),
  addMember: (channel, member) => set((s) => {
    const channels = new Map(s.channels);
    const ch = getOrCreateChannel(channels, channel);
    const existing = ch.members.get(member.nick.toLowerCase());
    ch.members.set(member.nick.toLowerCase(), {
      nick: member.nick,
      did: member.did ?? existing?.did,
      handle: member.handle ?? existing?.handle,
      displayName: member.displayName ?? existing?.displayName,
      avatarUrl: member.avatarUrl ?? existing?.avatarUrl,
      isOp: member.isOp ?? existing?.isOp ?? false,
      isHalfop: member.isHalfop ?? existing?.isHalfop ?? false,
      isVoiced: member.isVoiced ?? existing?.isVoiced ?? false,
      away: existing?.away,
    });
    channels.set(channel.toLowerCase(), ch);
    return { channels };
  }),

  removeMember: (channel, nick) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (ch) {
      ch.members.delete(nick.toLowerCase());
      channels.set(channel.toLowerCase(), ch);
    }
    return { channels };
  }),

  removeUserFromAll: (nick, reason) => set((s) => {
    const channels = new Map(s.channels);
    for (const [key, ch] of channels) {
      if (ch.members.has(nick.toLowerCase())) {
        ch.members.delete(nick.toLowerCase());
        ch.messages = [...ch.messages, {
          id: crypto.randomUUID(),
          from: '',
          text: `${nick} quit${reason ? ` (${reason})` : ''}`,
          timestamp: new Date(),
          tags: {},
          isSystem: true,
        }];
        channels.set(key, { ...ch });
      }
    }
    return { channels };
  }),

  renameUser: (oldNick, newNick) => set((s) => {
    const channels = new Map(s.channels);
    for (const [key, ch] of channels) {
      const member = ch.members.get(oldNick.toLowerCase());
      if (member) {
        ch.members.delete(oldNick.toLowerCase());
        ch.members.set(newNick.toLowerCase(), { ...member, nick: newNick });
        channels.set(key, ch);
      }
    }
    return { channels };
  }),

  setUserAway: (nick, reason) => set((s) => {
    const channels = new Map(s.channels);
    for (const [key, ch] of channels) {
      const member = ch.members.get(nick.toLowerCase());
      if (member) {
        ch.members.set(nick.toLowerCase(), { ...member, away: reason });
        channels.set(key, { ...ch });
      }
    }
    return { channels };
  }),

  setTyping: (channel, nick, typing) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (ch) {
      const member = ch.members.get(nick.toLowerCase());
      if (member) {
        ch.members.set(nick.toLowerCase(), { ...member, typing });
        channels.set(channel.toLowerCase(), { ...ch });
      }
    }
    return { channels };
  }),

  updateMemberDid: (nick, did) => set((s) => {
    const channels = new Map(s.channels);
    for (const [key, ch] of channels) {
      const member = ch.members.get(nick.toLowerCase());
      if (member) {
        ch.members.set(nick.toLowerCase(), { ...member, did });
        channels.set(key, { ...ch });
      }
    }
    return { channels };
  }),

  handleMode: (channel, mode, arg, _setBy) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (!ch) return { channels };

    const adding = mode.startsWith('+');
    const modeChar = mode.replace(/^[+-]/, '');

    // User modes (+o, +h, +v)
    if ((modeChar === 'o' || modeChar === 'h' || modeChar === 'v') && arg) {
      const member = ch.members.get(arg.toLowerCase());
      if (member) {
        if (modeChar === 'o') member.isOp = adding;
        if (modeChar === 'h') member.isHalfop = adding;
        if (modeChar === 'v') member.isVoiced = adding;
        ch.members.set(arg.toLowerCase(), { ...member });
      }
    } else {
      // Channel modes
      if (adding) ch.modes.add(modeChar);
      else ch.modes.delete(modeChar);
      // Track encryption mode
      if (modeChar === 'E') ch.isEncrypted = adding;
    }
    channels.set(channel.toLowerCase(), { ...ch });
    return { channels };
  }),

  // Messages
  addMessage: (channel, msg) => set((s) => {
    if (channel === 'server' || channel.toLowerCase() === 'server') {
      return { serverMessages: [...s.serverMessages, msg].slice(-500) };
    }

    const channels = new Map(s.channels);
    const ch = getOrCreateChannel(channels, channel);

    // Dedup by msgid — CHATHISTORY can return messages already shown live
    if (msg.id && !msg.isSystem && ch.messages.some((m) => m.id === msg.id)) {
      return {};
    }

    ch.messages = [...ch.messages, msg].slice(-1000);
    if (s.activeChannel.toLowerCase() !== channel.toLowerCase()) {
      ch.unreadCount++;
    }
    channels.set(channel.toLowerCase(), ch);
    return { channels };
  }),

  addSystemMessage: (channel, text) => {
    const msg: Message = {
      id: crypto.randomUUID(),
      from: '',
      text,
      timestamp: new Date(),
      tags: {},
      isSystem: true,
    };
    get().addMessage(channel, msg);
  },

  editMessage: (channel, originalMsgId, newText, newMsgId) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (ch) {
      ch.messages = ch.messages.map((m) =>
        m.id === originalMsgId
          ? { ...m, text: newText, id: newMsgId || m.id, editOf: originalMsgId }
          : m
      );
      channels.set(channel.toLowerCase(), { ...ch });
    }

    // Also update in-flight batch messages (CHATHISTORY) for this channel
    const batches = new Map(s.batches);
    for (const [id, batch] of batches) {
      if (batch.target.toLowerCase() !== channel.toLowerCase()) continue;
      batch.messages = batch.messages.map((m) =>
        m.id === originalMsgId
          ? { ...m, text: newText, id: newMsgId || m.id, editOf: originalMsgId }
          : m
      );
      batches.set(id, batch);
    }

    return { channels, batches };
  }),

  deleteMessage: (channel, msgId) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (!ch) return { channels };
    ch.messages = ch.messages.map((m) =>
      m.id === msgId ? { ...m, deleted: true, text: '' } : m
    );
    channels.set(channel.toLowerCase(), { ...ch });
    return { channels };
  }),

  addReaction: (channel, msgId, emoji, fromNick) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (!ch) return { channels };
    ch.messages = ch.messages.map((m) => {
      if (m.id !== msgId) return m;
      const reactions = new Map(m.reactions || []);
      const nicks = new Set(reactions.get(emoji) || []);
      nicks.add(fromNick);
      reactions.set(emoji, nicks);
      return { ...m, reactions };
    });
    channels.set(channel.toLowerCase(), { ...ch });
    return { channels };
  }),

  incrementMentions: (channel) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (ch && s.activeChannel.toLowerCase() !== channel.toLowerCase()) {
      ch.mentionCount++;
      channels.set(channel.toLowerCase(), { ...ch });
    }
    return { channels };
  }),

  clearUnread: (channel) => set((s) => {
    const channels = new Map(s.channels);
    const ch = channels.get(channel.toLowerCase());
    if (ch) {
      ch.unreadCount = 0;
      ch.mentionCount = 0;
      // Persist last-read message ID
      const lastMsg = ch.messages[ch.messages.length - 1];
      if (lastMsg?.id) {
        setLastReadMsgId(channel, lastMsg.id).catch(() => {});
      }
      channels.set(channel.toLowerCase(), { ...ch });
    }
    return { channels };
  }),

  // Batches
  startBatch: (id, type, target) => set((s) => {
    const batches = new Map(s.batches);
    batches.set(id, { type, target, messages: [] });
    return { batches };
  }),

  addBatchMessage: (id, msg) => set((s) => {
    const batches = new Map(s.batches);
    const batch = batches.get(id);
    if (!batch) return { batches };
    batch.messages = [...batch.messages, msg];
    batches.set(id, batch);
    return { batches };
  }),

  endBatch: (id) => set((s) => {
    const batches = new Map(s.batches);
    const batch = batches.get(id);
    batches.delete(id);
    if (!batch) return { batches };

    // Flush batch messages to the channel
    const channels = new Map(s.channels);
    const ch = getOrCreateChannel(channels, batch.target);

    // Dedup by msgid when merging history
    const existingIds = new Set(ch.messages.map((m) => m.id));
    const newMsgs = batch.messages.filter((m) => !m.id || !existingIds.has(m.id));

    // Sort batch messages by timestamp (oldest first)
    newMsgs.sort((a, b) => {
      const ta = a.timestamp?.getTime?.() ?? 0;
      const tb = b.timestamp?.getTime?.() ?? 0;
      if (ta !== tb) return ta - tb;
      return (a.id || '').localeCompare(b.id || '');
    });

    // Batch messages go at the beginning (history)
    ch.messages = [...newMsgs, ...ch.messages].slice(-1000);
    channels.set(batch.target.toLowerCase(), ch);
    return { channels, batches };
  }),

  // Whois
  updateWhois: (nick, info) => set((s) => {
    const whoisCache = new Map(s.whoisCache);
    const key = nick.toLowerCase();
    const existing = whoisCache.get(key) || { nick, fetchedAt: Date.now() };
    whoisCache.set(key, { ...existing, ...info, nick, fetchedAt: Date.now() });
    return { whoisCache };
  }),

  // UI actions
  setReplyTo: (ctx) => set({ replyTo: ctx }),
  setEditingMsg: (ctx) => set({ editingMsg: ctx }),
  setTheme: (theme) => {
    localStorage.setItem('freeq-theme', theme);
    set({ theme });
  },
  setMessageDensity: (d) => {
    localStorage.setItem('freeq-density', d);
    set({ messageDensity: d });
  },
  toggleFavorite: (channel) => set((s) => {
    const favs = new Set(s.favorites);
    const key = channel.toLowerCase();
    if (favs.has(key)) favs.delete(key); else favs.add(key);
    localStorage.setItem('freeq-favorites', JSON.stringify([...favs]));
    return { favorites: favs };
  }),
  toggleMuted: (channel) => set((s) => {
    const muted = new Set(s.mutedChannels);
    const key = channel.toLowerCase();
    if (muted.has(key)) muted.delete(key); else muted.add(key);
    localStorage.setItem('freeq-muted', JSON.stringify([...muted]));
    return { mutedChannels: muted };
  }),
  isFavorite: (channel) => get().favorites.has(channel.toLowerCase()),
  isMuted: (channel) => get().mutedChannels.has(channel.toLowerCase()),
  addBookmark: (channel, msgId, from, text, timestamp) => set((s) => {
    if (s.bookmarks.some((b) => b.msgId === msgId)) return s;
    const bookmarks = [...s.bookmarks, { channel, msgId, from, text, timestamp }];
    localStorage.setItem('freeq-bookmarks', JSON.stringify(bookmarks));
    return { bookmarks };
  }),
  removeBookmark: (msgId) => set((s) => {
    const bookmarks = s.bookmarks.filter((b) => b.msgId !== msgId);
    localStorage.setItem('freeq-bookmarks', JSON.stringify(bookmarks));
    return { bookmarks };
  }),
  setBookmarksPanelOpen: (open) => set({ bookmarksPanelOpen: open }),
  setSearchOpen: (open) => set({ searchOpen: open, searchQuery: open ? '' : '' }),
  setScrollToMsgId: (id) => set({ scrollToMsgId: id }),
  setPins: (channel, pins) => set((state) => {
    const channels = new Map(state.channels);
    const ch = channels.get(channel.toLowerCase());
    if (ch) { ch.pins = pins; channels.set(channel.toLowerCase(), { ...ch }); }
    return { channels };
  }),
  setSearchQuery: (query) => set({ searchQuery: query }),
  setChannelListOpen: (open) => set({ channelListOpen: open }),
  setChannelList: (list) => set({ channelList: list }),
  addChannelListEntry: (entry) => set((s) => ({
    channelList: [...s.channelList, entry],
  })),
  setLightboxUrl: (url) => set({ lightboxUrl: url }),
  openThread: (msgId, channel) => set({ threadMsgId: msgId, threadChannel: channel }),
  closeThread: () => set({ threadMsgId: null, threadChannel: null }),
  setJoinGateChannel: (channel) => set({ joinGateChannel: channel }),
  setChannelSettingsOpen: (channel) => set({ channelSettingsOpen: channel }),
}));
