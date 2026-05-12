/**
 * FreeqClient — event-driven IRC client with AT Protocol identity and E2EE.
 *
 * Usage:
 *   const client = new FreeqClient({ url: 'wss://irc.freeq.at/irc', nick: 'mybot' });
 *   client.on('message', (channel, msg) => console.log(`${msg.from}: ${msg.text}`));
 *   client.connect();
 */

import { EventEmitter } from './events.js';
import { parse, prefixNick, format } from './parser.js';
import { Transport } from './transport.js';
import * as signing from './signing.js';
import * as e2ee from './e2ee.js';
import { prefetchProfiles } from './profiles.js';
import type {
  IRCMessage, Message, Member, AvSession, AvParticipant,
  FreeqClientOptions, SaslCredentials, Batch, TransportState,
  PinnedMessage, WhoisInfo, HistoryOptions, EmitEventOptions,
  HeartbeatHandle, PresencePayload, GovernancePayload, GovernanceSignal,
  CoordinationEventPayload, SpendPayload, BudgetSnapshot,
  AgentSpawnedPayload, AgentDespawnedPayload,
} from './types.js';

export class FreeqClient extends EventEmitter {
  private transport: Transport | null = null;
  private _nick = '';
  private _authDid: string | null = null;
  /** Bearer token usable for `/agent/tools/*` HTTP calls. Populated
   *  from the server-emitted `NOTICE * :API-BEARER <session_id>` that
   *  fires immediately after SASL success. Bots use this to call
   *  diagnostic tools as themselves instead of as anonymous. */
  private _apiBearer: string | null = null;
  private _connectionState: TransportState = 'disconnected';
  private _registered = false;
  private opts: FreeqClientOptions;

  private ackedCaps = new Set<string>();
  private sasl: SaslCredentials | null = null;
  private skipBrokerRefresh: boolean;
  private guestFallbackCount = 0;
  /** Set when SASL was attempted and 904 was received. Suppresses any
   *  subsequent registration completion as a guest, and blocks outgoing
   *  PRIVMSGs that would silently leak under the guest identity. */
  private _saslFailed = false;
  /** Channels the server has flagged +E. Used to block plaintext sends
   *  when we don't (yet) have the passphrase, so messages don't leak
   *  unencrypted into a channel the rest of the room expects encrypted. */
  private _encryptedChannels = new Set<string>();
  /** Current AWAY reason, or null if not away. Re-asserted on
   *  reconnect so the wire and UI states don't diverge after the
   *  server forgets us during the disconnect. */
  private _currentAway: string | null = null;

  private autoJoinChannels: string[] = [];
  private _joinedChannels = new Set<string>();

  private backgroundWhois = new Set<string>();
  private echoPlaintextCache = new Map<string, { plaintext: string; ts: number }>();
  private batches = new Map<string, Batch>();
  private pendingAwayReason: string | null = null;

  private _avSessions = new Map<string, AvSession>();
  private _activeAvSession: string | null = null;

  // ── Internal caches and timer state ───────────────────────────────
  /** Lowercase nick → DID. Populated from numeric 330 (WHOIS) and from
   *  inbound `+freeq.at/account` tags. */
  private _nickToDid = new Map<string, string>();
  /** DID → lowercase nick. Reverse cache for AGENT PAUSE/REVOKE which
   *  take nicks, not DIDs. */
  private _didToNick = new Map<string, string>();
  /** Accumulating WHOIS info per nick. Multiple WHOIS numerics fire
   *  incrementally (311/312/319/330/671/673); we collect until 318
   *  (RPL_ENDOFWHOIS) and resolve the requestWhois() Promise. */
  private _whoisBuffer = new Map<string, Partial<WhoisInfo>>();
  /** Pending requestWhois() Promise resolvers, keyed by lowercase nick. */
  private _pendingWhois = new Map<string, Array<{
    resolve: (info: WhoisInfo) => void;
    reject: (err: Error) => void;
    timer: ReturnType<typeof setTimeout>;
  }>>();
  /** Random-suffix nick collision retry counter. */
  private _nickCollisionRetries = 0;
  /** Background heartbeat loop handle (set by startHeartbeat()). */
  private _agentHeartbeatTimer: ReturnType<typeof setInterval> | null = null;
  /** Recently-seen coordination event IDs (TAGMSG + companion PRIVMSG carry
   *  the same eventId; we fire `coordinationEvent` only once per pair). */
  private _seenCoordinationEvents = new Map<string, number>();

  constructor(opts: FreeqClientOptions) {
    super();
    this.opts = opts;
    this._nick = opts.nick;
    this.sasl = opts.sasl ?? null;
    this.autoJoinChannels = opts.channels ? [...opts.channels] : [];
    this.skipBrokerRefresh = opts.skipInitialBrokerRefresh ?? false;
  }

  // ── Accessors ──

  /** Current IRC nickname. */
  get nick(): string { return this._nick; }

  /** Authenticated AT Protocol DID, or null if guest. */
  get authDid(): string | null { return this._authDid; }

  /** Bearer token for `/agent/tools/*` HTTP calls. Set automatically
   *  on SASL success; null while unauthenticated. Use as
   *  `Authorization: Bearer <client.apiBearer>` to make diagnostic
   *  calls as the same identity the IRC session is bound to. */
  get apiBearer(): string | null { return this._apiBearer; }

  /** Current connection state. */
  get connectionState(): TransportState { return this._connectionState; }

  /** Whether IRC registration is complete (001 received). */
  get registered(): boolean { return this._registered; }

  /** Set of channels we're currently in (lowercase). */
  get joinedChannels(): ReadonlySet<string> { return this._joinedChannels; }

  /** Active AV sessions. */
  get avSessions(): ReadonlyMap<string, AvSession> { return this._avSessions; }

  /** Active AV session ID we're participating in. */
  get activeAvSession(): string | null { return this._activeAvSession; }

  /** Server origin for API calls. */
  get serverOrigin(): string {
    if (this.opts.serverOrigin) return this.opts.serverOrigin;
    try {
      const u = new URL(this.opts.url);
      return `${u.protocol === 'wss:' ? 'https:' : 'http:'}//${u.host}`;
    } catch {
      return '';
    }
  }

  // ── Connection ──

  /** Connect to the IRC server. */
  connect(): void {
    if (this.transport) {
      try { this.transport.disconnect(); } catch { /* ignore */ }
      this.transport = null;
    }
    this._saslFailed = false;

    let lineQueue: Promise<void> = Promise.resolve();
    const serializedHandleLine = (line: string) => {
      lineQueue = lineQueue.then(() => this.handleLine(line)).catch((e) =>
        console.error('[freeq-sdk] line handler error:', e)
      );
    };

    this.transport = new Transport({
      url: this.opts.url,
      onLine: serializedHandleLine,
      onStateChange: (s) => this.onTransportStateChange(s),
    });
    this.transport.connect();
  }

  /** Disconnect from the server. */
  disconnect(): void {
    this.transport?.disconnect();
    this.transport = null;
    this._nick = '';
    this._authDid = null;
    this._apiBearer = null;
    this._registered = false;
    this._saslFailed = false;
    this.ackedCaps.clear();
    this.sasl = null;
    this._joinedChannels.clear();
    this.backgroundWhois.clear();
    this.echoPlaintextCache.clear();
    this.batches.clear();
    this._avSessions.clear();
    this._activeAvSession = null;
    this._encryptedChannels.clear();
    this._currentAway = null;
    // Clear internal caches and timer state.
    this._nickToDid.clear();
    this._didToNick.clear();
    this._whoisBuffer.clear();
    // Reject any pending whois Promises so callers don't hang forever.
    for (const [, waiters] of this._pendingWhois) {
      for (const w of waiters) {
        clearTimeout(w.timer);
        w.reject(new Error('disconnect()'));
      }
    }
    this._pendingWhois.clear();
    this._seenCoordinationEvents.clear();
    this._nickCollisionRetries = 0;
    if (this._agentHeartbeatTimer) {
      clearInterval(this._agentHeartbeatTimer);
      this._agentHeartbeatTimer = null;
    }
    signing.resetSigning();
    this._connectionState = 'disconnected';
  }

  /** Force an immediate reconnect. */
  reconnect(): void {
    if (!this.opts.url || !this.opts.nick) return;
    this.transport?.disconnect();
    this.transport = null;
    const channels = [...this._joinedChannels];
    this.autoJoinChannels = channels;
    this._nick = this.opts.nick;
    this.connect();
  }

  /** Set SASL credentials (call before connect, or before reconnect). */
  setSaslCredentials(creds: SaslCredentials): void {
    this.sasl = creds;
    if (creds.token) this.skipBrokerRefresh = true;
  }

  // ── Sending ──

  /** Send a message to a channel or user. */
  sendMessage(target: string, text: string, multiline = false): void {
    const isChannel = target.startsWith('#') || target.startsWith('&');
    const wireText = multiline ? text.replace(/\n/g, '\\n') : text;
    const extraTags: Record<string, string> = multiline ? { '+freeq.at/multiline': '' } : {};

    // If the channel is +E and we have no key, the only thing we could
    // send is plaintext — which would leak into a room the rest of the
    // members expect encrypted. Refuse and surface a system message
    // instead so the user sets a passphrase before retrying.
    if (
      isChannel &&
      this._encryptedChannels.has(target.toLowerCase()) &&
      !e2ee.hasChannelKey(target)
    ) {
      this.emit(
        'systemMessage',
        target,
        `Cannot send to ${target}: channel is encrypted (+E) and you have no key set. Use the channel passphrase to enable encryption first.`,
      );
      return;
    }

    if (e2ee.hasChannelKey(target)) {
      e2ee.encryptChannel(target, wireText).then((encrypted) => {
        if (encrypted) {
          this.cacheEchoPlaintext(encrypted, text);
          const tags: Record<string, string> = { '+encrypted': '', ...extraTags };
          this.raw(format('PRIVMSG', [target, encrypted], tags));
        } else {
          this.signedPrivmsg(target, wireText, extraTags);
        }
      });
    } else if (!isChannel && e2ee.isE2eeReady()) {
      const remoteDid = this.didForNick(target);
      if (remoteDid) {
        e2ee.encryptMessage(remoteDid, wireText, this.serverOrigin).then((encrypted) => {
          if (encrypted) {
            this.cacheEchoPlaintext(encrypted, text);
            const tags: Record<string, string> = { '+encrypted': '', ...extraTags };
            this.raw(format('PRIVMSG', [target, encrypted], tags));
          } else {
            this.signedPrivmsg(target, wireText, extraTags);
          }
        });
      } else {
        this.signedPrivmsg(target, wireText, extraTags);
      }
    } else {
      this.signedPrivmsg(target, wireText, extraTags);
    }

    // Local echo if no echo-message cap
    const willEncrypt = e2ee.hasChannelKey(target) || (!isChannel && e2ee.isE2eeReady() && !!this.didForNick(target));
    if (!this.ackedCaps.has('echo-message')) {
      const msg: Message = {
        id: crypto.randomUUID(),
        from: this._nick,
        text,
        timestamp: new Date(),
        tags: {},
        isSelf: true,
        encrypted: willEncrypt,
      };
      this.emit('message', target, msg);
    }
  }

  /** Send a reply to a specific message. */
  sendReply(target: string, replyToMsgId: string, text: string, multiline = false): void {
    const tags: Record<string, string> = { '+reply': replyToMsgId };
    if (multiline) tags['+freeq.at/multiline'] = '';
    this.raw(format('PRIVMSG', [target, text], tags));
  }

  /** Edit a previously sent message. */
  sendEdit(target: string, originalMsgId: string, newText: string, multiline = false): void {
    const tags: Record<string, string> = { '+draft/edit': originalMsgId };
    if (multiline) tags['+freeq.at/multiline'] = '';
    this.raw(format('PRIVMSG', [target, newText], tags));
  }

  /** Send a message with Markdown formatting. */
  sendMarkdown(target: string, text: string): void {
    const isMultiline = text.includes('\n');
    const wireText = isMultiline ? text.replace(/\n/g, '\\n') : text;
    const tags: Record<string, string> = { '+freeq.at/mime': 'text/markdown' };
    if (isMultiline) tags['+freeq.at/multiline'] = '';
    this.signedPrivmsg(target, wireText, tags);

    if (!this.ackedCaps.has('echo-message')) {
      this.emit('message', target, {
        id: crypto.randomUUID(),
        from: this._nick,
        text: wireText,
        timestamp: new Date(),
        tags,
        isSelf: true,
      });
    }
  }

  /** Delete a message. */
  sendDelete(target: string, msgId: string): void {
    this.emit('messageDeleted', target, msgId);
    this.raw(format('TAGMSG', [target], { '+draft/delete': msgId }));
  }

  /** React to a message with an emoji. */
  sendReaction(target: string, emoji: string, msgId?: string): void {
    const tags: Record<string, string> = { '+react': emoji };
    if (msgId) tags['+reply'] = msgId;
    this.raw(format('TAGMSG', [target], tags));

    if (msgId) {
      this.emit('reactionAdded', target, msgId, emoji, this._nick);
    }
  }

  /** Remove our previous reaction to a message. */
  sendUnreact(target: string, emoji: string, msgId: string): void {
    const tags: Record<string, string> = {
      '+freeq.at/unreact': emoji,
      '+reply': msgId,
    };
    this.raw(format('TAGMSG', [target], tags));
    this.emit('reactionRemoved', target, msgId, emoji, this._nick);
  }

  // ── Channel management ──

  /** Join a channel. */
  join(channel: string): void {
    this.raw(`JOIN ${channel}`);
  }

  /** Leave a channel. */
  part(channel: string): void {
    this.raw(`PART ${channel}`);
    this._joinedChannels.delete(channel.toLowerCase());
  }

  /** Set a channel's topic. */
  setTopic(channel: string, topic: string): void {
    this.raw(`TOPIC ${channel} :${topic}`);
  }

  /** Set a channel or user mode. */
  setMode(channel: string, mode: string, arg?: string): void {
    this.raw(arg ? `MODE ${channel} ${mode} ${arg}` : `MODE ${channel} ${mode}`);
  }

  /** Kick a user from a channel. */
  kick(channel: string, nick: string, reason?: string): void {
    this.raw(`KICK ${channel} ${nick} :${reason || 'kicked'}`);
  }

  /** Invite a user to a channel. */
  invite(channel: string, nick: string): void {
    this.raw(`INVITE ${nick} ${channel}`);
  }

  /** Set or clear away status. */
  setAway(reason?: string): void {
    this.pendingAwayReason = reason || null;
    this._currentAway = reason || null;
    this.raw(reason ? `AWAY :${reason}` : 'AWAY');
  }

  /** Fire a WHOIS and resolve with parsed info when 318 (RPL_ENDOFWHOIS)
   *  arrives. Renamed from `whois()` — that name remains as a deprecated
   *  alias for one release. */
  requestWhois(nick: string, opts: { timeoutMs?: number } = {}): Promise<WhoisInfo> {
    const lc = nick.toLowerCase();
    const timeoutMs = opts.timeoutMs ?? 5000;
    return new Promise<WhoisInfo>((resolve, reject) => {
      const timer = setTimeout(() => {
        // Remove this waiter from the queue.
        const queue = this._pendingWhois.get(lc) ?? [];
        const idx = queue.findIndex((w) => w.timer === timer);
        if (idx >= 0) queue.splice(idx, 1);
        if (queue.length === 0) this._pendingWhois.delete(lc);
        else this._pendingWhois.set(lc, queue);
        reject(new Error(`requestWhois('${nick}') timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      const queue = this._pendingWhois.get(lc) ?? [];
      queue.push({ resolve, reject, timer });
      this._pendingWhois.set(lc, queue);
      // Fire WHOIS lazily — multiple concurrent waiters share one request.
      if (queue.length === 1) {
        this.raw(`WHOIS ${nick}`);
      }
    });
  }

  /** @deprecated Use `requestWhois(nick)` (returns `Promise<WhoisInfo>`).
   *  Kept for one release; calling this still fires the `whois` event
   *  on each numeric, same as before. */
  whois(nick: string): void {
    this.raw(`WHOIS ${nick}`);
  }

  /** Request chat history for a target (channel or DM partner).
   *
   *  `opts.mode` selects:
   *    - 'latest' — most recent N messages
   *    - 'before' — N messages before `opts.msgid`
   *    - 'after'  — N messages after `opts.msgid`
   */
  requestHistory(opts: HistoryOptions): void;
  /** @deprecated Use the `HistoryOptions` form. The two-arg form is kept
   *  for backwards compatibility with freeq-app. */
  requestHistory(channel: string, before?: string): void;
  requestHistory(channelOrOpts: string | HistoryOptions, before?: string): void {
    const count = 50;
    let opts: HistoryOptions;
    if (typeof channelOrOpts === 'string') {
      // Legacy positional form: (channel, before?). `before` is treated
      // as a timestamp marker for CHATHISTORY BEFORE (existing behavior).
      if (before) {
        this.raw(`CHATHISTORY BEFORE ${channelOrOpts} timestamp=${before} ${count}`);
      } else {
        this.raw(`CHATHISTORY LATEST ${channelOrOpts} * ${count}`);
      }
      return;
    }
    opts = channelOrOpts;
    const c = opts.count ?? count;
    switch (opts.mode) {
      case 'latest':
        this.raw(`CHATHISTORY LATEST ${opts.target} * ${c}`);
        break;
      case 'before':
        if (!opts.msgid) throw new Error("requestHistory mode='before' requires opts.msgid");
        this.raw(`CHATHISTORY BEFORE ${opts.target} msgid=${opts.msgid} ${c}`);
        break;
      case 'after':
        if (!opts.msgid) throw new Error("requestHistory mode='after' requires opts.msgid");
        this.raw(`CHATHISTORY AFTER ${opts.target} msgid=${opts.msgid} ${c}`);
        break;
    }
  }

  /** Request CHATHISTORY TARGETS — list of recent conversation targets
   *  (channels + DM partners with recent activity).
   *  Each result fires `historyTarget(target, timestamp?)`. */
  requestHistoryTargets(limit = 50): void {
    this.raw(`CHATHISTORY TARGETS * * ${limit}`);
  }

  /** @deprecated Use `requestHistoryTargets(limit)`. CHATHISTORY TARGETS
   *  returns channels too, not just DMs; the original name was misleading.
   *  Kept for one release. */
  requestDmTargets(limit = 50): void {
    this.raw(`CHATHISTORY TARGETS * * ${limit}`);
  }

  /** Pin a message. */
  pin(channel: string, msgid: string): void {
    this.raw(`PIN ${channel} ${msgid}`);
  }

  /** Unpin a message. */
  unpin(channel: string, msgid: string): void {
    this.raw(`UNPIN ${channel} ${msgid}`);
  }

  /** Send a raw IRC command. */
  raw(line: string): void {
    // Defense in depth against the silent-guest-fallback bug: if SASL
    // was attempted and failed on this socket, refuse to write anything
    // that could leak under the guest identity the server would have
    // assigned. The transport is normally already torn down by the 904
    // handler, but a queued send during the close window is still
    // possible.
    if (this._saslFailed) return;
    this.transport?.send(line);
  }

  /** Set a channel encryption passphrase (ENC1). */
  async setChannelEncryption(channel: string, passphrase: string): Promise<void> {
    await e2ee.setChannelKey(channel, passphrase);
  }

  /** Remove channel encryption. */
  removeChannelEncryption(channel: string): void {
    e2ee.removeChannelKey(channel);
  }

  /** Initialize E2EE for DMs (called automatically after SASL success). */
  async initializeE2EE(did: string): Promise<void> {
    await e2ee.initialize(did, this.serverOrigin);
  }

  /** Get the E2EE safety number for a DM partner. */
  async getSafetyNumber(remoteDid: string): Promise<string | null> {
    return e2ee.getSafetyNumber(remoteDid);
  }

  /** Fetch pinned messages for a channel via REST API.
   *  Returns the fetched pins; also fires the `pins` event for any
   *  subscribers. Returns an empty array on failure. */
  async fetchPins(channel: string): Promise<PinnedMessage[]> {
    try {
      const name = channel.startsWith('#') ? channel.slice(1) : channel;
      const resp = await fetch(`${this.serverOrigin}/api/v1/channels/${encodeURIComponent(name)}/pins`);
      if (resp.ok) {
        const data = await resp.json();
        const pins: PinnedMessage[] = data.pins || [];
        this.emit('pins', channel, pins);
        return pins;
      }
    } catch { /* ignore */ }
    return [];
  }

  // ── Internals ──

  private onTransportStateChange(state: TransportState): void {
    const prev = this._connectionState;
    this._connectionState = state;
    this.emit('connectionStateChanged', state);

    // Discrete transition events (complement `connectionStateChanged`).
    if (state === 'connected' && prev !== 'connected') {
      this.emit('connected');
    } else if (state === 'disconnected' && prev !== 'disconnected') {
      this.emit('disconnected', 'transport closed');
    }

    if (state === 'connected') {
      this.ackedCaps.clear();
      let registrationSent = false;

      const sendRegistration = (token?: string) => {
        if (registrationSent) return;
        registrationSent = true;
        if (token && this.sasl) this.sasl.token = token;
        this.raw('CAP LS 302');
        this.raw(`NICK ${this._nick}`);
        this.raw(`USER ${this._nick} 0 * :freeq sdk`);
      };

      const safetyTimer = setTimeout(() => {
        if (!registrationSent) {
          console.warn('[freeq-sdk] Registration safety timeout — sending as guest');
          this.sasl = null;
          sendRegistration();
        }
      }, 8000);

      const brokerToken = this.opts.brokerToken;
      const brokerBase = this.opts.brokerUrl;

      // Skip broker refresh when we have token-based credentials (the
      // broker would re-mint them anyway) OR when we have a signer
      // (did:key auth: no broker needed, no token to refresh).
      if (this.skipBrokerRefresh && (this.sasl?.token || this.sasl?.signer)) {
        this.skipBrokerRefresh = false;
        clearTimeout(safetyTimer);
        sendRegistration();
      } else if (this.sasl?.signer) {
        // did:key flow — bypass broker entirely.
        clearTimeout(safetyTimer);
        sendRegistration();
      } else if (brokerToken && brokerBase && this.sasl?.did) {
        const ctrl = new AbortController();
        const tm = setTimeout(() => ctrl.abort(), 8000);
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
              if (r.status === 401) throw new Error('broker token invalid');
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
            if (this.sasl?.token) {
              sendRegistration();
            } else {
              this.sasl = null;
              sendRegistration();
            }
          });
      } else {
        clearTimeout(safetyTimer);
        sendRegistration();
      }
    }
  }

  private didForNick(targetNick: string): string | undefined {
    // Internal cache first (populated from WHOIS 330 + JOIN account tags).
    // Falls back to the legacy external `nickToDid` resolver an app layer
    // may have set. New code should use the public `getDidForNick()`.
    return this._nickToDid.get(targetNick.toLowerCase()) ?? this.nickToDid?.(targetNick);
  }

  /** Resolve nick to DID — set by the app layer for E2EE support. */
  nickToDid: ((nick: string) => string | undefined) | null = null;

  private resolveNickToDid(targetNick: string): string | undefined {
    return this.nickToDid?.(targetNick);
  }

  /** Parse a `+freeq.at/event=*` TAGMSG/PRIVMSG and emit `coordinationEvent`.
   *  De-dupes by eventId so the paired TAGMSG + companion PRIVMSG fire
   *  the event only once. */
  private emitCoordinationEvent(channel: string, from: string, tags: Record<string, string>): void {
    const eventType = tags['+freeq.at/event'];
    if (!eventType) return;
    const eventId = tags['msgid'] || '';
    if (eventId) {
      const now = Date.now();
      const seen = this._seenCoordinationEvents.get(eventId);
      if (seen !== undefined && now - seen < 30_000) return; // dup
      this._seenCoordinationEvents.set(eventId, now);
      // Trim periodically.
      if (this._seenCoordinationEvents.size > 1000) {
        const cutoff = now - 30_000;
        for (const [k, t] of this._seenCoordinationEvents) {
          if (t < cutoff) this._seenCoordinationEvents.delete(k);
        }
      }
    }
    // Payload is percent-encoded JSON per the wire format.
    let payload: unknown = null;
    const rawPayload = tags['+freeq.at/payload'];
    if (rawPayload) {
      try {
        payload = JSON.parse(decodeURIComponent(rawPayload));
      } catch {
        payload = null;
      }
    }
    const did = this.getDidForNick(from);
    const taskId = tags['+freeq.at/task-id'] || tags['+freeq.at/ref'];
    const evidenceType = tags['+freeq.at/evidence-type'];
    const eventPayload: CoordinationEventPayload = {
      channel,
      from,
      did,
      eventType,
      eventId,
      taskId: taskId || undefined,
      evidenceType: evidenceType || undefined,
      payload,
      tags,
    };
    this.emit('coordinationEvent', eventPayload);
  }

  private async signedPrivmsg(target: string, text: string, extraTags?: Record<string, string>): Promise<void> {
    const sig = await signing.signMessage(target, text);
    const tags: Record<string, string> = { ...extraTags };
    if (sig) tags['+freeq.at/sig'] = sig;
    if (Object.keys(tags).length > 0) {
      this.raw(format('PRIVMSG', [target, text], tags));
    } else {
      this.raw(`PRIVMSG ${target} :${text}`);
    }
  }

  private cacheEchoPlaintext(ciphertext: string, plaintext: string): void {
    this.echoPlaintextCache.set(ciphertext, { plaintext, ts: Date.now() });
    if (this.echoPlaintextCache.size > 100) {
      const now = Date.now();
      for (const [k, v] of this.echoPlaintextCache) {
        if (now - v.ts > 60_000) this.echoPlaintextCache.delete(k);
      }
    }
  }

  private async handleLine(rawLine: string): Promise<void> {
    const msg = parse(rawLine);
    const from = prefixNick(msg.prefix);

    this.emit('raw', rawLine, msg);

    switch (msg.command) {
      case 'CAP':
        this.handleCap(msg);
        break;

      case 'AUTHENTICATE':
        await this.handleAuthenticate(msg);
        break;

      case '900':
        this._authDid = this.sasl?.did ?? null;
        this.emit('authenticated', this._authDid || '', msg.params[msg.params.length - 1]);
        if (this._authDid) {
          prefetchProfiles([this._authDid]);
          e2ee.initialize(this._authDid, this.serverOrigin).catch((e) =>
            console.warn('[e2ee] Init failed:', e)
          );
        }
        break;

      case '903':
        // Auto-mint a per-session ed25519 signing key and register it via
        // MSGSIG. Some consumers (Node-side bots, agents that already hold
        // their own signing key) want to skip this; opt out via
        // FreeqClientOptions.autoMsgSig=false.
        if (this.sasl?.did && this.opts.autoMsgSig !== false) {
          signing.setSigningDid(this.sasl.did);
          signing.generateSigningKey().then((pubkey) => {
            if (pubkey) this.raw(`MSGSIG ${pubkey}`);
          });
        }
        this.raw('CAP END');
        break;

      case '904': {
        // SASL failed. The user expected to be authenticated, but our
        // credentials (often a token that went stale during an idle
        // reconnect) didn't validate. The server will now finish IRC
        // registration and force-rename us to GuestNNNNN since the nick
        // is registered to a DID we can't prove ownership of.
        //
        // We MUST NOT silently let registration complete as a guest:
        // the user would post messages under the guest identity while
        // the UI still shows them as authenticated. Drop the dead
        // credentials and intentionally tear the socket down so the
        // app can re-auth (or explicitly choose guest mode) instead of
        // racing the next reconnect with the same dead token.
        const reason = msg.params[msg.params.length - 1] || 'SASL failed';
        const hadSaslAttempt = !!this.sasl?.token;
        this.sasl = null;
        this._authDid = null;
        this._apiBearer = null;
        this.emit('authError', reason);
        // Mirror the wire identity to the app: did is now empty.
        this.emit('authenticated', '', reason);
        if (hadSaslAttempt) {
          // Refuse to register as a guest on a connection where SASL
          // was requested. Mark _saslFailed so any in-flight 001 from
          // the server is suppressed (the WS may still deliver buffered
          // lines for a moment after close), and tear down the socket
          // so the next user action is an explicit re-auth.
          this._saslFailed = true;
          this.transport?.disconnect();
          this.transport = null;
          this._connectionState = 'disconnected';
          this.emit('connectionStateChanged', 'disconnected');
        } else {
          this.raw('CAP END');
        }
        break;
      }

      case 'PING':
        this.raw(`PONG :${msg.params[0] || ''}`);
        break;

      case 'ERROR': {
        const reason = msg.params[0] || '';
        this.emit('error', reason);
        if (reason.includes('same identity reconnected')) {
          this.transport?.disconnect();
        }
        break;
      }

      case '001': {
        const serverNick = msg.params[0] || this._nick;
        // If SASL failed on this socket, suppress any in-flight 001
        // from the server. We've already torn the socket down; do not
        // let the app think we registered as the assigned Guest nick.
        if (this._saslFailed) break;
        this.guestFallbackCount = 0;
        this._nick = serverNick;
        this._registered = true;
        this.emit('registered', this._nick);
        this.emit('nickChanged', this._nick);

        const toJoin = this.autoJoinChannels.length > 0
          ? this.autoJoinChannels
          : (this.sasl?.did ? [] : (this._joinedChannels.size > 0 ? [...this._joinedChannels] : ['#freeq']));
        if (!this.sasl?.did && toJoin.length === 0) toJoin.push('#freeq');
        for (const ch of toJoin) {
          if (ch.trim()) this.raw(`JOIN ${ch.trim()}`);
        }
        this.autoJoinChannels = [];
        if (this.sasl?.did) this.requestDmTargets();
        // Re-assert AWAY across reconnects so the server stops thinking
        // we're present. We deliberately re-send even on the first 001
        // if _currentAway was set earlier; it's a no-op if we weren't
        // away.
        if (this._currentAway !== null) {
          this.raw(`AWAY :${this._currentAway}`);
        }
        this.emit('ready');
        break;
      }

      case '433': {
        // 433 ERR_NICKNAMEINUSE — apply onNickCollision policy.
        const policy = this.opts.onNickCollision ?? 'auto-suffix';
        if (policy === 'refuse') {
          this.emit('authError', `nick '${this._nick}' is already taken`);
          this.transport?.disconnect();
          this.transport = null;
          this._connectionState = 'disconnected';
          this.emit('connectionStateChanged', 'disconnected');
        } else if (policy === 'random-suffix') {
          const MAX_RETRIES = 3;
          if (this._nickCollisionRetries >= MAX_RETRIES) {
            this.emit('authError', `exhausted ${MAX_RETRIES} nick collision retries for '${this.opts.nick}'`);
            this.transport?.disconnect();
            this.transport = null;
            this._connectionState = 'disconnected';
            this.emit('connectionStateChanged', 'disconnected');
            break;
          }
          this._nickCollisionRetries++;
          const suffix = Math.floor(1000 + Math.random() * 9000).toString();
          this._nick = `${this.opts.nick}-${suffix}`;
          this.raw(`NICK ${this._nick}`);
        } else {
          // auto-suffix (legacy default): append `_` and retry.
          this._nick += '_';
          this.raw(`NICK ${this._nick}`);
        }
        break;
      }

      case 'NICK': {
        const newNick = msg.params[0];
        if (from.toLowerCase() === this._nick.toLowerCase()) {
          this._nick = newNick;
          this.emit('nickChanged', this._nick);
        }
        this.emit('userRenamed', from, newNick);
        break;
      }

      case 'JOIN': {
        const channel = msg.params[0];
        const account = msg.params[1];
        const isSelf = from.toLowerCase() === this._nick.toLowerCase();
        if (isSelf) {
          this._joinedChannels.add(channel.toLowerCase());
          this.emit('channelJoined', channel);
          this.emit('membersCleared', channel);
          this.fetchPins(channel);
        }
        const joinDid = account && account !== '*' ? account : undefined;
        const actorClass = (msg.tags?.['freeq.at/actor-class'] || msg.tags?.['+freeq.at/actor-class']) as Member['actorClass'] | undefined;
        this.emit('memberJoined', channel, { nick: from, did: joinDid, actorClass });
        if (joinDid) {
          prefetchProfiles([joinDid]);
          // Populate internal nick↔DID cache (account-notify tag carries DID).
          const lc = from.toLowerCase();
          this._nickToDid.set(lc, joinDid);
          this._didToNick.set(joinDid, lc);
        }
        // Spawned-agent broadcast (`+freeq.at/parent=<nick>` indicates
        // a child agent joining the channel; see server connection/mod.rs
        // SPAWN handler).
        const parent = msg.tags['+freeq.at/parent'];
        if (parent) {
          this.emit('agentSpawned', {
            parentNick: parent,
            childNick: from,
            channel,
            capabilities: [],
            ttlSeconds: undefined,
            taskRef: undefined,
          });
        }
        this.emit('systemMessage', channel, `${from} joined`);
        break;
      }

      case 'PART': {
        const channel = msg.params[0];
        if (from.toLowerCase() === this._nick.toLowerCase()) {
          this._joinedChannels.delete(channel.toLowerCase());
          this.emit('channelLeft', channel);
        } else {
          this.emit('memberLeft', channel, from);
          this.emit('systemMessage', channel, `${from} left`);
        }
        break;
      }

      case 'QUIT': {
        const reason = msg.params[0] || '';
        this.emit('userQuit', from, reason);
        // Spawned-child despawn pattern: hostmask is `*!spawn@freeq/spawn*`
        // when the server tears down a TTL'd or explicitly despawned
        // child agent. Mirror to `agentDespawned`.
        if (msg.prefix.includes('!spawn@freeq/spawn')) {
          this.emit('agentDespawned', { nick: from, reason: reason || undefined });
        }
        // Forget any cached DID binding for this nick.
        const lc = from.toLowerCase();
        const did = this._nickToDid.get(lc);
        this._nickToDid.delete(lc);
        if (did) this._didToNick.delete(did);
        break;
      }

      case 'KICK': {
        const channel = msg.params[0];
        const kicked = msg.params[1];
        const reason = msg.params[2] || '';
        if (kicked.toLowerCase() === this._nick.toLowerCase()) {
          this._joinedChannels.delete(channel.toLowerCase());
          this.emit('channelLeft', channel);
          this.emit('systemMessage', 'server', `Kicked from ${channel} by ${from}: ${reason}`);
        } else {
          this.emit('userKicked', channel, kicked, from, reason);
          this.emit('systemMessage', channel, `${kicked} kicked by ${from}${reason ? `: ${reason}` : ''}`);
        }
        break;
      }

      case 'PRIVMSG': {
        const target = msg.params[0];
        const text = msg.params[1] || '';
        const isAction = text.startsWith('\x01ACTION ') && text.endsWith('\x01');
        const isChannel = target.startsWith('#') || target.startsWith('&');
        const isSelf = from.toLowerCase() === this._nick.toLowerCase();
        const bufName = isChannel ? target : (isSelf ? target : from);

        // Coordination event companion PRIVMSG. The paired TAGMSG fires
        // `coordinationEvent` first; the de-dupe in emitCoordinationEvent
        // suppresses the second fire. We still emit the regular `message`
        // event below so human-readable text renders normally.
        if (msg.tags['+freeq.at/event']) {
          this.emitCoordinationEvent(target, from, msg.tags);
        }

        const editOf = msg.tags['+draft/edit'];
        if (editOf) {
          const isStreaming = msg.tags['+freeq.at/streaming'] === '1';
          this.emit('messageEdited', bufName, editOf, text, msg.tags['msgid'], isStreaming);
          break;
        }

        let displayText = isAction ? text.slice(8, -1) : text;
        let isEncryptedMsg = false;

        const cachedPlain = this.echoPlaintextCache.get(text);
        if (cachedPlain && isSelf) {
          displayText = cachedPlain.plaintext;
          isEncryptedMsg = true;
          this.echoPlaintextCache.delete(text);
        } else if (e2ee.isENC1(text) && isChannel) {
          const plain = await e2ee.decryptChannel(target, text);
          if (plain !== null) { displayText = plain; isEncryptedMsg = true; }
          else { displayText = '[encrypted message]'; isEncryptedMsg = true; }
        } else if (e2ee.isEncrypted(text) && !isChannel && !isSelf) {
          const remoteDid = this.resolveNickToDid(from);
          if (remoteDid) {
            const plain = await e2ee.decryptMessage(remoteDid, text, this.serverOrigin);
            if (plain !== null) { displayText = plain; isEncryptedMsg = true; }
            else { displayText = '[encrypted DM — could not decrypt]'; isEncryptedMsg = true; }
          } else {
            displayText = '[encrypted DM — unknown sender identity]'; isEncryptedMsg = true;
          }
        } else if (e2ee.isEncrypted(text) && !isChannel && isSelf) {
          displayText = '[encrypted message]'; isEncryptedMsg = true;
        }
        if (msg.tags['+encrypted']) isEncryptedMsg = true;

        const message: Message = {
          id: msg.tags['msgid'] || crypto.randomUUID(),
          from,
          text: displayText,
          timestamp: msg.tags['time'] ? new Date(msg.tags['time']) : new Date(),
          tags: msg.tags,
          isAction,
          isSelf,
          replyTo: msg.tags['+reply'],
          encrypted: isEncryptedMsg,
          isStreaming: msg.tags['+freeq.at/streaming'] === '1',
        };

        // Parse persisted reactions from CHATHISTORY
        const reactionsTag = msg.tags['+freeq.at/reactions'];
        if (reactionsTag && message.id) {
          for (const part of reactionsTag.split(';')) {
            const [emoji, nicks] = part.split(':');
            if (emoji && nicks) {
              for (const n of nicks.split(',')) {
                if (n) {
                  message.reactions = message.reactions || new Map();
                  const set = message.reactions.get(emoji) || new Set();
                  set.add(n);
                  message.reactions.set(emoji, set);
                }
              }
            }
          }
        }

        // Background WHOIS for DM partners
        if (!isChannel && !isSelf && !this.resolveNickToDid(from) && !this.backgroundWhois.has(from.toLowerCase()) && this.backgroundWhois.size < 500) {
          this.backgroundWhois.add(from.toLowerCase());
          this.raw(`WHOIS ${from}`);
        }

        // Check if this message belongs to a batch
        const batchId = msg.tags['batch'];
        if (batchId && this.batches.has(batchId)) {
          this.batches.get(batchId)!.messages.push(message);
          break;
        }

        this.emit('message', bufName, message);

        // Mention detection
        const isMention = !message.isSelf && text.toLowerCase().includes(this._nick.toLowerCase());
        const isDM = !isChannel && !message.isSelf;
        if (isMention || isDM) {
          // Emitted so the app can show notifications / increment badges
          this.emit('systemMessage', '__mention__', JSON.stringify({ channel: bufName, from, text, isDM, isMention }));
        }
        break;
      }

      case 'NOTICE': {
        const target = msg.params[0];
        const text = msg.params[1] || '';
        const buf = target === '*' || target === this._nick ? 'server' : target;

        const noticeActorClass = (msg.tags?.['freeq.at/actor-class'] || msg.tags?.['+freeq.at/actor-class']) as Member['actorClass'] | undefined;
        if (noticeActorClass && from && (target.startsWith('#') || target.startsWith('&'))) {
          this.emit('memberJoined', target, { nick: from, actorClass: noticeActorClass });
        }

        // API bearer (sent by the server immediately after SASL success).
        // Capture so the bot can use the same identity it just authenticated
        // to IRC with when calling the /agent/tools/* HTTP surface. The
        // bearer is the bot's IRC session_id, which only the server knows;
        // without this NOTICE there's no production path for a bot to
        // discover its own bearer.
        const bearerMatch = text.match(/^API-BEARER (\S+)$/);
        if (bearerMatch) {
          this._apiBearer = bearerMatch[1];
          break; // suppress; do not surface to systemMessage
        }

        // AV ticket
        const ticketMatch = text.match(/^AV ticket: (.+)$/);
        if (ticketMatch) {
          const activeId = this._activeAvSession;
          if (activeId) this.emit('avTicket', activeId, ticketMatch[1]);
          break;
        }

        // Pin/unpin sync
        const pinMsgid = msg.tags?.['+freeq.at/pin'];
        const unpinMsgid = msg.tags?.['+freeq.at/unpin'];
        if (pinMsgid && (target.startsWith('#') || target.startsWith('&'))) {
          this.emit('pinAdded', target, pinMsgid, from);
        }
        if (unpinMsgid && (target.startsWith('#') || target.startsWith('&'))) {
          this.emit('pinRemoved', target, unpinMsgid);
        }

        const isAction = text.startsWith('\x01ACTION ') && text.endsWith('\x01');
        if (isAction) {
          this.emit('systemMessage', buf, `${from} ${text.slice(8, -1)}`);
        } else {
          this.emit('systemMessage', buf, `[${from || 'server'}] ${text}`);
        }
        break;
      }

      case 'TAGMSG': {
        const target = msg.params[0];
        const isChannel = target.startsWith('#') || target.startsWith('&');
        const isSelf = from.toLowerCase() === this._nick.toLowerCase();
        const bufName = isChannel ? target : (isSelf ? target : from);

        const deleteOf = msg.tags['+draft/delete'];
        if (deleteOf) { this.emit('messageDeleted', bufName, deleteOf); break; }

        const reaction = msg.tags['+react'];
        if (reaction) {
          const reactTarget = msg.tags['+reply'];
          if (reactTarget) {
            this.emit('reactionAdded', bufName, reactTarget, reaction, from);
          }
        }

        const unreact = msg.tags['+freeq.at/unreact'];
        if (unreact) {
          const unreactTarget = msg.tags['+reply'];
          if (unreactTarget) {
            this.emit('reactionRemoved', bufName, unreactTarget, unreact, from);
          }
        }

        const typing = msg.tags['+typing'];
        if (typing) {
          this.emit('typing', bufName, from, typing === 'active');
        }

        // Governance signal (TAGMSG to a specific nick, usually us).
        const govSignal = msg.tags['+freeq.at/governance'];
        if (govSignal) {
          const validSignals: GovernanceSignal[] = ['pause', 'resume', 'revoke', 'approval_granted', 'approval_denied', 'budget_exceeded'];
          if ((validSignals as readonly string[]).includes(govSignal)) {
            this.emit('governance', {
              signal: govSignal as GovernanceSignal,
              target,
              by: from || undefined,
              reason: msg.tags['+freeq.at/detail'] || undefined,
            });
          }
        }

        // Coordination event (+freeq.at/event=*). Server stores these
        // from TAGMSG; PRIVMSG companion fires the same event below.
        // De-dupe by eventId so handlers fire at most once per pair.
        const eventType = msg.tags['+freeq.at/event'];
        if (eventType) {
          this.emitCoordinationEvent(target, from, msg.tags);
        }

        const avState = msg.tags['+freeq.at/av-state'];
        const avId = msg.tags['+freeq.at/av-id'];
        if (avState && avId) {
          this.handleAvSessionState(avId, avState, target,
            msg.tags['+freeq.at/av-actor'] || '',
            parseInt(msg.tags['+freeq.at/av-participants'] || '0', 10),
            msg.tags['+freeq.at/av-title']);
        }
        break;
      }

      case 'TOPIC': {
        const channel = msg.params[0];
        this.emit('topicChanged', channel, msg.params[1] || '', from);
        break;
      }
      case '332': {
        const channel = msg.params[1];
        this.emit('topicChanged', channel, msg.params[2] || '');
        break;
      }

      case '353': {
        const channel = msg.params[2];
        const nicks = (msg.params[3] || '').split(' ').filter(Boolean);
        const members: Array<Partial<Member> & { nick: string }> = [];
        for (const n of nicks) {
          const prefixMatch = n.match(/^([@%+]+)/);
          const prefixes = prefixMatch ? prefixMatch[1] : '';
          const bare = n.slice(prefixes.length);
          members.push({
            nick: bare,
            isOp: prefixes.includes('@'),
            isHalfop: prefixes.includes('%'),
            isVoiced: prefixes.includes('+'),
          });
        }
        this.emit('membersList', channel, members);
        break;
      }

      case '366': {
        const namesChannel = msg.params[1];
        this.requestHistory(namesChannel);
        break;
      }

      case 'MODE': {
        const target = msg.params[0];
        if (target.startsWith('#') || target.startsWith('&')) {
          const modeStr = msg.params[1] || '';
          const argsWithParam = new Set(['o', 'h', 'v', 'k', 'b']);
          const targetLower = target.toLowerCase();
          let adding = true;
          let argIdx = 2;
          for (const ch of modeStr) {
            if (ch === '+') { adding = true; continue; }
            if (ch === '-') { adding = false; continue; }
            const modeArg = argsWithParam.has(ch) ? msg.params[argIdx++] : undefined;
            // Track +E so we can block plaintext sends; drop the cached
            // e2ee key on -E so we don't keep encrypting with a key the
            // rest of the channel no longer expects.
            if (ch === 'E') {
              if (adding) {
                this._encryptedChannels.add(targetLower);
              } else {
                this._encryptedChannels.delete(targetLower);
                e2ee.removeChannelKey(target);
              }
            }
            this.emit('modeChanged', target, `${adding ? '+' : '-'}${ch}`, modeArg, from);
          }
          const allArgs = msg.params.slice(2).join(' ');
          this.emit('systemMessage', target, `${from} set mode ${modeStr}${allArgs ? ' ' + allArgs : ''}`);
        }
        break;
      }

      case 'AWAY': {
        const awayText = msg.params[0] || null;
        this.emit('userAway', from, awayText);
        // Server broadcasts structured PRESENCE updates via the AWAY
        // mechanism (see freeq-server connection/mod.rs PRESENCE handler).
        // Format: either "<state>" alone, or "<state>: <status text>".
        // Parse back into the structured `presence` event.
        if (awayText) {
          const colonIdx = awayText.indexOf(':');
          let state: string = awayText;
          let status: string | undefined;
          if (colonIdx > 0) {
            state = awayText.slice(0, colonIdx).trim();
            status = awayText.slice(colonIdx + 1).trim() || undefined;
          }
          this.emit('presence', {
            nick: from,
            did: this.getDidForNick(from),
            state,
            status,
            task: undefined,
          });
        } else {
          // AWAY cleared = back to online.
          this.emit('presence', {
            nick: from,
            did: this.getDidForNick(from),
            state: 'online',
          });
        }
        break;
      }

      case '306':
        this.emit('userAway', this._nick, this.pendingAwayReason || 'away');
        this.pendingAwayReason = null;
        this.emit('systemMessage', 'server', `You are now away: ${this.pendingAwayReason || 'away'}`);
        break;

      case '305':
        this.pendingAwayReason = null;
        this._currentAway = null;
        this.emit('userAway', this._nick, null);
        this.emit('systemMessage', 'server', 'You are no longer away');
        break;

      case 'BATCH': {
        const ref = msg.params[0];
        if (ref.startsWith('+')) {
          this.batches.set(ref.slice(1), {
            type: msg.params[1] || '',
            target: msg.params[2] || '',
            messages: [],
          });
        } else if (ref.startsWith('-')) {
          const id = ref.slice(1);
          const batch = this.batches.get(id);
          if (batch) {
            this.batches.delete(id);
            this.emit('historyBatch', batch.target, batch.messages);
          }
        }
        break;
      }

      case 'CHATHISTORY': {
        const sub = msg.params[0];
        if (sub === 'TARGETS' && msg.params[1]) {
          const targetNick = msg.params[1];
          const timestamp = msg.params[2] || undefined;
          // Canonical event name (renamed from `dmTarget` — CHATHISTORY
          // TARGETS returns channels too, not just DMs).
          this.emit('historyTarget', targetNick, timestamp);
          // Deprecated alias — kept for one release for backwards compat.
          this.emit('dmTarget', targetNick);
          this.requestHistory(targetNick);
        }
        break;
      }

      case 'INVITE':
        if (msg.params.length >= 2) {
          this.emit('invited', msg.params[1], from);
          this.emit('systemMessage', 'server', `${from} invited you to ${msg.params[1]}`);
        }
        break;

      // Error numerics
      case '401': {
        const failNick = msg.params[1];
        this.emit('systemMessage', failNick || 'server',
          `${failNick} is offline — message saved, they'll see it next time they connect`);
        break;
      }
      case '404':
        this.emit('systemMessage', msg.params[1] || 'server', msg.params[2] || 'Cannot send to channel');
        break;
      case '473':
        this.emit('systemMessage', msg.params[1] || 'server', `Cannot join ${msg.params[1]} — invite only (+i)`);
        break;
      case '474':
        this.emit('systemMessage', msg.params[1] || 'server', `Cannot join ${msg.params[1]} — you are banned`);
        break;
      case '475':
        this.emit('systemMessage', msg.params[1] || 'server', `Cannot join ${msg.params[1]} — incorrect channel key`);
        break;
      case '477': {
        const ch = msg.params[1] || '';
        this.emit('systemMessage', 'server', `Cannot join ${ch}: ${msg.params[2] || 'Policy acceptance required'}`);
        this.emit('joinGateRequired', ch);
        break;
      }
      case '482':
        this.emit('systemMessage', msg.params[1] || 'server', msg.params[2] || 'Not operator');
        break;

      // WHOIS
      case '311': {
        const whoisNick = msg.params[1] || '';
        const info = {
          user: msg.params[2],
          host: msg.params[3],
          realname: msg.params[5] || msg.params[4],
          did: undefined,
          handle: undefined,
        };
        this.emit('whois', whoisNick, info);
        // Accumulate for requestWhois() Promise.
        const lc = whoisNick.toLowerCase();
        const buf = this._whoisBuffer.get(lc) ?? { nick: whoisNick, fetchedAt: 0 };
        buf.user = info.user;
        buf.host = info.host;
        buf.realname = info.realname;
        this._whoisBuffer.set(lc, buf);
        if (!this.backgroundWhois.has(lc)) {
          this.emit('systemMessage', 'server', `WHOIS ${whoisNick}: ${msg.params[2]}@${msg.params[3]} (${msg.params[5] || msg.params[4]})`);
        }
        break;
      }
      case '312': {
        const whoisNick = msg.params[1] || '';
        this.emit('whois', whoisNick, { server: msg.params[2] });
        const lc = whoisNick.toLowerCase();
        const buf = this._whoisBuffer.get(lc) ?? { nick: whoisNick, fetchedAt: 0 };
        buf.server = msg.params[2];
        this._whoisBuffer.set(lc, buf);
        if (!this.backgroundWhois.has(lc)) {
          this.emit('systemMessage', 'server', `  Server: ${msg.params[2]}`);
        }
        break;
      }
      case '318': {
        // End of WHOIS. Resolve any pending requestWhois() Promise(s)
        // for this nick with the accumulated info.
        const lc = (msg.params[1] || '').toLowerCase();
        this.backgroundWhois.delete(lc);
        const buf = this._whoisBuffer.get(lc);
        this._whoisBuffer.delete(lc);
        const waiters = this._pendingWhois.get(lc);
        if (waiters) {
          this._pendingWhois.delete(lc);
          const info: WhoisInfo = {
            nick: buf?.nick ?? msg.params[1] ?? '',
            user: buf?.user,
            host: buf?.host,
            realname: buf?.realname,
            server: buf?.server,
            did: buf?.did,
            handle: buf?.handle,
            channels: buf?.channels,
            fetchedAt: Date.now(),
          };
          for (const w of waiters) {
            clearTimeout(w.timer);
            w.resolve(info);
          }
        }
        break;
      }
      case '319': {
        const whoisNick = msg.params[1] || '';
        this.emit('whois', whoisNick, { channels: msg.params[2] });
        const lc = whoisNick.toLowerCase();
        const buf = this._whoisBuffer.get(lc) ?? { nick: whoisNick, fetchedAt: 0 };
        buf.channels = msg.params[2];
        this._whoisBuffer.set(lc, buf);
        if (!this.backgroundWhois.has(lc)) {
          this.emit('systemMessage', 'server', `  Channels: ${msg.params[2]}`);
        }
        break;
      }
      case '330': {
        const whoisNick = msg.params[1] || '';
        const did = msg.params[2]?.trim() || undefined;
        this.emit('whois', whoisNick, { did });
        if (whoisNick && did) {
          this.emit('memberDid', whoisNick, did);
          // Populate internal bidirectional cache (used by getDidForNick /
          // getNickForDid / requestWhois). Lowercase nick key for
          // case-insensitive lookup. Forget any previous nick that was
          // bound to this DID (e.g. after NICK change).
          const lc = whoisNick.toLowerCase();
          const prevDid = this._nickToDid.get(lc);
          if (prevDid && prevDid !== did) this._didToNick.delete(prevDid);
          const prevNick = this._didToNick.get(did);
          if (prevNick && prevNick !== lc) this._nickToDid.delete(prevNick);
          this._nickToDid.set(lc, did);
          this._didToNick.set(did, lc);
          // Accumulate for the requestWhois() Promise.
          const buf = this._whoisBuffer.get(lc) ?? { nick: whoisNick, fetchedAt: 0 };
          buf.did = did;
          this._whoisBuffer.set(lc, buf);
          prefetchProfiles([did]);
        }
        if (!this.backgroundWhois.has(whoisNick.toLowerCase())) {
          this.emit('systemMessage', 'server', `  DID: ${did}`);
        }
        break;
      }
      case '673': {
        const whoisNick = msg.params[1] || '';
        const classStr = msg.params[2] || '';
        const match = classStr.match(/actor_class=(\w+)/);
        if (match && whoisNick) {
          this.emit('memberJoined', '', { nick: whoisNick, actorClass: match[1] as Member['actorClass'] });
        }
        if (!this.backgroundWhois.has(whoisNick.toLowerCase())) {
          this.emit('systemMessage', 'server', `  Actor class: ${classStr}`);
        }
        break;
      }
      case '671': {
        const whoisNick = msg.params[1] || '';
        const handle = msg.params[2]?.trim();
        this.emit('whois', whoisNick, { handle });
        const lc = whoisNick.toLowerCase();
        const buf = this._whoisBuffer.get(lc) ?? { nick: whoisNick, fetchedAt: 0 };
        buf.handle = handle;
        this._whoisBuffer.set(lc, buf);
        if (!this.backgroundWhois.has(lc)) {
          this.emit('systemMessage', 'server', `  Handle: ${handle}`);
        }
        break;
      }

      // Channel list
      case '321':
        break;
      case '322': {
        const chName = msg.params[1] || '';
        const chCount = parseInt(msg.params[2] || '0', 10);
        const chTopic = msg.params[3] || '';
        this.emit('channelListEntry', { name: chName, topic: chTopic, count: chCount });
        this.emit('systemMessage', 'server', `  ${chName} (${chCount}) ${chTopic}`);
        break;
      }
      case '323':
        this.emit('channelListEnd');
        break;

      // MOTD
      case '375':
        this.emit('motdStart');
        this.emit('systemMessage', 'server', msg.params[msg.params.length - 1]);
        break;
      case '372': {
        const motdLine = msg.params[msg.params.length - 1];
        this.emit('systemMessage', 'server', motdLine);
        this.emit('motd', motdLine.replace(/^- ?/, ''));
        break;
      }

      default:
        if (/^\d{3}$/.test(msg.command)) {
          this.emit('systemMessage', 'server', msg.params.slice(1).join(' '));
        }
        break;
    }
  }

  private handleCap(msg: IRCMessage): void {
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
      // Negotiate `sasl` whenever the bot has SOME way to authenticate:
      // either a pre-issued token (pds-session/pds-oauth) OR a signer
      // callback (crypto / did:key). Previously only the token branch
      // qualified, so JS bots using did:key never reached SASL.
      const wantsSasl = (this.sasl?.token || this.sasl?.signer) && available.includes('sasl');
      if (wantsSasl) {
        wantedCaps.push('sasl');
      }
      if (wantedCaps.length) {
        this.raw(`CAP REQ :${wantedCaps.join(' ')}`);
      } else {
        this.raw('CAP END');
      }
    } else if (sub === 'ACK') {
      const caps = msg.params.slice(2).join(' ');
      for (const c of caps.split(' ')) this.ackedCaps.add(c);
      const canSasl = this.ackedCaps.has('sasl') && (this.sasl?.token || this.sasl?.signer);
      if (canSasl) {
        this.raw('AUTHENTICATE ATPROTO-CHALLENGE');
      } else {
        this.raw('CAP END');
      }
    } else if (sub === 'NAK') {
      this.raw('CAP END');
    }
  }

  private async handleAuthenticate(msg: IRCMessage): Promise<void> {
    const param = msg.params[0] || '';
    if (param === '+' || !param) return;

    // Decode the raw challenge bytes the server sent. Two parallel
    // uses:
    //   - PDS methods need only the nonce (echoed back so the server
    //     can bind the PDS verification to this specific challenge).
    //   - Crypto / did:key signs the raw challenge bytes themselves
    //     and puts the signature in the response.
    const padded = param.replace(/-/g, '+').replace(/_/g, '/');
    let rawChallengeBytes = new Uint8Array(0);
    let challengeNonce: string | undefined;
    try {
      const bin = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
      rawChallengeBytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) rawChallengeBytes[i] = bin.charCodeAt(i);
      const challenge = JSON.parse(new TextDecoder().decode(rawChallengeBytes));
      challengeNonce = challenge.nonce;
    } catch { /* proceed without nonce — pds-* path will still work for legacy servers */ }

    const method = this.sasl?.method || 'pds-session';

    // ── Crypto / did:key auth — sign the raw challenge bytes ──
    let signature = this.sasl?.token ?? '';
    if (method === 'crypto') {
      if (!this.sasl?.signer) {
        console.warn('[freeq-sdk] SASL method=crypto requires a signer callback in setSaslCredentials; aborting');
        this.raw('AUTHENTICATE *');
        return;
      }
      try {
        signature = await this.sasl.signer(rawChallengeBytes);
      } catch (e) {
        console.error('[freeq-sdk] Crypto SASL signer threw:', e);
        this.raw('AUTHENTICATE *');
        return;
      }
    }

    const response = JSON.stringify({
      did: this.sasl?.did,
      method,
      signature,
      pds_url: this.sasl?.pdsUrl,
      challenge_nonce: challengeNonce,
    });
    const encoded = btoa(response)
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=+$/, '');

    if (encoded.length <= 400) {
      this.raw(`AUTHENTICATE ${encoded}`);
    } else {
      for (let i = 0; i < encoded.length; i += 400) {
        this.raw(`AUTHENTICATE ${encoded.slice(i, i + 400)}`);
      }
      this.raw('AUTHENTICATE +');
    }
  }

  private handleAvSessionState(
    sessionId: string,
    action: string,
    channel: string,
    actorNick: string,
    _participantCount: number,
    title?: string,
  ): void {
    const existing = this._avSessions.get(sessionId);

    switch (action) {
      case 'started': {
        const session: AvSession = {
          id: sessionId,
          channel,
          createdBy: '',
          createdByNick: actorNick,
          title: title || undefined,
          participants: new Map([[actorNick, {
            did: '',
            nick: actorNick,
            role: 'host' as const,
            joinedAt: new Date(),
          }]]),
          state: 'active',
          startedAt: new Date(),
        };
        this._avSessions.set(sessionId, session);
        this.emit('avSessionUpdate', session);
        if (actorNick.toLowerCase() === this._nick.toLowerCase()) {
          this._activeAvSession = sessionId;
        }
        break;
      }
      case 'joined': {
        if (existing && existing.state === 'active') {
          const updated = { ...existing, participants: new Map(existing.participants) };
          updated.participants.set(actorNick, {
            did: '',
            nick: actorNick,
            role: 'speaker' as const,
            joinedAt: new Date(),
          });
          this._avSessions.set(sessionId, updated);
          this.emit('avSessionUpdate', updated);
          if (actorNick.toLowerCase() === this._nick.toLowerCase()) {
            this._activeAvSession = sessionId;
          }
        }
        break;
      }
      case 'left': {
        if (existing && existing.state === 'active') {
          const updated = { ...existing, participants: new Map(existing.participants) };
          updated.participants.delete(actorNick);
          this._avSessions.set(sessionId, updated);
          this.emit('avSessionUpdate', updated);
        }
        break;
      }
      case 'ended': {
        if (existing) {
          const ended = { ...existing, state: 'ended' as const, participants: new Map<string, AvParticipant>() };
          this._avSessions.set(sessionId, ended);
          this.emit('avSessionUpdate', ended);
          setTimeout(() => {
            this._avSessions.delete(sessionId);
            this.emit('avSessionRemoved', sessionId);
          }, 5000);
        }
        if (this._activeAvSession === sessionId) {
          this._activeAvSession = null;
        }
        break;
      }
    }
  }

  // ── Channels ──

  /** Send IRC QUIT. Closes the session cleanly on the server side. */
  quit(reason?: string): void {
    this.raw(reason ? `QUIT :${reason}` : 'QUIT');
  }

  /** JOIN multiple channels at once (comma-separated wire form). */
  joinMany(channels: string[]): void {
    if (channels.length === 0) return;
    this.raw(`JOIN ${channels.join(',')}`);
  }

  // ── Messaging extensions ──

  /** PRIVMSG with arbitrary IRCv3 tags. Caller-managed escaping is handled
   *  by the SDK's format() helper. */
  sendTagged(target: string, text: string, tags: Record<string, string>): void {
    this.raw(format('PRIVMSG', [target, text], tags));
  }

  /** TAGMSG (tags-only, no body) to a target. */
  sendTagmsg(target: string, tags: Record<string, string>): void {
    this.raw(format('TAGMSG', [target], tags));
  }

  /** Send a media attachment (image/audio/video URL with metadata).
   *  Server side stores the media tags; rich clients render the embed. */
  sendMedia(
    target: string,
    media: { url: string; mime?: string; alt?: string; width?: number; height?: number; durationMs?: number; sizeBytes?: number; fallback?: string },
  ): void {
    const tags: Record<string, string> = { '+freeq.at/media-url': media.url };
    if (media.mime) tags['+freeq.at/media-mime'] = media.mime;
    if (media.alt) tags['+freeq.at/media-alt'] = media.alt;
    if (media.width !== undefined) tags['+freeq.at/media-w'] = String(media.width);
    if (media.height !== undefined) tags['+freeq.at/media-h'] = String(media.height);
    if (media.durationMs !== undefined) tags['+freeq.at/media-duration'] = String(media.durationMs);
    if (media.sizeBytes !== undefined) tags['+freeq.at/media-size'] = String(media.sizeBytes);
    const body = media.fallback ?? `📎 ${media.url}`;
    this.raw(format('PRIVMSG', [target, body], tags));
  }

  /** Attach link-preview metadata to a message. */
  sendLinkPreview(
    target: string,
    preview: { url: string; title?: string; description?: string; imageUrl?: string },
  ): void {
    const tags: Record<string, string> = { '+freeq.at/link-url': preview.url };
    if (preview.title) tags['+freeq.at/link-title'] = preview.title;
    if (preview.description) tags['+freeq.at/link-desc'] = preview.description;
    if (preview.imageUrl) tags['+freeq.at/link-image'] = preview.imageUrl;
    const fallback = preview.title && preview.description
      ? `🔗 ${preview.title} — ${preview.description} (${preview.url})`
      : preview.title
        ? `🔗 ${preview.title} (${preview.url})`
        : `🔗 ${preview.url}`;
    this.raw(format('PRIVMSG', [target, fallback], tags));
  }

  /** Send a message and await the server-assigned msgid via echo-message.
   *  Resolves with the msgid the server stamps on the echo. Requires
   *  `echo-message` cap (negotiated by default). Timeouts after 5s. */
  sendAndAwaitEcho(target: string, text: string, tags: Record<string, string> = {}): Promise<string> {
    return new Promise<string>((resolve, reject) => {
      const nonce = `echo-${Date.now().toString(16)}${Math.floor(Math.random() * 0xffffffff).toString(16).padStart(8, '0')}`;
      const fullTags = { ...tags, '+freeq.at/echo-nonce': nonce };
      const timer = setTimeout(() => {
        this.off('raw', onRaw);
        reject(new Error('sendAndAwaitEcho timed out waiting for echo-message'));
      }, 5000);
      const onRaw = (_line: string, parsed: IRCMessage): void => {
        if (parsed.command !== 'PRIVMSG' && parsed.command !== 'TAGMSG') return;
        if (parsed.tags?.['+freeq.at/echo-nonce'] !== nonce) return;
        const msgid = parsed.tags?.['msgid'];
        if (!msgid) return;
        clearTimeout(timer);
        this.off('raw', onRaw);
        resolve(msgid);
      };
      this.on('raw', onRaw);
      this.raw(format('PRIVMSG', [target, text], fullTags));
    });
  }

  /** Send a threaded reply (alias for sendReply, named to match Rust SDK
   *  `reply_in_thread`). */
  sendReplyInThread(target: string, parentMsgId: string, text: string): void {
    this.sendReply(target, parentMsgId, text);
  }

  // ── Typing indicators ──

  /** Start a typing indicator in a target (channel or DM). */
  startTyping(target: string): void {
    this.raw(format('TAGMSG', [target], { '+typing': 'active' }));
  }

  /** Stop a typing indicator. */
  stopTyping(target: string): void {
    this.raw(format('TAGMSG', [target], { '+typing': 'done' }));
  }

  // ── Identity resolution (sync getters; cache is auto-populated) ──

  /** Sync lookup: nick → DID. Returns undefined if unknown.
   *  Auto-populated from WHOIS 330, JOIN account tags, and ACCOUNT notify. */
  getDidForNick(nick: string): string | undefined {
    return this._nickToDid.get(nick.toLowerCase()) ?? this.nickToDid?.(nick);
  }

  /** Sync lookup: DID → current nick. Returns undefined if unknown.
   *  Needed for AGENT PAUSE/REVOKE which take nicks, not DIDs. */
  getNickForDid(did: string): string | undefined {
    return this._didToNick.get(did);
  }

  // ── Agent lifecycle ──

  /** Declare actor_class for this session. Class is one of:
   *  'agent' | 'external_agent' | 'human'. Broadcast to shared channels. */
  registerAgent(actorClass: 'agent' | 'external_agent' | 'human'): void {
    this.raw(`AGENT REGISTER :class=${actorClass}`);
  }

  /** Submit a provenance declaration (JSON value, base64url-encoded on
   *  the wire). For agents, typically a FreeqBotDelegation/v1 cert. */
  submitProvenance(provenance: unknown): void {
    const json = JSON.stringify(provenance);
    const bytes = new TextEncoder().encode(json);
    // base64url, no padding.
    let b64 = btoa(String.fromCharCode(...bytes));
    b64 = b64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    this.raw(`PROVENANCE :${b64}`);
  }

  /** Update structured agent presence (state, status, task). */
  setPresence(state: string, status?: string, task?: string): void {
    const parts = [`state=${state}`];
    if (status) parts.push(`status=${status}`);
    if (task) parts.push(`task=${task}`);
    this.raw(`PRESENCE :${parts.join(';')}`);
  }

  /** Send a single heartbeat. */
  sendHeartbeat(state: string, ttlSeconds: number): void {
    this.raw(`HEARTBEAT :state=${state};ttl=${ttlSeconds}`);
  }

  /** Start a background heartbeat loop at the given interval (ms).
   *  TTL is set to 2× interval per Rust SDK convention. */
  startHeartbeat(intervalMs: number): HeartbeatHandle {
    if (this._agentHeartbeatTimer) clearInterval(this._agentHeartbeatTimer);
    const ttl = Math.max(1, Math.floor(intervalMs / 1000) * 2);
    // First beat immediately so server marks us alive without waiting.
    this.sendHeartbeat('active', ttl);
    this._agentHeartbeatTimer = setInterval(() => {
      try { this.sendHeartbeat('active', ttl); }
      catch { /* socket gone; next reconnect re-arms */ }
    }, intervalMs);
    return {
      stop: () => {
        if (this._agentHeartbeatTimer) {
          clearInterval(this._agentHeartbeatTimer);
          this._agentHeartbeatTimer = null;
        }
      },
    };
  }

  // ── Governance ──

  /** Request approval from channel ops for a capability use. */
  requestApproval(channel: string, capability: string, resource?: string): void {
    const tail = resource ? `${capability};resource=${resource}` : capability;
    this.raw(`APPROVAL_REQUEST ${channel} :${tail}`);
  }

  /** Op-only. Pause target agent — expects PRESENCE=paused within 10s. */
  pauseAgent(nick: string, reason?: string): void {
    this.raw(reason ? `AGENT PAUSE ${nick} :${reason}` : `AGENT PAUSE ${nick}`);
  }

  /** Op-only. Resume a paused agent. */
  resumeAgent(nick: string): void {
    this.raw(`AGENT RESUME ${nick}`);
  }

  /** Op-only. Revoke capabilities + force disconnect. */
  revokeAgent(nick: string, reason?: string): void {
    this.raw(reason ? `AGENT REVOKE ${nick} :${reason}` : `AGENT REVOKE ${nick}`);
  }

  /** Op approval response. */
  approveAgent(nick: string, capability: string): void {
    this.raw(`AGENT APPROVE ${nick} ${capability}`);
  }

  /** Op denial response. */
  denyAgent(nick: string, capability: string, reason?: string): void {
    this.raw(reason
      ? `AGENT DENY ${nick} ${capability} :${reason}`
      : `AGENT DENY ${nick} ${capability}`);
  }

  // ── Coordination events ──

  /** Emit a coordination event as paired TAGMSG (for storage) +
   *  companion PRIVMSG (for rich-client rendering). Returns the
   *  server-stored event ID. */
  emitEvent(
    channel: string,
    eventType: string,
    payload: unknown,
    opts: EmitEventOptions = {},
  ): string {
    const eventId = opts.eventId ?? mintEventId();
    const payloadJson = JSON.stringify(payload);
    // Percent-encode `;` and ` ` so the value survives both IRCv3 tag
    // escape and the server's url-decode pass (see proposal §5.0).
    const encoded = payloadJson.replace(/;/g, '%3B').replace(/ /g, '%20');
    const tags: Record<string, string> = {
      msgid: eventId,
      '+freeq.at/event': eventType,
      '+freeq.at/payload': encoded,
    };
    if (opts.refId) tags['+freeq.at/task-id'] = opts.refId;
    if (opts.extraTags) Object.assign(tags, opts.extraTags);
    const humanText = opts.humanText ?? `${eventType}`;
    this.raw(format('TAGMSG', [channel], tags));
    this.raw(format('PRIVMSG', [channel, humanText], tags));
    return eventId;
  }

  /** Sugar over `emitEvent` for `task_request`. Returns the task ID. */
  createTask(channel: string, description: string): string {
    return this.emitEvent(channel, 'task_request', { description }, {
      humanText: `📋 New task: ${description}`,
    });
  }

  /** Sugar for `task_update` — progress update on a task. */
  updateTask(channel: string, taskId: string, phase: string, summary: string): void {
    this.emitEvent(channel, 'task_update', { phase, summary }, {
      refId: taskId,
      humanText: `🔄 [${phase}] ${summary}`,
    });
  }

  /** Sugar for `task_complete`. */
  completeTask(channel: string, taskId: string, summary: string, url?: string): void {
    const payload: Record<string, unknown> = { summary };
    if (url) payload.url = url;
    const urlStr = url ? ` — ${url}` : '';
    this.emitEvent(channel, 'task_complete', payload, {
      refId: taskId,
      humanText: `🎉 Task complete: ${summary}${urlStr}`,
    });
  }

  /** Sugar for `task_failed`. */
  failTask(channel: string, taskId: string, error: string): void {
    this.emitEvent(channel, 'task_failed', { error }, {
      refId: taskId,
      humanText: `❌ Task failed: ${error}`,
    });
  }

  /** Sugar for `evidence_attach` — attach evidence to a task. */
  attachEvidence(
    channel: string,
    taskId: string,
    evidenceType: string,
    summary: string,
    url?: string,
  ): void {
    const payload: Record<string, unknown> = { type: evidenceType, summary };
    if (url) payload.url = url;
    const urlStr = url ? ` — ${url}` : '';
    this.emitEvent(channel, 'evidence_attach', payload, {
      refId: taskId,
      extraTags: { '+freeq.at/evidence-type': evidenceType },
      humanText: `📎 Evidence (${evidenceType}): ${summary}${urlStr}`,
    });
  }

  // ── Spawning (Phase 4) ──

  /** Submit an agent manifest (base64-encoded TOML). */
  submitManifest(tomlContent: string): void {
    const bytes = new TextEncoder().encode(tomlContent);
    const b64 = btoa(String.fromCharCode(...bytes));
    this.raw(`AGENT MANIFEST ${b64}`);
  }

  /** Spawn a child agent in a channel. */
  spawnAgent(
    channel: string,
    nick: string,
    capabilities: string[],
    ttlSeconds?: number,
    taskRef?: string,
  ): void {
    let params = `nick=${nick}`;
    if (capabilities.length > 0) params += `;capabilities=${capabilities.join(',')}`;
    if (ttlSeconds !== undefined) params += `;ttl=${ttlSeconds}`;
    if (taskRef) params += `;task=${taskRef}`;
    this.raw(`AGENT SPAWN ${channel} :${params}`);
  }

  /** Despawn a child agent (parent only). */
  despawnAgent(nick: string): void {
    this.raw(`AGENT DESPAWN ${nick}`);
  }

  /** Send a message attributed to a spawned child agent. */
  sendAsChild(childNick: string, channel: string, text: string): void {
    this.raw(`AGENT MSG ${childNick} ${channel} :${text}`);
  }

  // ── Economics (Phase 5) ──

  /** Submit a spend record for the current action.
   *  (Server emits a `budget_exceeded` governance TAGMSG to us if this
   *  spend pushes us past the per-agent budget cap.) */
  submitSpend(
    channel: string,
    amount: number,
    unit: string,
    description: string,
    taskRef?: string,
  ): void {
    let params = `amount=${amount.toFixed(6)};unit=${unit};desc=${description}`;
    if (taskRef) params += `;task=${taskRef}`;
    this.raw(`SPEND ${channel} :${params}`);
  }

  /** Set a per-agent budget on a channel (op only). */
  setBudget(
    channel: string,
    maxAmount: number,
    unit: string,
    period: string,
    sponsorDid: string,
  ): void {
    this.raw(`BUDGET ${channel} :max=${maxAmount};unit=${unit};period=${period};sponsor=${sponsorDid}`);
  }

  /** Query channel budget state (server replies with snapshot). */
  requestBudget(channel: string): void {
    this.raw(`BUDGET ${channel}`);
  }
}

/** Generate a coordination event ID. Format mirrors Rust SDK
 *  (millis-hex + 16 random hex chars). Not a ULID. */
function mintEventId(): string {
  const millis = Date.now().toString(16).padStart(13, '0');
  const r1 = Math.floor(Math.random() * 0xffffffff).toString(16).padStart(8, '0');
  const r2 = Math.floor(Math.random() * 0xffffffff).toString(16).padStart(8, '0');
  return millis + r1 + r2;
}
