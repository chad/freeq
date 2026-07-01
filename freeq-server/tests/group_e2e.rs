//! End-to-end test for VC-bootstrapped E2E channels (EG1/EGK1).
//!
//! Exercises the whole server-blind key-distribution cycle against the REAL
//! database layer (`db.save_group_key` / `db.get_group_keys_for_member`) glued
//! to the REAL client crypto (`freeq_sdk::e2ee_group`):
//!
//!   steward seals → server stores opaque blobs → member fetches → opens →
//!   decrypts channel traffic → membership change → rotate + re-seal →
//!   departed member is locked out of the new epoch.
//!
//! This is the proof that the server never holds a group key it can open, and
//! that offboarding actually revokes read access to future messages.

use freeq_sdk::e2ee_group::{GroupState, open_best};
use freeq_server::db::Db;
use x25519_dalek::{PublicKey, StaticSecret};

/// A channel member: a DID plus a long-lived X25519 identity key. In production
/// the public half lives in the member's pre-key bundle.
struct Member {
    did: String,
    secret: StaticSecret,
    public: [u8; 32],
}

impl Member {
    fn new(did: &str) -> Self {
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&secret).to_bytes();
        Self {
            did: did.into(),
            secret,
            public,
        }
    }
}

/// Steward pushes a sealed batch into the server exactly as the
/// `POST /api/v1/channels/{c}/groupkeys` handler does.
fn server_store(db: &Db, channel: &str, state: &GroupState, members: &[&Member]) {
    let pairs: Vec<(String, [u8; 32])> =
        members.iter().map(|m| (m.did.clone(), m.public)).collect();
    for (did, wire) in state.seal_batch(&pairs) {
        db.save_group_key(channel, &did, state.epoch as i64, &wire)
            .unwrap();
    }
}

/// Member fetches their sealed keys exactly as `GET .../groupkeys` returns them.
fn server_fetch(db: &Db, channel: &str, m: &Member) -> Vec<(u64, String)> {
    db.get_group_keys_for_member(channel, &m.did)
        .unwrap()
        .into_iter()
        .map(|(epoch, wire)| (epoch as u64, wire))
        .collect()
}

#[test]
fn full_lifecycle_join_read_rotate_revoke() {
    let db = Db::open_memory().unwrap();
    let channel = "#eng";

    let alice = Member::new("did:plc:alice"); // founder / steward
    let bob = Member::new("did:plc:bob");

    // ── Epoch 1: steward admits Alice + Bob and seals the group key ──────────
    let e1 = GroupState::create(channel);
    server_store(&db, channel, &e1, &[&alice, &bob]);

    // Steward posts an encrypted message; server stores only ciphertext.
    let msg1 = e1.encrypt("Q3 board deck is in the drive").unwrap();
    assert!(msg1.starts_with("EG1:1:"));

    // Bob fetches his sealed key, opens it, reads the message.
    let bob_state = open_best(&server_fetch(&db, channel, &bob), &bob.secret).unwrap();
    assert_eq!(bob_state.epoch, 1);
    assert_eq!(
        bob_state.decrypt(&msg1).unwrap(),
        "Q3 board deck is in the drive"
    );

    // Alice (steward) can obviously read her own channel too.
    let alice_state = open_best(&server_fetch(&db, channel, &alice), &alice.secret).unwrap();
    assert_eq!(alice_state.decrypt(&msg1).unwrap(), "Q3 board deck is in the drive");

    // ── Bob leaves the company → rotate to epoch 2, re-seal to Alice only ────
    let e2 = e1.rotate();
    server_store(&db, channel, &e2, &[&alice]);

    let msg2 = e2.encrypt("post-offboarding: new vendor pricing").unwrap();
    assert!(msg2.starts_with("EG1:2:"));

    // Alice picks up epoch 2 and reads the new message.
    let alice_e2 = open_best(&server_fetch(&db, channel, &alice), &alice.secret).unwrap();
    assert_eq!(alice_e2.epoch, 2);
    assert_eq!(
        alice_e2.decrypt(&msg2).unwrap(),
        "post-offboarding: new vendor pricing"
    );

    // Bob was NOT re-sealed. The newest key he can open is still epoch 1, so the
    // epoch-2 message is unreadable to him — revocation works.
    let bob_latest = open_best(&server_fetch(&db, channel, &bob), &bob.secret).unwrap();
    assert_eq!(bob_latest.epoch, 1, "Bob must not obtain the epoch-2 key");
    assert!(
        bob_latest.decrypt(&msg2).is_err(),
        "departed member must not decrypt post-rotation traffic"
    );

    // Alice retains epoch 1 too, so channel HISTORY across the rotation stays
    // readable to continuing members.
    let alice_e1 = server_fetch(&db, channel, &alice)
        .into_iter()
        .find(|(e, _)| *e == 1)
        .map(|(_, wire)| {
            GroupState::open(
                &freeq_sdk::e2ee_group::SealedGroupKey::from_wire(&wire).unwrap(),
                &alice.secret,
            )
            .unwrap()
        })
        .unwrap();
    assert_eq!(alice_e1.decrypt(&msg1).unwrap(), "Q3 board deck is in the drive");
}

#[test]
fn server_only_ever_holds_opaque_blobs() {
    // Prove the stored rows are sealed: nothing in what the server persists lets
    // it (or a disk thief) recover the plaintext of an EG1 message.
    let db = Db::open_memory().unwrap();
    let channel = "#secret";
    let carol = Member::new("did:plc:carol");

    let state = GroupState::create(channel);
    server_store(&db, channel, &state, &[&carol]);
    let ciphertext = state.encrypt("merger closes friday").unwrap();

    // Everything the server can see for this channel/member:
    let stored = server_fetch(&db, channel, &carol);
    assert_eq!(stored.len(), 1);
    let (_epoch, sealed_wire) = &stored[0];

    // The sealed blob is an EGK1 envelope; it is not the group secret and can't
    // be turned into one without Carol's static X25519 secret.
    assert!(sealed_wire.starts_with("EGK1:"));
    assert!(!sealed_wire.contains("merger"));
    assert!(!ciphertext.contains("merger"));

    // A different key (simulating the server trying to brute a substitute) can't open it.
    let attacker = StaticSecret::random_from_rng(rand::rngs::OsRng);
    assert!(open_best(&stored, &attacker).is_err());
}
