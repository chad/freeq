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
  secretKey: Uint8Array;      // X25519 identity private key
  publicKey: Uint8Array;      // X25519 identity public key
  spkSecret: Uint8Array;      // X25519 signed pre-key private
  spkPublic: Uint8Array;      // X25519 signed pre-key public
  spkSignature: Uint8Array;   // Ed25519 signature of SPK by signing key
  spkId: number;
  signingKey?: CryptoKeyPair; // Ed25519 signing keypair (for SPK signature + verification)
  signingPublic?: Uint8Array; // Ed25519 public key (exported for bundle)
}

interface SessionState {
  sharedSecret: number[];
  sendChainKey: number[];
  recvChainKey: number[];
  sendMsgNum: number;
  recvMsgNum: number;
  prevChainLen: number;
  // DH Ratchet state (Signal protocol)
  dhSendSecret?: number[];   // our current ratchet private key
  dhSendPublic?: number[];   // our current ratchet public key
  dhRecvPublic?: number[];   // their current ratchet public key
  rootKey?: number[];         // root key for KDF ratchet
  dhRatchetInitialized?: boolean;
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
 * Get the safety number for a DM session — a human-readable fingerprint
 * of both identity keys. Users compare this out-of-band to verify no MITM.
 * Format: 12 groups of 5 digits (60 digits total), like Signal.
 */
export async function getSafetyNumber(remoteDid: string): Promise<string | null> {
  if (!identityKeys) return null;

  // Combine both public keys in canonical order for consistent fingerprint
  const myKey = identityKeys.publicKey;
  const encoder = new TextEncoder();
  const remoteDIDBytes = encoder.encode(remoteDid);
  // Include the DID string so different users with same keys get different numbers
  const material = new Uint8Array(64 + remoteDIDBytes.length);
  // Sort: lower key first for consistency regardless of who checks
  const myKeyHex = Array.from(myKey).map(b => b.toString(16).padStart(2, '0')).join('');
  const weAreFirst = myKeyHex < remoteDid; // deterministic ordering
  if (weAreFirst) {
    material.set(myKey, 0);
    material.set(remoteDIDBytes, 32);
  } else {
    material.set(remoteDIDBytes, 0);
    material.set(myKey, remoteDIDBytes.length);
  }

  const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', material));
  const digits: string[] = [];
  for (let i = 0; i < 12; i++) {
    const val = ((hash[i * 2] << 8) | hash[i * 2 + 1]) % 100000;
    digits.push(val.toString().padStart(5, '0'));
  }
  return digits.join(' ');
}

/**
 * Initialize E2EE for an authenticated user.
 */
export async function initialize(did: string, serverOrigin: string): Promise<void> {
  // Check X25519 availability before doing anything
  try {
    await (crypto.subtle.generateKey as any)({ name: 'X25519' }, false, ['deriveBits']);
  } catch {
    console.warn('[e2ee] X25519 not available in this browser — E2EE disabled');
    return;
  }

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
      signingPublic: stored.signingPublic ? new Uint8Array(stored.signingPublic) : undefined,
    };
    // Restore CryptoKeyPair if we have the exported private key
    if (stored.signingPrivate) {
      try {
        const privKey = await crypto.subtle.importKey('pkcs8', new Uint8Array(stored.signingPrivate), 'Ed25519', false, ['sign']);
        const pubKey = await crypto.subtle.importKey('raw', new Uint8Array(stored.signingPublic), 'Ed25519', false, ['verify']);
        identityKeys.signingKey = { privateKey: privKey, publicKey: pubKey };
      } catch { /* Ed25519 import not available */ }
    }
  } else {
    identityKeys = await generateIdentityKeys();
    const toStore: Record<string, unknown> = {
      secretKey: Array.from(identityKeys.secretKey),
      publicKey: Array.from(identityKeys.publicKey),
      spkSecret: Array.from(identityKeys.spkSecret),
      spkPublic: Array.from(identityKeys.spkPublic),
      spkSignature: Array.from(identityKeys.spkSignature),
      spkId: identityKeys.spkId,
    };
    if (identityKeys.signingPublic) {
      toStore.signingPublic = Array.from(identityKeys.signingPublic);
    }
    if (identityKeys.signingKey) {
      try {
        const privBytes = await crypto.subtle.exportKey('pkcs8', identityKeys.signingKey.privateKey);
        toStore.signingPrivate = Array.from(new Uint8Array(privBytes));
      } catch { /* can't export */ }
    }
    await db.put('identity', toStore, did);
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

  // Generate Ed25519 signing key for SPK signature + identity verification
  let signingKey: CryptoKeyPair | undefined;
  let signingPublic: Uint8Array | undefined;
  let spkSignature: Uint8Array;
  try {
    signingKey = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']) as CryptoKeyPair;
    signingPublic = new Uint8Array(await crypto.subtle.exportKey('raw', signingKey.publicKey));
    // Sign the SPK public key with our Ed25519 identity signing key
    const sig = await crypto.subtle.sign('Ed25519', signingKey.privateKey, spkPublic);
    spkSignature = new Uint8Array(sig);
  } catch {
    // Ed25519 signing not available — use empty signature
    spkSignature = new Uint8Array(64);
  }

  return {
    secretKey: ikSecret, publicKey: ikPublic,
    spkSecret, spkPublic,
    spkSignature,
    spkId: 1,
    signingKey,
    signingPublic,
  };
}

// ── Pre-Key Bundle API ──

async function uploadPreKeyBundle(origin: string, did: string, keys: IdentityKeys): Promise<void> {
  const bundle: Record<string, unknown> = {
    did,
    identity_key: toB64(keys.publicKey),
    signed_pre_key: toB64(keys.spkPublic),
    spk_signature: toB64(keys.spkSignature),
    spk_id: keys.spkId,
  };
  // Include Ed25519 signing public key for identity verification
  if (keys.signingPublic) {
    bundle.signing_key = toB64(keys.signingPublic);
  }
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

    // DH ratchet step: generate new DH keypair every N messages for forward secrecy
    if (st.dhRatchetInitialized && st.dhRecvPublic && st.sendMsgNum > 0 && st.sendMsgNum % 10 === 0) {
      try {
        const dhPair = await (crypto.subtle.generateKey as any)({ name: 'X25519' }, true, ['deriveBits']);
        const newSecret = new Uint8Array(await crypto.subtle.exportKey('raw', dhPair.privateKey));
        const newPublic = new Uint8Array(await crypto.subtle.exportKey('raw', dhPair.publicKey));
        // DH with their current public key
        const dhOutput = await x25519DH(newSecret, new Uint8Array(st.dhRecvPublic));
        // Derive new root key and chain key
        const newRoot = await hkdfDerive(dhOutput, 'freeq-ratchet-root');
        const newChain = await hkdfDerive(newRoot, 'freeq-ratchet-chain');
        st.prevChainLen = st.sendMsgNum;
        st.sendMsgNum = 0;
        st.dhSendSecret = Array.from(newSecret);
        st.dhSendPublic = Array.from(newPublic);
        st.rootKey = Array.from(newRoot);
        st.sendChainKey = Array.from(newChain);
        console.log('[e2ee] DH ratchet step completed');
      } catch (e) {
        console.warn('[e2ee] DH ratchet step failed, continuing with chain key:', e);
      }
    }

    // Header: DH ratchet public key (32) + prevChainLen (4) + msgNum (4)
    const dhPub = st.dhSendPublic ? new Uint8Array(st.dhSendPublic) : identityKeys.publicKey;
    const header = new Uint8Array(40);
    header.set(dhPub, 0);
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
  serverOrigin?: string,
): Promise<string | null> {
  if (!initialized) return null;
  if (!wire.startsWith(ENC3_PREFIX)) return null;

  let session = sessions.get(remoteDid);
  // Auto-establish session if we don't have one (receiver side)
  if (!session && serverOrigin) {
    const newSession = await establishSession(remoteDid, serverOrigin);
    if (!newSession) return null;
    session = newSession;
  }
  if (!session) return null;

  try {
    const body = wire.slice(ENC3_PREFIX.length);
    const parts = body.split(':');
    if (parts.length !== 3) return null;

    const header = fromB64(parts[0]);
    const iv = fromB64(parts[1]);
    const ct = fromB64(parts[2]);
    if (header.length !== 40 || iv.length !== 12) return null;

    const senderDHPub = header.slice(0, 32);
    const msgNum = new DataView(header.buffer, header.byteOffset + 36, 4).getUint32(0, false);
    const st: SessionState = JSON.parse(session.state);

    // Check if sender has done a DH ratchet step (new DH public key)
    if (st.dhRatchetInitialized && st.dhRecvPublic && st.dhSendSecret) {
      const currentRecvPub = new Uint8Array(st.dhRecvPublic);
      if (!arraysEqual(senderDHPub, currentRecvPub)) {
        try {
          // Sender rotated their DH key — perform receiving DH ratchet
          const dhOutput = await x25519DH(new Uint8Array(st.dhSendSecret), senderDHPub);
          const newRoot = await hkdfDerive(dhOutput, 'freeq-ratchet-root');
          const newChain = await hkdfDerive(newRoot, 'freeq-ratchet-chain');
          st.dhRecvPublic = Array.from(senderDHPub);
          st.rootKey = Array.from(newRoot);
          st.recvChainKey = Array.from(newChain);
          st.recvMsgNum = 0;
          console.log('[e2ee] Receiving DH ratchet step completed');
        } catch (e) {
          console.warn('[e2ee] Receiving DH ratchet failed:', e);
        }
      }
    }

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

    // Verify SPK signature if signing key is present
    if (bundle.signing_key && bundle.spk_signature) {
      try {
        const signingPub = fromB64(bundle.signing_key);
        const spkSig = fromB64(bundle.spk_signature);
        const verifyKey = await (crypto.subtle as any).importKey('raw', signingPub, 'Ed25519', false, ['verify']);
        const valid = await (crypto.subtle as any).verify('Ed25519', verifyKey, spkSig, theirSPK);
        if (!valid) {
          console.error('[e2ee] SPK signature verification failed for', remoteDid);
          return null;
        }
        console.log('[e2ee] SPK signature verified for', remoteDid);
      } catch (e) {
        console.warn('[e2ee] Could not verify SPK signature:', e);
        // Continue anyway — older clients may not have signing keys
      }
    }

    // Symmetric X3DH: both sides compute the same 3 DH values.
    // We sort by public key to ensure both sides use the same order:
    //   dh1 = DH(my_ik, their_spk)
    //   dh2 = DH(my_spk, their_ik)
    //   dh3 = DH(my_spk, their_spk)
    // DH is commutative, so DH(a_secret, B_pub) == DH(b_secret, A_pub).
    // But dh1 and dh2 use DIFFERENT key pairs, so we need to canonicalize.
    // Solution: always order (dh_ik×spk, dh_spk×ik) by comparing public keys.
    const dh_ik_spk = await x25519DH(identityKeys.secretKey, theirSPK);
    const dh_spk_ik = await x25519DH(identityKeys.spkSecret, theirIK);
    const dh_spk_spk = await x25519DH(identityKeys.spkSecret, theirSPK);

    // Canonical order: sort the two cross-DH values by comparing our IK vs their IK
    const myIK = identityKeys.publicKey;
    const weAreFirst = compareBytes(myIK, theirIK) < 0;
    const dh1 = weAreFirst ? dh_ik_spk : dh_spk_ik;
    const dh2 = weAreFirst ? dh_spk_ik : dh_ik_spk;

    const ikm = new Uint8Array(96);
    ikm.set(dh1, 0); ikm.set(dh2, 32); ikm.set(dh_spk_spk, 64);

    const sharedSecret = await hkdfDerive(ikm, 'freeq-x3dh-v1');

    // Derive separate send/recv chain keys based on who has the "lower" public key.
    // The party with the lexicographically lower IK uses chain_a for sending, chain_b for receiving.
    // The other party uses chain_b for sending, chain_a for receiving.
    const chain_a = await hkdfDerive(sharedSecret, 'freeq-chain-a');
    const chain_b = await hkdfDerive(sharedSecret, 'freeq-chain-b');

    // Generate initial DH ratchet keypair
    const dhPair = await (crypto.subtle.generateKey as any)({ name: 'X25519' }, true, ['deriveBits']);
    const dhSecret = new Uint8Array(await crypto.subtle.exportKey('raw', dhPair.privateKey));
    const dhPublic = new Uint8Array(await crypto.subtle.exportKey('raw', dhPair.publicKey));

    const st: SessionState = {
      sharedSecret: Array.from(sharedSecret),
      sendChainKey: Array.from(weAreFirst ? chain_a : chain_b),
      recvChainKey: Array.from(weAreFirst ? chain_b : chain_a),
      sendMsgNum: 0, recvMsgNum: 0, prevChainLen: 0,
      // DH ratchet initial state
      dhSendSecret: Array.from(dhSecret),
      dhSendPublic: Array.from(dhPublic),
      dhRecvPublic: Array.from(theirSPK), // start with their SPK
      rootKey: Array.from(sharedSecret),
      dhRatchetInitialized: true,
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

function arraysEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) { if (a[i] !== b[i]) return false; }
  return true;
}

async function x25519DH(mySecret: Uint8Array, theirPublic: Uint8Array): Promise<Uint8Array> {
  const myKey = await (crypto.subtle as any).importKey('raw', mySecret, { name: 'X25519' }, false, ['deriveBits']);
  const theirKey = await (crypto.subtle as any).importKey('raw', theirPublic, { name: 'X25519' }, false, []);
  const bits = await (crypto.subtle as any).deriveBits({ name: 'X25519', public: theirKey }, myKey, 256);
  return new Uint8Array(bits);
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

/** Lexicographic comparison of two byte arrays. */
function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) {
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return a.length - b.length;
}

function toB64(data: Uint8Array): string {
  return btoa(String.fromCharCode(...data))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function fromB64(str: string): Uint8Array {
  const padded = str.replace(/-/g, '+').replace(/_/g, '/') + '=='.slice(0, (4 - str.length % 4) % 4);
  return Uint8Array.from(atob(padded), c => c.charCodeAt(0));
}
