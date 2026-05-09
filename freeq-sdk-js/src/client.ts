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

  /** Send a WHOIS query. */
  whois(nick: string): void {
    this.raw(`WHOIS ${nick}`);
  }

  /** Request chat history for a channel. */
  requestHistory(channel: string, before?: string): void {
    if (before) {
      this.raw(`CHATHISTORY BEFORE ${channel} timestamp=${before} 50`);
    } else {
      this.raw(`CHATHISTORY LATEST ${channel} * 50`);
    }
  }

  /** Request DM conversation targets. */
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

  /** Fetch pinned messages for a channel via REST API. */
  async fetchPins(channel: string): Promise<void> {
    try {
      const name = channel.startsWith('#') ? channel.slice(1) : channel;
      const resp = await fetch(`${this.serverOrigin}/api/v1/channels/${encodeURIComponent(name)}/pins`);
      if (resp.ok) {
        const data = await resp.json();
        this.emit('pins', channel, data.pins || []);
      }
    } catch { /* ignore */ }
  }

  // ── Internals ──

  private onTransportStateChange(state: TransportState): void {
    this._connectionState = state;
    this.emit('connectionStateChanged', state);

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
    // Ask listeners to resolve nick→DID. The app layer provides this via member lists.
    // For now, iterate background whois cache — the app can also provide a resolver.
    return undefined; // Will be augmented by the app layer
  }

  /** Resolve nick to DID — set by the app layer for E2EE support. */
  nickToDid: ((nick: string) => string | undefined) | null = null;

  private resolveNickToDid(targetNick: string): string | undefined {
    return this.nickToDid?.(targetNick);
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

      case '433':
        this._nick += '_';
        this.raw(`NICK ${this._nick}`);
        break;

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
        if (joinDid) prefetchProfiles([joinDid]);
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

      case 'AWAY':
        this.emit('userAway', from, msg.params[0] || null);
        break;

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
        this.emit('whois', whoisNick, {
          user: msg.params[2],
          host: msg.params[3],
          realname: msg.params[5] || msg.params[4],
          did: undefined,
          handle: undefined,
        });
        if (!this.backgroundWhois.has(whoisNick.toLowerCase())) {
          this.emit('systemMessage', 'server', `WHOIS ${whoisNick}: ${msg.params[2]}@${msg.params[3]} (${msg.params[5] || msg.params[4]})`);
        }
        break;
      }
      case '312': {
        const whoisNick = msg.params[1] || '';
        this.emit('whois', whoisNick, { server: msg.params[2] });
        if (!this.backgroundWhois.has(whoisNick.toLowerCase())) {
          this.emit('systemMessage', 'server', `  Server: ${msg.params[2]}`);
        }
        break;
      }
      case '318':
        this.backgroundWhois.delete((msg.params[1] || '').toLowerCase());
        break;
      case '319': {
        const whoisNick = msg.params[1] || '';
        this.emit('whois', whoisNick, { channels: msg.params[2] });
        if (!this.backgroundWhois.has(whoisNick.toLowerCase())) {
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
        this.emit('whois', whoisNick, { handle: msg.params[2]?.trim() });
        if (!this.backgroundWhois.has(whoisNick.toLowerCase())) {
          this.emit('systemMessage', 'server', `  Handle: ${msg.params[2]?.trim()}`);
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
}
