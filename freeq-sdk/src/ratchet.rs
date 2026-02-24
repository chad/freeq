//! Double Ratchet protocol for forward-secret encrypted DMs.
//!
//! Implements the Signal Double Ratchet algorithm using:
//! - X25519 for Diffie-Hellman ratchet
//! - HMAC-SHA256 for KDF chains (root, sending, receiving)
//! - AES-256-GCM for message encryption
//! - HKDF-SHA256 for key derivation
//!
//! Reference: <https://signal.org/docs/specifications/doubleratchet/>
//!
//! # Wire Format
//!
//! ```text
//! ENC3:<header-b64url>:<nonce-b64url>:<ciphertext-b64url>
//! ```
//!
//! Header (MessagePack or fixed-format):
//! - sender ratchet public key (32 bytes)
//! - previous chain length (u32)
//! - message number (u32)
//!
//! The header is included as AAD (additional authenticated data) in the
//! AES-GCM encryption, so it can't be tampered with.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, AeadCore, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use std::collections::HashMap;

/// Wire prefix for Double Ratchet encrypted messages.
pub const ENC3_PREFIX: &str = "ENC3:";

/// Maximum number of skipped message keys to store per session.
/// Prevents memory exhaustion from malicious counter inflation.
const MAX_SKIP: u32 = 1000;

// ── KDF Functions ──────────────────────────────────────────────────

/// KDF for the root chain. Takes the current root key and a DH output,
/// produces a new root key and a chain key.
fn kdf_root(root_key: &[u8; 32], dh_out: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(root_key), dh_out);
    let mut output = [0u8; 64];
    hk.expand(b"freeq-ratchet-v1", &mut output)
        .expect("64 bytes valid for HKDF");
    let mut new_root = [0u8; 32];
    let mut chain_key = [0u8; 32];
    new_root.copy_from_slice(&output[..32]);
    chain_key.copy_from_slice(&output[32..]);
    (new_root, chain_key)
}

/// KDF for the symmetric chain. Advances the chain key and produces
/// a message key.
fn kdf_chain(chain_key: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    use hmac::digest::KeyInit;
    use hmac::Mac;
    type HmacSha256 = hmac::Hmac<Sha256>;

    // Message key = HMAC(chain_key, 0x01)
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(chain_key).unwrap();
    Mac::update(&mut mac, &[0x01]);
    let msg_key: [u8; 32] = mac.finalize().into_bytes().into();

    // Next chain key = HMAC(chain_key, 0x02)
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(chain_key).unwrap();
    Mac::update(&mut mac, &[0x02]);
    let next_chain: [u8; 32] = mac.finalize().into_bytes().into();

    (next_chain, msg_key)
}

/// X25519 Diffie-Hellman.
fn dh(secret: &StaticSecret, public: &PublicKey) -> [u8; 32] {
    secret.diffie_hellman(public).to_bytes()
}

// ── Message Header ─────────────────────────────────────────────────

/// Header sent with each encrypted message (unencrypted but authenticated).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Header {
    /// Sender's current ratchet public key (32 bytes, base64url).
    pub ratchet_key: [u8; 32],
    /// Number of messages in the previous sending chain.
    pub prev_chain_len: u32,
    /// Message number in the current sending chain.
    pub msg_num: u32,
}

impl Header {
    /// Encode header to bytes (fixed 40-byte format).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(40);
        out.extend_from_slice(&self.ratchet_key);
        out.extend_from_slice(&self.prev_chain_len.to_be_bytes());
        out.extend_from_slice(&self.msg_num.to_be_bytes());
        out
    }

    /// Decode header from bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, RatchetError> {
        if data.len() != 40 {
            return Err(RatchetError::MalformedHeader);
        }
        let mut ratchet_key = [0u8; 32];
        ratchet_key.copy_from_slice(&data[..32]);
        let prev_chain_len = u32::from_be_bytes(data[32..36].try_into().unwrap());
        let msg_num = u32::from_be_bytes(data[36..40].try_into().unwrap());
        Ok(Self { ratchet_key, prev_chain_len, msg_num })
    }
}

// ── Session State ──────────────────────────────────────────────────

/// A Double Ratchet session between two parties.
///
/// Serializable so it can be persisted between app restarts.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// Our current DH ratchet keypair (secret is 32 bytes).
    dh_self_secret: [u8; 32],
    dh_self_public: [u8; 32],

    /// Their current DH ratchet public key.
    dh_remote: Option<[u8; 32]>,

    /// Root key.
    root_key: [u8; 32],

    /// Sending chain key.
    send_chain_key: Option<[u8; 32]>,
    /// Number of messages sent in current sending chain.
    send_msg_num: u32,

    /// Receiving chain key.
    recv_chain_key: Option<[u8; 32]>,
    /// Number of messages received in current receiving chain.
    recv_msg_num: u32,

    /// Previous sending chain length (for header).
    prev_send_chain_len: u32,

    /// Skipped message keys: (ratchet_public_key, msg_num) → message_key.
    /// For handling out-of-order messages.
    skipped: HashMap<([u8; 32], u32), [u8; 32]>,

    /// Whether we sent the first message (determines ratchet direction).
    is_initiator: bool,
}

impl Session {
    /// Initialize a session as the initiator (Alice).
    ///
    /// `shared_secret` comes from X3DH.
    /// `their_ratchet_key` is Bob's signed pre-key (used as initial ratchet key).
    pub fn init_alice(shared_secret: [u8; 32], their_ratchet_key: [u8; 32]) -> Self {
        let our_secret = StaticSecret::random_from_rng(OsRng);
        let our_public = PublicKey::from(&our_secret);

        // Perform initial DH ratchet step
        let their_pk = PublicKey::from(their_ratchet_key);
        let dh_out = dh(&our_secret, &their_pk);
        let (root_key, send_chain_key) = kdf_root(&shared_secret, &dh_out);

        Session {
            dh_self_secret: our_secret.to_bytes(),
            dh_self_public: our_public.to_bytes(),
            dh_remote: Some(their_ratchet_key),
            root_key,
            send_chain_key: Some(send_chain_key),
            send_msg_num: 0,
            recv_chain_key: None,
            recv_msg_num: 0,
            prev_send_chain_len: 0,
            skipped: HashMap::new(),
            is_initiator: true,
        }
    }

    /// Initialize a session as the responder (Bob).
    ///
    /// `shared_secret` comes from X3DH.
    /// `our_ratchet_keypair` is our signed pre-key (used as initial ratchet key).
    pub fn init_bob(shared_secret: [u8; 32], our_ratchet_secret: [u8; 32]) -> Self {
        let our_public = PublicKey::from(
            &StaticSecret::from(our_ratchet_secret)
        ).to_bytes();

        Session {
            dh_self_secret: our_ratchet_secret,
            dh_self_public: our_public,
            dh_remote: None,
            root_key: shared_secret,
            send_chain_key: None,
            send_msg_num: 0,
            recv_chain_key: None,
            recv_msg_num: 0,
            prev_send_chain_len: 0,
            skipped: HashMap::new(),
            is_initiator: false,
        }
    }

    /// Encrypt a plaintext message.
    ///
    /// Returns the wire-format string: `ENC3:<header>:<nonce>:<ciphertext>`
    pub fn encrypt(&mut self, plaintext: &str) -> Result<String, RatchetError> {
        // Ensure we have a sending chain
        if self.send_chain_key.is_none() {
            return Err(RatchetError::NoSendChain);
        }

        // Advance the sending chain
        let chain_key = self.send_chain_key.unwrap();
        let (next_chain, msg_key) = kdf_chain(&chain_key);
        self.send_chain_key = Some(next_chain);

        let header = Header {
            ratchet_key: self.dh_self_public,
            prev_chain_len: self.prev_send_chain_len,
            msg_num: self.send_msg_num,
        };
        self.send_msg_num += 1;

        // Encrypt with AES-256-GCM, using header as AAD
        let cipher = Aes256Gcm::new_from_slice(&msg_key)
            .map_err(|_| RatchetError::CryptoError)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let header_bytes = header.to_bytes();
        let payload = aes_gcm::aead::Payload {
            msg: plaintext.as_bytes(),
            aad: &header_bytes,
        };
        let ciphertext = cipher.encrypt(&nonce, payload)
            .map_err(|_| RatchetError::CryptoError)?;

        // Wire format
        let header_b64 = B64.encode(&header_bytes);
        let nonce_b64 = B64.encode(&nonce[..]);
        let ct_b64 = B64.encode(&ciphertext);

        Ok(format!("{ENC3_PREFIX}{header_b64}:{nonce_b64}:{ct_b64}"))
    }

    /// Decrypt a wire-format encrypted message.
    pub fn decrypt(&mut self, wire: &str) -> Result<String, RatchetError> {
        let body = wire.strip_prefix(ENC3_PREFIX)
            .ok_or(RatchetError::NotEncrypted)?;

        let parts: Vec<&str> = body.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(RatchetError::MalformedMessage);
        }

        let header_bytes = B64.decode(parts[0])
            .map_err(|_| RatchetError::MalformedMessage)?;
        let nonce_bytes = B64.decode(parts[1])
            .map_err(|_| RatchetError::MalformedMessage)?;
        let ct_bytes = B64.decode(parts[2])
            .map_err(|_| RatchetError::MalformedMessage)?;

        if nonce_bytes.len() != 12 {
            return Err(RatchetError::MalformedMessage);
        }

        let header = Header::from_bytes(&header_bytes)?;

        // Try skipped message keys first (out-of-order delivery)
        if let Some(msg_key) = self.skipped.remove(&(header.ratchet_key, header.msg_num)) {
            return decrypt_with_key(&msg_key, &header_bytes, &nonce_bytes, &ct_bytes);
        }

        // If the sender's ratchet key changed, perform a DH ratchet step
        let their_key_changed = self.dh_remote
            .map(|k| k != header.ratchet_key)
            .unwrap_or(true);

        if their_key_changed {
            // Skip any remaining messages in the current receiving chain
            if let Some(recv_ck) = self.recv_chain_key {
                self.skip_messages(
                    self.dh_remote.unwrap_or([0u8; 32]),
                    recv_ck,
                    self.recv_msg_num,
                    header.prev_chain_len,
                )?;
            }

            // DH ratchet step
            self.dh_remote = Some(header.ratchet_key);
            let their_pk = PublicKey::from(header.ratchet_key);
            let our_sk = StaticSecret::from(self.dh_self_secret);
            let dh_out = dh(&our_sk, &their_pk);

            let (root_key, recv_chain_key) = kdf_root(&self.root_key, &dh_out);
            self.root_key = root_key;
            self.recv_chain_key = Some(recv_chain_key);
            self.recv_msg_num = 0;

            // Generate new DH keypair for our next sending chain
            self.prev_send_chain_len = self.send_msg_num;
            self.send_msg_num = 0;
            let new_secret = StaticSecret::random_from_rng(OsRng);
            let new_public = PublicKey::from(&new_secret);
            self.dh_self_secret = new_secret.to_bytes();
            self.dh_self_public = new_public.to_bytes();

            // New sending chain
            let dh_out = dh(
                &StaticSecret::from(self.dh_self_secret),
                &their_pk,
            );
            let (root_key, send_chain_key) = kdf_root(&self.root_key, &dh_out);
            self.root_key = root_key;
            self.send_chain_key = Some(send_chain_key);
        }

        // Skip messages in the current receiving chain up to msg_num
        let recv_ck = self.recv_chain_key.ok_or(RatchetError::NoReceiveChain)?;
        self.skip_messages(
            header.ratchet_key,
            recv_ck,
            self.recv_msg_num,
            header.msg_num,
        )?;

        // Advance the receiving chain to get the message key
        let chain_key = self.recv_chain_key.unwrap();
        let (next_chain, msg_key) = kdf_chain(&chain_key);
        self.recv_chain_key = Some(next_chain);
        self.recv_msg_num = header.msg_num + 1;

        decrypt_with_key(&msg_key, &header_bytes, &nonce_bytes, &ct_bytes)
    }

    /// Skip messages in a chain, storing their keys for later decryption.
    fn skip_messages(
        &mut self,
        ratchet_key: [u8; 32],
        mut chain_key: [u8; 32],
        from: u32,
        until: u32,
    ) -> Result<(), RatchetError> {
        if until < from {
            return Ok(());
        }
        if until - from > MAX_SKIP {
            return Err(RatchetError::TooManySkipped);
        }
        for n in from..until {
            let (next_chain, msg_key) = kdf_chain(&chain_key);
            self.skipped.insert((ratchet_key, n), msg_key);
            chain_key = next_chain;
        }
        // Update the chain key to point past the skipped messages
        self.recv_chain_key = Some(chain_key);
        Ok(())
    }

    /// Serialize session state for persistence.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("Session is serializable")
    }

    /// Deserialize session state.
    pub fn from_bytes(data: &[u8]) -> Result<Self, RatchetError> {
        serde_json::from_slice(data).map_err(|_| RatchetError::InvalidSession)
    }

    /// Get our current ratchet public key (for including in key bundles).
    pub fn our_public_key(&self) -> [u8; 32] {
        self.dh_self_public
    }
}

/// Decrypt a message with a specific message key.
fn decrypt_with_key(
    msg_key: &[u8; 32],
    header_bytes: &[u8],
    nonce_bytes: &[u8],
    ct_bytes: &[u8],
) -> Result<String, RatchetError> {
    let cipher = Aes256Gcm::new_from_slice(msg_key)
        .map_err(|_| RatchetError::CryptoError)?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let payload = aes_gcm::aead::Payload {
        msg: ct_bytes,
        aad: header_bytes,
    };
    let plaintext = cipher.decrypt(nonce, payload)
        .map_err(|_| RatchetError::DecryptFailed)?;
    String::from_utf8(plaintext).map_err(|_| RatchetError::InvalidUtf8)
}

/// Check if a message is Double Ratchet encrypted.
pub fn is_encrypted(text: &str) -> bool {
    text.starts_with(ENC3_PREFIX)
}

// ── Errors ─────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum RatchetError {
    #[error("not an ENC3 encrypted message")]
    NotEncrypted,
    #[error("malformed encrypted message")]
    MalformedMessage,
    #[error("malformed header")]
    MalformedHeader,
    #[error("no sending chain (session not fully initialized)")]
    NoSendChain,
    #[error("no receiving chain")]
    NoReceiveChain,
    #[error("too many skipped messages")]
    TooManySkipped,
    #[error("decryption failed (wrong key or tampered)")]
    DecryptFailed,
    #[error("crypto error")]
    CryptoError,
    #[error("invalid UTF-8")]
    InvalidUtf8,
    #[error("invalid session data")]
    InvalidSession,
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sessions() -> (Session, Session) {
        // Simulate X3DH: both sides agree on a shared secret
        let shared_secret = [42u8; 32];

        // Bob's initial ratchet keypair (his signed pre-key)
        let bob_ratchet_secret = StaticSecret::random_from_rng(OsRng);
        let bob_ratchet_public = PublicKey::from(&bob_ratchet_secret).to_bytes();

        let alice = Session::init_alice(shared_secret, bob_ratchet_public);
        let bob = Session::init_bob(shared_secret, bob_ratchet_secret.to_bytes());

        (alice, bob)
    }

    #[test]
    fn basic_roundtrip() {
        let (mut alice, mut bob) = make_sessions();

        // Alice sends to Bob
        let wire = alice.encrypt("Hello Bob!").unwrap();
        assert!(is_encrypted(&wire));
        let pt = bob.decrypt(&wire).unwrap();
        assert_eq!(pt, "Hello Bob!");
    }

    #[test]
    fn bidirectional() {
        let (mut alice, mut bob) = make_sessions();

        // Alice → Bob
        let w1 = alice.encrypt("Hi Bob").unwrap();
        assert_eq!(bob.decrypt(&w1).unwrap(), "Hi Bob");

        // Bob → Alice
        let w2 = bob.encrypt("Hi Alice").unwrap();
        assert_eq!(alice.decrypt(&w2).unwrap(), "Hi Alice");

        // Alice → Bob again (new ratchet step)
        let w3 = alice.encrypt("Second message").unwrap();
        assert_eq!(bob.decrypt(&w3).unwrap(), "Second message");
    }

    #[test]
    fn many_messages_one_direction() {
        let (mut alice, mut bob) = make_sessions();

        for i in 0..100 {
            let msg = format!("Message {i}");
            let wire = alice.encrypt(&msg).unwrap();
            let pt = bob.decrypt(&wire).unwrap();
            assert_eq!(pt, msg);
        }
    }

    #[test]
    fn out_of_order() {
        let (mut alice, mut bob) = make_sessions();

        // Alice sends 3 messages
        let w1 = alice.encrypt("msg 1").unwrap();
        let w2 = alice.encrypt("msg 2").unwrap();
        let w3 = alice.encrypt("msg 3").unwrap();

        // Bob receives them out of order
        assert_eq!(bob.decrypt(&w3).unwrap(), "msg 3");
        assert_eq!(bob.decrypt(&w1).unwrap(), "msg 1");
        assert_eq!(bob.decrypt(&w2).unwrap(), "msg 2");
    }

    #[test]
    fn forward_secrecy() {
        let (mut alice, mut bob) = make_sessions();

        // Exchange several rounds to advance the ratchet
        let w1 = alice.encrypt("msg 1").unwrap();
        bob.decrypt(&w1).unwrap();
        let w2 = bob.encrypt("reply 1").unwrap();
        alice.decrypt(&w2).unwrap();
        let w3 = alice.encrypt("msg 2").unwrap();
        bob.decrypt(&w3).unwrap();
        let w4 = bob.encrypt("reply 2").unwrap();
        alice.decrypt(&w4).unwrap();

        // Save Alice's state at this point
        let alice_state = alice.to_bytes();

        // Continue conversation — multiple DH ratchet steps forward
        let w5 = alice.encrypt("msg 3").unwrap();
        bob.decrypt(&w5).unwrap();
        let w6 = bob.encrypt("reply 3").unwrap();
        alice.decrypt(&w6).unwrap();
        let w7 = alice.encrypt("msg 4").unwrap();
        bob.decrypt(&w7).unwrap();

        // Now Bob sends a message using keys from AFTER the ratchet advanced
        let w8 = bob.encrypt("future msg after ratchet").unwrap();

        // Old Alice (from before the ratchet steps) can't decrypt w8
        // because Bob's ratchet key has changed and old Alice doesn't
        // have the chain keys derived from the new DH ratchet steps
        let mut old_alice = Session::from_bytes(&alice_state).unwrap();
        assert!(old_alice.decrypt(&w8).is_err(),
            "Old session state should not decrypt messages from advanced ratchet");
    }

    #[test]
    fn replay_rejected() {
        let (mut alice, mut bob) = make_sessions();

        let wire = alice.encrypt("test").unwrap();
        assert_eq!(bob.decrypt(&wire).unwrap(), "test");

        // Replaying the same message fails (key was consumed)
        assert!(bob.decrypt(&wire).is_err());
    }

    #[test]
    fn wrong_session_fails() {
        let (mut alice, _bob) = make_sessions();
        let (_, mut bob2) = make_sessions();

        let wire = alice.encrypt("hello").unwrap();
        // Different Bob can't decrypt
        assert!(bob2.decrypt(&wire).is_err());
    }

    #[test]
    fn session_serialization() {
        let (mut alice, mut bob) = make_sessions();

        let w1 = alice.encrypt("before persist").unwrap();
        assert_eq!(bob.decrypt(&w1).unwrap(), "before persist");

        // Serialize and restore both sessions
        let alice_bytes = alice.to_bytes();
        let bob_bytes = bob.to_bytes();
        let mut alice2 = Session::from_bytes(&alice_bytes).unwrap();
        let mut bob2 = Session::from_bytes(&bob_bytes).unwrap();

        // Continue conversation
        let w2 = bob2.encrypt("after persist").unwrap();
        assert_eq!(alice2.decrypt(&w2).unwrap(), "after persist");
    }

    #[test]
    fn unicode_and_emoji() {
        let (mut alice, mut bob) = make_sessions();

        let msg = "こんにちは 🔐 мир العالم";
        let wire = alice.encrypt(msg).unwrap();
        assert_eq!(bob.decrypt(&wire).unwrap(), msg);
    }

    #[test]
    fn empty_message() {
        let (mut alice, mut bob) = make_sessions();

        let wire = alice.encrypt("").unwrap();
        assert_eq!(bob.decrypt(&wire).unwrap(), "");
    }

    #[test]
    fn alternating_conversation() {
        let (mut alice, mut bob) = make_sessions();

        for i in 0..20 {
            if i % 2 == 0 {
                let w = alice.encrypt(&format!("A:{i}")).unwrap();
                assert_eq!(bob.decrypt(&w).unwrap(), format!("A:{i}"));
            } else {
                let w = bob.encrypt(&format!("B:{i}")).unwrap();
                assert_eq!(alice.decrypt(&w).unwrap(), format!("B:{i}"));
            }
        }
    }
}
