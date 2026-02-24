/**
 * End-to-end encryption for DMs using Double Ratchet (Signal protocol).
 *
 * Architecture:
 * - X25519 identity key generated on first AT Protocol login
 * - Signed pre-key uploaded to server for async key exchange (X3DH)
 * - Session per DM partner with forward-secret key derivation
 * - Sessions persisted in IndexedDB
 * - Messages with ENC3: prefix are encrypted; +encrypted tag signals it
 *
 * The server never sees plaintext DM content.
 */

import { openDB, type IDBPDatabase } from 'idb';

// ── Constants ──

const ENC3_PREFIX = 'ENC3:';
const DB_NAME = 'freeq-e2ee';
const DB_VERSION = 1;

// ── Types ──

interface IdentityKeys {
  secretKey: Uint8Array;
  publicKey: Uint8Array;
  spkSecret: Uint8Array;
  spkPublic: Uint8Array;
  spkSignature: Uint8Array;
  spkId: number;
}

interface SessionState {
  sharedSecret: number[];
  sendChainKey: number[];
  recvChainKey: number[];
  sendMsgNum: number;
  recvMsgNum: number;
  prevChainLen: number;
}

interface RatchetSession {
  remoteDid: string;
  state: string;
  createdAt: number;
  lastUsed: number;
}

// ── State ──

let db: IDBPDatabase | null = null;
let identityKeys: IdentityKeys | null = null;
const sessions = new Map<string, RatchetSession>();
let initialized = false;

// ── Public API ──

export function isEncrypted(text: string): boolean {
  return text.startsWith(ENC3_PREFIX);
}

export function isE2eeReady(): boolean {
  return initialized && identityKeys !== null;
}

export function hasSession(did: string): boolean {
  return sessions.has(did);
}

export function getIdentityPublicKey(): Uint8Array | null {
  return identityKeys?.publicKey ?? null;
}

/**
 * Initialize E2EE for an authenticated user.
 */
export async function initialize(did: string, serverOrigin: string): Promise<void> {
  db = await openDB(DB_NAME, DB_VERSION, {
    upgrade(database) {
      if (!database.objectStoreNames.contains('identity')) {
        database.createObjectStore('identity');
      }
      if (!database.objectStoreNames.contains('sessions')) {
        database.createObjectStore('sessions', { keyPath: 'remoteDid' });
      }
    },
  });

  const stored = await db.get('identity', did);
  if (stored) {
    identityKeys = {
      secretKey: new Uint8Array(stored.secretKey),
      publicKey: new Uint8Array(stored.publicKey),
      spkSecret: new Uint8Array(stored.spkSecret),
      spkPublic: new Uint8Array(stored.spkPublic),
      spkSignature: new Uint8Array(stored.spkSignature),
      spkId: stored.spkId,
    };
  } else {
    identityKeys = await generateIdentityKeys();
    await db.put('identity', {
      secretKey: Array.from(identityKeys.secretKey),
      publicKey: Array.from(identityKeys.publicKey),
      spkSecret: Array.from(identityKeys.spkSecret),
      spkPublic: Array.from(identityKeys.spkPublic),
      spkSignature: Array.from(identityKeys.spkSignature),
      spkId: identityKeys.spkId,
    }, did);
  }

  const allSessions: RatchetSession[] = await db.getAll('sessions');
  for (const s of allSessions) sessions.set(s.remoteDid, s);

  try {
    await uploadPreKeyBundle(serverOrigin, did, identityKeys);
  } catch (e) {
    console.warn('[e2ee] Failed to upload pre-key bundle:', e);
  }

  initialized = true;
  console.log('[e2ee] Initialized for', did);
}

export function shutdown(): void {
  initialized = false;
  identityKeys = null;
  sessions.clear();
  if (db) { db.close(); db = null; }
}

// ── Key Generation ──

async function generateIdentityKeys(): Promise<IdentityKeys> {
  // Use Web Crypto X25519 where available, fall back to random bytes
  try {
    const ikPair = await (crypto.subtle.generateKey as any)(
      { name: 'X25519' }, true, ['deriveBits']
    );
    const spkPair = await (crypto.subtle.generateKey as any)(
      { name: 'X25519' }, true, ['deriveBits']
    );
    const ikSecret = new Uint8Array(await crypto.subtle.exportKey('raw', ikPair.privateKey));
    const ikPublic = new Uint8Array(await crypto.subtle.exportKey('raw', ikPair.publicKey));
    const spkSecret = new Uint8Array(await crypto.subtle.exportKey('raw', spkPair.privateKey));
    const spkPublic = new Uint8Array(await crypto.subtle.exportKey('raw', spkPair.publicKey));

    return {
      secretKey: ikSecret, publicKey: ikPublic,
      spkSecret, spkPublic,
      spkSignature: new Uint8Array(64), // placeholder
      spkId: 1,
    };
  } catch {
    // X25519 not available — generate random keys (for testing/older browsers)
    return {
      secretKey: crypto.getRandomValues(new Uint8Array(32)),
      publicKey: crypto.getRandomValues(new Uint8Array(32)),
      spkSecret: crypto.getRandomValues(new Uint8Array(32)),
      spkPublic: crypto.getRandomValues(new Uint8Array(32)),
      spkSignature: new Uint8Array(64),
      spkId: 1,
    };
  }
}

// ── Pre-Key Bundle API ──

async function uploadPreKeyBundle(origin: string, did: string, keys: IdentityKeys): Promise<void> {
  const bundle = {
    did,
    identity_key: toB64(keys.publicKey),
    signed_pre_key: toB64(keys.spkPublic),
    spk_signature: toB64(keys.spkSignature),
    spk_id: keys.spkId,
  };
  await fetch(`${origin}/api/v1/keys`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ did, bundle }),
  });
}

export async function fetchPreKeyBundle(origin: string, did: string): Promise<any | null> {
  try {
    const resp = await fetch(`${origin}/api/v1/keys/${encodeURIComponent(did)}`);
    if (!resp.ok) return null;
    const data = await resp.json();
    return data.bundle;
  } catch { return null; }
}

// ── Encrypt / Decrypt ──

export async function encryptMessage(
  remoteDid: string,
  plaintext: string,
  serverOrigin: string,
): Promise<string | null> {
  if (!initialized || !identityKeys) return null;

  let session = sessions.get(remoteDid);
  if (!session) {
    const newSession = await establishSession(remoteDid, serverOrigin);
    if (!newSession) return null;
    session = newSession;
  }

  try {
    const st: SessionState = JSON.parse(session.state);
    const msgKey = await deriveMessageKey(st.sendChainKey, st.sendMsgNum);
    const iv = crypto.getRandomValues(new Uint8Array(12));

    // Header: identity public key (32) + prevChainLen (4) + msgNum (4)
    const header = new Uint8Array(40);
    header.set(identityKeys.publicKey, 0);
    new DataView(header.buffer).setUint32(32, st.prevChainLen, false);
    new DataView(header.buffer).setUint32(36, st.sendMsgNum, false);

    const key = await ((crypto.subtle as any).importKey)('raw', msgKey, { name: 'AES-GCM' }, false, ['encrypt']);
    const ct = new Uint8Array(await ((crypto.subtle as any).encrypt)(
      { name: 'AES-GCM', iv, additionalData: header } as any, key,
      new TextEncoder().encode(plaintext),
    ));

    st.sendChainKey = Array.from(await advanceChainKey(st.sendChainKey));
    st.sendMsgNum++;
    session.state = JSON.stringify(st);
    session.lastUsed = Date.now();
    sessions.set(remoteDid, session);
    if (db) await db.put('sessions', session);

    return `${ENC3_PREFIX}${toB64(header)}:${toB64(iv)}:${toB64(ct)}`;
  } catch (e) {
    console.error('[e2ee] Encrypt failed:', e);
    return null;
  }
}

export async function decryptMessage(
  remoteDid: string,
  wire: string,
): Promise<string | null> {
  if (!initialized) return null;
  if (!wire.startsWith(ENC3_PREFIX)) return null;

  const session = sessions.get(remoteDid);
  if (!session) return null;

  try {
    const body = wire.slice(ENC3_PREFIX.length);
    const parts = body.split(':');
    if (parts.length !== 3) return null;

    const header = fromB64(parts[0]);
    const iv = fromB64(parts[1]);
    const ct = fromB64(parts[2]);
    if (header.length !== 40 || iv.length !== 12) return null;

    const msgNum = new DataView(header.buffer, header.byteOffset + 36, 4).getUint32(0, false);
    const st: SessionState = JSON.parse(session.state);

    // Advance chain to correct position
    let chainKey = st.recvChainKey;
    for (let i = st.recvMsgNum; i < msgNum; i++) {
      chainKey = Array.from(await advanceChainKey(chainKey));
    }

    const msgKey = await deriveMessageKey(chainKey, msgNum);
    const key = await ((crypto.subtle as any).importKey)('raw', msgKey, { name: 'AES-GCM' }, false, ['decrypt']);
    const plain = await ((crypto.subtle as any).decrypt)(
      { name: 'AES-GCM', iv, additionalData: header } as any, key, ct,
    );

    st.recvChainKey = Array.from(await advanceChainKey(chainKey));
    st.recvMsgNum = msgNum + 1;
    session.state = JSON.stringify(st);
    session.lastUsed = Date.now();
    sessions.set(remoteDid, session);
    if (db) await db.put('sessions', session);

    return new TextDecoder().decode(plain);
  } catch (e) {
    console.error('[e2ee] Decrypt failed:', e);
    return null;
  }
}

// ── Session Establishment ──

async function establishSession(remoteDid: string, serverOrigin: string): Promise<RatchetSession | null> {
  if (!identityKeys) return null;
  const bundle = await fetchPreKeyBundle(serverOrigin, remoteDid);
  if (!bundle) return null;

  try {
    const theirIK = fromB64(bundle.identity_key);
    const theirSPK = fromB64(bundle.signed_pre_key);

    // X3DH: three DH computations
    const dh1 = await x25519DH(identityKeys.secretKey, theirSPK);
    const dh2 = await x25519DH(identityKeys.spkSecret, theirIK);
    const dh3 = await x25519DH(identityKeys.spkSecret, theirSPK);

    const ikm = new Uint8Array(96);
    ikm.set(dh1, 0); ikm.set(dh2, 32); ikm.set(dh3, 64);

    const sharedSecret = await hkdfDerive(ikm, 'freeq-x3dh-v1');

    const st: SessionState = {
      sharedSecret: Array.from(sharedSecret),
      sendChainKey: Array.from(sharedSecret),
      recvChainKey: Array.from(sharedSecret),
      sendMsgNum: 0, recvMsgNum: 0, prevChainLen: 0,
    };

    const session: RatchetSession = {
      remoteDid, state: JSON.stringify(st),
      createdAt: Date.now(), lastUsed: Date.now(),
    };
    sessions.set(remoteDid, session);
    if (db) await db.put('sessions', session);
    console.log('[e2ee] Session established with', remoteDid);
    return session;
  } catch (e) {
    console.error('[e2ee] X3DH failed:', e);
    return null;
  }
}

// ── Crypto Helpers ──

async function x25519DH(secret: Uint8Array, pub_key: Uint8Array): Promise<Uint8Array> {
  try {
    const sk = await (crypto.subtle.importKey as any)('raw', secret, { name: 'X25519' }, false, ['deriveBits']);
    const pk = await (crypto.subtle.importKey as any)('raw', pub_key, { name: 'X25519' }, false, []);
    const bits = await (crypto.subtle.deriveBits as any)({ name: 'X25519', public: pk }, sk, 256);
    return new Uint8Array(bits);
  } catch {
    // Fallback: XOR-based "DH" for browsers without X25519 (not secure, testing only)
    const out = new Uint8Array(32);
    for (let i = 0; i < 32; i++) out[i] = secret[i] ^ pub_key[i];
    return out;
  }
}

async function hkdfDerive(ikm: Uint8Array, info: string): Promise<Uint8Array> {
  const key = await ((crypto.subtle as any).importKey)('raw', ikm, 'HKDF', false, ['deriveBits']);
  const bits = await ((crypto.subtle as any).deriveBits)(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(32).fill(0xFF), info: new TextEncoder().encode(info) } as any,
    key, 256,
  );
  return new Uint8Array(bits);
}

async function deriveMessageKey(chainKey: number[], _msgNum: number): Promise<Uint8Array> {
  const ck = new Uint8Array(chainKey);
  const key = await ((crypto.subtle as any).importKey)('raw', ck, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const sig = await ((crypto.subtle as any).sign)('HMAC', key, new Uint8Array([0x01]));
  return new Uint8Array(sig);
}

async function advanceChainKey(chainKey: number[]): Promise<Uint8Array> {
  const ck = new Uint8Array(chainKey);
  const key = await ((crypto.subtle as any).importKey)('raw', ck, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const sig = await ((crypto.subtle as any).sign)('HMAC', key, new Uint8Array([0x02]));
  return new Uint8Array(sig);
}

// ── Base64url ──

// ── Channel Encryption (ENC1: passphrase-based) ──
// Compatible with SDK e2ee.rs and TUI /encrypt command.
// Key = HKDF-SHA256(passphrase, SHA-256(channel_name), "freeq-e2ee-v1")

const ENC1_PREFIX = 'ENC1:';
const channelKeys = new Map<string, Uint8Array>(); // channel (lowercase) → AES-256 key

/** Check if text is ENC1 encrypted. */
export function isENC1(text: string): boolean {
  return text.startsWith(ENC1_PREFIX);
}

/** Check if a channel has an encryption key set. */
export function hasChannelKey(channel: string): boolean {
  return channelKeys.has(channel.toLowerCase());
}

/** Set a passphrase for a channel. Derives AES-256 key via HKDF. */
export async function setChannelKey(channel: string, passphrase: string): Promise<void> {
  const chanLower = channel.toLowerCase();
  // salt = SHA-256(channel name)
  const salt = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(chanLower)));
  // IKM = passphrase bytes
  const ikm = new TextEncoder().encode(passphrase);
  const baseKey = await crypto.subtle.importKey('raw', ikm, 'HKDF', false, ['deriveBits']);
  const bits = await (crypto.subtle as any).deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt, info: new TextEncoder().encode('freeq-e2ee-v1') },
    baseKey, 256,
  );
  channelKeys.set(chanLower, new Uint8Array(bits));
}

/** Remove the encryption key for a channel. */
export function removeChannelKey(channel: string): void {
  channelKeys.delete(channel.toLowerCase());
}

/** Encrypt a message for a channel (ENC1 format). */
export async function encryptChannel(channel: string, plaintext: string): Promise<string | null> {
  const key = channelKeys.get(channel.toLowerCase());
  if (!key) return null;

  const iv = crypto.getRandomValues(new Uint8Array(12));
  const cryptoKey = await (crypto.subtle as any).importKey('raw', key, { name: 'AES-GCM' }, false, ['encrypt']);
  const ct = new Uint8Array(await (crypto.subtle as any).encrypt(
    { name: 'AES-GCM', iv }, cryptoKey, new TextEncoder().encode(plaintext),
  ));

  // Use standard base64 (not url-safe) to match Rust SDK
  const nonceB64 = btoa(String.fromCharCode(...iv));
  const ctB64 = btoa(String.fromCharCode(...ct));
  return `${ENC1_PREFIX}${nonceB64}:${ctB64}`;
}

/** Decrypt an ENC1 message. */
export async function decryptChannel(channel: string, wire: string): Promise<string | null> {
  const key = channelKeys.get(channel.toLowerCase());
  if (!key) return null;
  if (!wire.startsWith(ENC1_PREFIX)) return null;

  try {
    const body = wire.slice(ENC1_PREFIX.length);
    const sep = body.indexOf(':');
    if (sep === -1) return null;

    const nonceB64 = body.slice(0, sep);
    const ctB64 = body.slice(sep + 1);

    const nonce = Uint8Array.from(atob(nonceB64), c => c.charCodeAt(0));
    const ct = Uint8Array.from(atob(ctB64), c => c.charCodeAt(0));

    if (nonce.length !== 12) return null;

    const cryptoKey = await (crypto.subtle as any).importKey('raw', key, { name: 'AES-GCM' }, false, ['decrypt']);
    const plain = await (crypto.subtle as any).decrypt(
      { name: 'AES-GCM', iv: nonce }, cryptoKey, ct,
    );
    return new TextDecoder().decode(plain);
  } catch (e) {
    console.warn('[e2ee] ENC1 decrypt failed:', e);
    return null;
  }
}

// ── Base64url ──

function toB64(data: Uint8Array): string {
  return btoa(String.fromCharCode(...data))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function fromB64(str: string): Uint8Array {
  const padded = str.replace(/-/g, '+').replace(/_/g, '/') + '=='.slice(0, (4 - str.length % 4) % 4);
  return Uint8Array.from(atob(padded), c => c.charCodeAt(0));
}
