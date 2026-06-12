//! End-to-end encryption key exchange tests.
//!
//! Tests the full X3DH → Double Ratchet pipeline: bundle generation,
//! initiate/respond handshake, session establishment, and encrypted
//! message exchange between two parties.

use freeq_sdk::e2ee_did::{DmKey, GroupKey};
use freeq_sdk::ratchet::Session;
use freeq_sdk::x3dh::{self, IdentityKeyPair, InitialMessage, PreKeyBundle, SignedPreKey};

// ═══════════════════════════════════════════════════════════════
// X3DH HANDSHAKE
// ═══════════════════════════════════════════════════════════════

#[test]
fn x3dh_full_handshake_produces_same_shared_secret() {
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let bob_verifying = bob_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);

    let bob_bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);

    // Alice initiates
    let result = x3dh::initiate(&alice_ik, "did:plc:alice", &bob_bundle, &bob_verifying).unwrap();

    // Bob responds
    let (bob_secret, _bob_ratchet_secret) =
        x3dh::respond(&bob_ik, &bob_spk, &result.initial_message).unwrap();

    // Both sides should derive the same shared secret
    assert_eq!(
        result.shared_secret, bob_secret,
        "X3DH shared secrets must match"
    );
}

#[test]
fn x3dh_tampered_spk_signature_rejected() {
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let bob_verifying = bob_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);

    let mut bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);
    // Tamper with signature
    bundle.spk_signature = "AAAA".to_string();

    let result = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying);
    assert!(result.is_err(), "Tampered SPK signature must be rejected");
}

#[test]
fn x3dh_wrong_verifying_key_rejected() {
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let wrong_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let wrong_verifying = wrong_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);

    let bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);
    let result = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &wrong_verifying);
    assert!(result.is_err(), "Wrong verifying key must be rejected");
}

#[test]
fn x3dh_different_sessions_different_secrets() {
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let bob_verifying = bob_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);
    let bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);

    let r1 = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying).unwrap();
    let r2 = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying).unwrap();
    // Different ephemeral keys → different shared secrets
    assert_ne!(
        r1.shared_secret, r2.shared_secret,
        "Each handshake uses fresh ephemeral"
    );
}

#[test]
fn x3dh_bundle_serialization_roundtrip() {
    let ik = IdentityKeyPair::generate();
    let signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let spk = SignedPreKey::generate(42, &signing);
    let bundle = PreKeyBundle::new("did:plc:test", &ik, &spk);

    let json = serde_json::to_string(&bundle).unwrap();
    let parsed: PreKeyBundle = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.did, "did:plc:test");
    assert_eq!(parsed.spk_id, 42);
    assert_eq!(parsed.identity_key, bundle.identity_key);
}

#[test]
fn x3dh_initial_message_serialization_roundtrip() {
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let bob_verifying = bob_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);
    let bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);

    let result = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying).unwrap();
    let json = serde_json::to_string(&result.initial_message).unwrap();
    let parsed: InitialMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.did, "did:plc:alice");
    assert_eq!(parsed.spk_id, 1);
}

// ═══════════════════════════════════════════════════════════════
// X3DH → RATCHET: full pipeline
// ═══════════════════════════════════════════════════════════════

#[test]
fn x3dh_to_ratchet_full_conversation() {
    // X3DH handshake
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let bob_verifying = bob_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);
    let bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);

    let alice_result = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying).unwrap();
    let (bob_secret, bob_ratchet_secret) =
        x3dh::respond(&bob_ik, &bob_spk, &alice_result.initial_message).unwrap();

    // Initialize ratchet sessions
    let mut alice_session =
        Session::init_alice(alice_result.shared_secret, alice_result.their_ratchet_key);
    let mut bob_session = Session::init_bob(bob_secret, bob_ratchet_secret);

    // Alice → Bob
    let ct1 = alice_session
        .encrypt("hello bob, this is encrypted")
        .unwrap();
    assert!(ct1.starts_with("ENC3:"));
    let pt1 = bob_session.decrypt(&ct1).unwrap();
    assert_eq!(pt1, "hello bob, this is encrypted");

    // Bob → Alice
    let ct2 = bob_session.encrypt("hi alice, got your message").unwrap();
    let pt2 = alice_session.decrypt(&ct2).unwrap();
    assert_eq!(pt2, "hi alice, got your message");

    // Multi-round
    for i in 0..10 {
        let c = alice_session.encrypt(&format!("msg {i}")).unwrap();
        assert_eq!(bob_session.decrypt(&c).unwrap(), format!("msg {i}"));
        let c = bob_session.encrypt(&format!("reply {i}")).unwrap();
        assert_eq!(alice_session.decrypt(&c).unwrap(), format!("reply {i}"));
    }
}

#[test]
fn x3dh_ratchet_replay_rejected() {
    let alice_ik = IdentityKeyPair::generate();
    let bob_ik = IdentityKeyPair::generate();
    let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let bob_verifying = bob_signing.verifying_key();
    let bob_spk = SignedPreKey::generate(1, &bob_signing);
    let bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);

    let ar = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying).unwrap();
    let (bs, brs) = x3dh::respond(&bob_ik, &bob_spk, &ar.initial_message).unwrap();

    let mut a = Session::init_alice(ar.shared_secret, ar.their_ratchet_key);
    let mut b = Session::init_bob(bs, brs);

    let ct = a.encrypt("once").unwrap();
    b.decrypt(&ct).unwrap();
    assert!(b.decrypt(&ct).is_err(), "Replay must be rejected");
}

#[test]
fn x3dh_ratchet_wrong_session_fails() {
    // Two independent X3DH handshakes → two independent sessions
    let make_session = || {
        let alice_ik = IdentityKeyPair::generate();
        let bob_ik = IdentityKeyPair::generate();
        let bob_signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let bob_verifying = bob_signing.verifying_key();
        let bob_spk = SignedPreKey::generate(1, &bob_signing);
        let bundle = PreKeyBundle::new("did:plc:bob", &bob_ik, &bob_spk);
        let ar = x3dh::initiate(&alice_ik, "did:plc:alice", &bundle, &bob_verifying).unwrap();
        let (bs, brs) = x3dh::respond(&bob_ik, &bob_spk, &ar.initial_message).unwrap();
        (
            Session::init_alice(ar.shared_secret, ar.their_ratchet_key),
            Session::init_bob(bs, brs),
        )
    };

    let (mut a1, _b1) = make_session();
    let (_a2, mut b2) = make_session();

    let ct = a1.encrypt("wrong session").unwrap();
    assert!(
        b2.decrypt(&ct).is_err(),
        "Cross-session decryption must fail"
    );
}

// ═══════════════════════════════════════════════════════════════
// GROUP KEY (ENC2) — CHANNEL ENCRYPTION
// ═══════════════════════════════════════════════════════════════

#[test]
fn group_key_members_order_independent() {
    let k1 = GroupKey::derive("#test", &["did:a".into(), "did:b".into()], 1);
    let k2 = GroupKey::derive("#test", &["did:b".into(), "did:a".into()], 1);
    let ct = k1.encrypt("secret").unwrap();
    assert!(
        k2.decrypt(&ct).is_ok(),
        "Key must be same regardless of member order"
    );
}

#[test]
fn group_key_different_channel_different_key() {
    let k1 = GroupKey::derive("#chan1", &["did:a".into()], 1);
    let k2 = GroupKey::derive("#chan2", &["did:a".into()], 1);
    let ct = k1.encrypt("test").unwrap();
    assert!(
        k2.decrypt(&ct).is_err(),
        "Different channels must produce different keys"
    );
}

#[test]
fn group_key_epoch_mismatch_rejected() {
    let k1 = GroupKey::derive("#test", &["did:a".into()], 1);
    let k2 = GroupKey::derive("#test", &["did:a".into()], 2);
    let ct = k1.encrypt("old epoch").unwrap();
    assert!(k2.decrypt(&ct).is_err(), "Epoch mismatch must be rejected");
}

#[test]
fn group_key_non_member_cannot_decrypt() {
    let k1 = GroupKey::derive("#test", &["did:a".into(), "did:b".into()], 1);
    let k2 = GroupKey::derive("#test", &["did:a".into(), "did:c".into()], 1);
    let ct = k1.encrypt("members only").unwrap();
    assert!(k2.decrypt(&ct).is_err(), "Non-member must not decrypt");
}

#[test]
fn group_key_channel_case_insensitive() {
    let k1 = GroupKey::derive("#TEST", &["did:a".into()], 1);
    let k2 = GroupKey::derive("#test", &["did:a".into()], 1);
    let ct = k1.encrypt("case test").unwrap();
    assert_eq!(k2.decrypt(&ct).unwrap(), "case test");
}

// ═══════════════════════════════════════════════════════════════
// DM KEY (ENC2:dm:) — ECDH-BASED DM ENCRYPTION
// ═══════════════════════════════════════════════════════════════

#[test]
fn dm_key_bidirectional() {
    use k256::ecdsa::SigningKey;
    let alice_sk = SigningKey::random(&mut rand::rngs::OsRng);
    let bob_sk = SigningKey::random(&mut rand::rngs::OsRng);
    let alice_pk = alice_sk.verifying_key().to_sec1_bytes();
    let bob_pk = bob_sk.verifying_key().to_sec1_bytes();

    let k_ab =
        DmKey::from_secp256k1("did:a", "did:b", &alice_sk.to_bytes().into(), &bob_pk).unwrap();
    let k_ba =
        DmKey::from_secp256k1("did:b", "did:a", &bob_sk.to_bytes().into(), &alice_pk).unwrap();

    let ct = k_ab.encrypt("hello dm").unwrap();
    assert_eq!(k_ba.decrypt(&ct).unwrap(), "hello dm");
}

#[test]
fn dm_key_third_party_cannot_decrypt() {
    use k256::ecdsa::SigningKey;
    let alice_sk = SigningKey::random(&mut rand::rngs::OsRng);
    let bob_sk = SigningKey::random(&mut rand::rngs::OsRng);
    let eve_sk = SigningKey::random(&mut rand::rngs::OsRng);
    let bob_pk = bob_sk.verifying_key().to_sec1_bytes();

    let k_ab =
        DmKey::from_secp256k1("did:a", "did:b", &alice_sk.to_bytes().into(), &bob_pk).unwrap();
    let k_eb = DmKey::from_secp256k1("did:e", "did:b", &eve_sk.to_bytes().into(), &bob_pk).unwrap();

    let ct = k_ab.encrypt("private").unwrap();
    assert!(
        k_eb.decrypt(&ct).is_err(),
        "Eve must not decrypt Alice↔Bob DM"
    );
}
