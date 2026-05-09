/** Core types for the freeq SDK. */

/** Parsed IRC message with optional IRCv3 tags. */
export interface IRCMessage {
  tags: Record<string, string>;
  prefix: string;
  command: string;
  params: string[];
}

/** A chat message. */
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
  isStreaming?: boolean;
  deleted?: boolean;
  reactions?: Map<string, Set<string>>;
  encrypted?: boolean;
}

/** A channel or DM member. */
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
  actorClass?: 'human' | 'agent' | 'external_agent';
}

/** A pinned message reference. */
export interface PinnedMessage {
  msgid: string;
  pinned_by: string;
  pinned_at: number;
}

/** A channel with members and messages. */
export interface Channel {
  name: string;
  topic: string;
  topicSetBy?: string;
  members: Map<string, Member>;
  messages: Message[];
  modes: Set<string>;
  isEncrypted: boolean;
  unreadCount: number;
  mentionCount: number;
  lastReadMsgId?: string;
  isJoined: boolean;
  pins: PinnedMessage[];
}

/** WHOIS information for a user. */
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

/** An entry in the server's channel list. */
export interface ChannelListEntry {
  name: string;
  topic: string;
  count: number;
}

/** AV session participant. */
export interface AvParticipant {
  did: string;
  nick: string;
  role: 'host' | 'speaker' | 'listener';
  joinedAt: Date;
}

/** AV (audio/video) session. */
export interface AvSession {
  id: string;
  channel: string | null;
  createdBy: string;
  createdByNick: string;
  title?: string;
  participants: Map<string, AvParticipant>;
  state: 'active' | 'ended';
  startedAt: Date;
  irohTicket?: string;
}

/** WebSocket transport state. */
export type TransportState = 'disconnected' | 'connecting' | 'connected';

/** SASL credentials for AT Protocol authentication.
 *
 * Two flavours:
 *   - Token-based (`method: "pds-session"` or `"pds-oauth"`): a PDS-
 *     issued bearer goes in `token`; the server checks it with the PDS.
 *   - Crypto / did:key (`method: "crypto"`): no token — instead, the
 *     SDK signs the challenge bytes via the `signer` callback. The
 *     server resolves `did` and verifies the signature against the
 *     advertised public key. No PDS, no OAuth, no external service.
 *     See `generateDidKey()` in `./did-key.ts`.
 */
export interface SaslCredentials {
  /** Bearer token for token-based methods. Empty when method=crypto. */
  token: string;
  did: string;
  pdsUrl: string;
  method: string;
  /** Required when `method === "crypto"`. Called on the raw challenge
   *  bytes the server emits in AUTHENTICATE. Returns base64url(sig). */
  signer?: (challengeBytes: Uint8Array) => Promise<string>;
}

/** Options for creating a FreeqClient. */
export interface FreeqClientOptions {
  /** WebSocket URL (e.g. "wss://irc.freeq.at/irc"). */
  url: string;

  /** Desired IRC nickname. */
  nick: string;

  /** Channels to auto-join on connect. */
  channels?: string[];

  /** SASL credentials for AT Protocol authentication. */
  sasl?: SaslCredentials;

  /**
   * Base URL for the auth broker (for session refresh).
   * If set along with `brokerToken`, the client refreshes
   * the web-token on each reconnect.
   */
  brokerUrl?: string;

  /** Long-lived broker token for session refresh. */
  brokerToken?: string;

  /** Server origin for API calls (e.g. E2EE key upload). Defaults to url origin. */
  serverOrigin?: string;

  /** Skip the first broker token refresh (use when token is already fresh). */
  skipInitialBrokerRefresh?: boolean;

  /** When `false`, the SDK does NOT auto-mint a session ed25519 key and
   *  send MSGSIG after SASL success. Defaults to true for backward
   *  compatibility. Useful for agents that hold their own signing key
   *  (e.g. freeqcc using its did:key seed) or for headless tests. */
  autoMsgSig?: boolean;
}

/** A batch of messages (e.g. CHATHISTORY response). */
export interface Batch {
  type: string;
  target: string;
  messages: Message[];
}
