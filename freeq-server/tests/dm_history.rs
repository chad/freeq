//! DM history tests: canonical key, persistence, dm_conversations.
//!
//! Tests cover:
//! - Canonical DM key computation (alphabetical sorting, determinism)
//! - DM message storage and retrieval via existing messages table
//! - Isolation between DM conversations and channels
//! - dm_conversations() listing, ordering, limits, filtering
//! - BEFORE/AFTER/LATEST/BETWEEN subcommands with DM keys
//! - Tags, msgid, and encryption-at-rest with DM messages

use std::collections::HashMap;

use freeq_server::db::{canonical_dm_key, Db};

fn make_db() -> Db {
    Db::open_memory().unwrap()
}

#[test]
fn canonical_key_alphabetical_order() {
    let k1 = canonical_dm_key("did:plc:alice", "did:plc:bob");
    let k2 = canonical_dm_key("did:plc:bob", "did:plc:alice");
    assert_eq!(k1, k2, "Key should be the same regardless of argument order");
    assert_eq!(k1, "dm:did:plc:alice,did:plc:bob");
}

#[test]
fn canonical_key_same_did() {
    let k = canonical_dm_key("did:plc:alice", "did:plc:alice");
    assert_eq!(k, "dm:did:plc:alice,did:plc:alice");
}

#[test]
fn canonical_key_deterministic() {
    let k1 = canonical_dm_key("did:plc:xyz", "did:plc:abc");
    let k2 = canonical_dm_key("did:plc:xyz", "did:plc:abc");
    assert_eq!(k1, k2);
}

#[test]
fn dm_messages_stored_and_retrieved() {
    let db = make_db();
    let key = canonical_dm_key("did:plc:alice", "did:plc:bob");

    db.insert_message(&key, "alice!user@host", "hello bob", 1000, &HashMap::new(), Some("msg1"))
        .unwrap();
    db.insert_message(&key, "bob!user@host", "hi alice", 1001, &HashMap::new(), Some("msg2"))
        .unwrap();

    let msgs = db.get_messages(&key, 10, None).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].text, "hello bob");
    assert_eq!(msgs[1].text, "hi alice");
}

#[test]
fn dm_messages_isolated_from_channels() {
    let db = make_db();
    let dm_key = canonical_dm_key("did:plc:alice", "did:plc:bob");

    db.insert_message(&dm_key, "alice!user@host", "dm message", 1000, &HashMap::new(), Some("msg1"))
        .unwrap();
    db.insert_message("#general", "alice!user@host", "channel message", 1001, &HashMap::new(), Some("msg2"))
        .unwrap();

    let dm_msgs = db.get_messages(&dm_key, 10, None).unwrap();
    let ch_msgs = db.get_messages("#general", 10, None).unwrap();
    assert_eq!(dm_msgs.len(), 1);
    assert_eq!(ch_msgs.len(), 1);
    assert_eq!(dm_msgs[0].text, "dm message");
    assert_eq!(ch_msgs[0].text, "channel message");
}

#[test]
fn dm_messages_isolated_between_conversations() {
    let db = make_db();
    let key_ab = canonical_dm_key("did:plc:alice", "did:plc:bob");
    let key_ac = canonical_dm_key("did:plc:alice", "did:plc:charlie");

    db.insert_message(&key_ab, "alice", "to bob", 1000, &HashMap::new(), Some("msg1"))
        .unwrap();
    db.insert_message(&key_ac, "alice", "to charlie", 1001, &HashMap::new(), Some("msg2"))
        .unwrap();

    let ab = db.get_messages(&key_ab, 10, None).unwrap();
    let ac = db.get_messages(&key_ac, 10, None).unwrap();
    assert_eq!(ab.len(), 1);
    assert_eq!(ac.len(), 1);
    assert_eq!(ab[0].text, "to bob");
    assert_eq!(ac[0].text, "to charlie");
}

#[test]
fn dm_conversations_lists_for_did() {
    let db = make_db();
    let key_ab = canonical_dm_key("did:plc:alice", "did:plc:bob");
    let key_ac = canonical_dm_key("did:plc:alice", "did:plc:charlie");
    let key_bc = canonical_dm_key("did:plc:bob", "did:plc:charlie");

    db.insert_message(&key_ab, "alice", "hi bob", 1000, &HashMap::new(), None).unwrap();
    db.insert_message(&key_ac, "alice", "hi charlie", 2000, &HashMap::new(), None).unwrap();
    db.insert_message(&key_bc, "bob", "hi charlie", 3000, &HashMap::new(), None).unwrap();

    // Alice should see 2 conversations (ab, ac) but not bc
    let alice_convos = db.dm_conversations("did:plc:alice", 50).unwrap();
    assert_eq!(alice_convos.len(), 2);
    // Ordered by most recent
    assert_eq!(alice_convos[0].0, key_ac); // ts 2000
    assert_eq!(alice_convos[1].0, key_ab); // ts 1000

    // Bob should see 2 conversations (ab, bc)
    let bob_convos = db.dm_conversations("did:plc:bob", 50).unwrap();
    assert_eq!(bob_convos.len(), 2);
    assert_eq!(bob_convos[0].0, key_bc); // ts 3000
    assert_eq!(bob_convos[1].0, key_ab); // ts 1000

    // Charlie should see 2 conversations (ac, bc)
    let charlie_convos = db.dm_conversations("did:plc:charlie", 50).unwrap();
    assert_eq!(charlie_convos.len(), 2);
}

#[test]
fn dm_conversations_respects_limit() {
    let db = make_db();
    for i in 0..5 {
        let partner = format!("did:plc:partner{i}");
        let key = canonical_dm_key("did:plc:alice", &partner);
        db.insert_message(&key, "alice", &format!("msg {i}"), 1000 + i, &HashMap::new(), None)
            .unwrap();
    }

    let convos = db.dm_conversations("did:plc:alice", 3).unwrap();
    assert_eq!(convos.len(), 3);
}

#[test]
fn dm_conversations_empty_for_unknown_did() {
    let db = make_db();
    let key = canonical_dm_key("did:plc:alice", "did:plc:bob");
    db.insert_message(&key, "alice", "hello", 1000, &HashMap::new(), None).unwrap();

    let convos = db.dm_conversations("did:plc:nobody", 50).unwrap();
    assert!(convos.is_empty());
}

#[test]
fn dm_conversations_excludes_channels() {
    let db = make_db();
    let dm_key = canonical_dm_key("did:plc:alice", "did:plc:bob");
    db.insert_message(&dm_key, "alice", "dm msg", 1000, &HashMap::new(), None).unwrap();
    db.insert_message("#general", "alice", "channel msg", 2000, &HashMap::new(), None).unwrap();

    let convos = db.dm_conversations("did:plc:alice", 50).unwrap();
    assert_eq!(convos.len(), 1);
    assert!(convos[0].0.starts_with("dm:"));
}

#[test]
fn dm_conversations_excludes_deleted() {
    let db = make_db();
    let key = canonical_dm_key("did:plc:alice", "did:plc:bob");
    db.insert_message(&key, "alice", "msg1", 1000, &HashMap::new(), Some("del1")).unwrap();
    db.insert_message(&key, "alice", "msg2", 2000, &HashMap::new(), Some("keep1")).unwrap();

    // Delete one message
    db.soft_delete_message(&key, "del1").unwrap();

    // Conversation should still appear (one message remains)
    let convos = db.dm_conversations("did:plc:alice", 50).unwrap();
    assert_eq!(convos.len(), 1);
    assert_eq!(convos[0].1, 2000); // latest non-deleted timestamp
}

#[test]
fn dm_history_before_after_latest() {
    let db = make_db();
    let key = canonical_dm_key("did:plc:alice", "did:plc:bob");
    for i in 0..10 {
        db.insert_message(&key, "alice", &format!("msg-{i}"), 1000 + i, &HashMap::new(), None)
            .unwrap();
    }

    // BEFORE ts=1005 limit=3 -> messages at 1002,1003,1004
    let before = db.get_messages(&key, 3, Some(1005)).unwrap();
    assert_eq!(before.len(), 3);
    assert_eq!(before[0].text, "msg-2");
    assert_eq!(before[2].text, "msg-4");

    // AFTER ts=1005 limit=3 -> messages at 1006,1007,1008
    let after = db.get_messages_after(&key, 1005, 3).unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(after[0].text, "msg-6");

    // LATEST limit=3 -> last 3 messages
    let latest = db.get_messages(&key, 3, None).unwrap();
    assert_eq!(latest.len(), 3);
    assert_eq!(latest[2].text, "msg-9");

    // BETWEEN 1003..1007 limit=10 (exclusive bounds: 1004,1005,1006)
    let between = db.get_messages_between(&key, 1003, 1007, 10).unwrap();
    assert_eq!(between.len(), 3);
    assert_eq!(between[0].text, "msg-4");
    assert_eq!(between[2].text, "msg-6");
}

#[test]
fn dm_with_tags_and_msgid() {
    let db = make_db();
    let key = canonical_dm_key("did:plc:alice", "did:plc:bob");
    let mut tags = HashMap::new();
    tags.insert("msgid".to_string(), "dm001".to_string());
    tags.insert("+freeq.at/sig".to_string(), "sig123".to_string());

    db.insert_message(&key, "alice!user@host", "signed dm", 1000, &tags, Some("dm001"))
        .unwrap();

    let msgs = db.get_messages(&key, 10, None).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].text, "signed dm");
    assert_eq!(msgs[0].msgid.as_deref(), Some("dm001"));
    assert_eq!(msgs[0].tags.get("+freeq.at/sig").unwrap(), "sig123");
}

#[test]
fn dm_encrypted_roundtrip() {
    let key: [u8; 32] = [0xAB; 32];
    let db = Db::open_encrypted_memory(key).unwrap();
    let dm_key = canonical_dm_key("did:plc:alice", "did:plc:bob");

    db.insert_message(&dm_key, "alice", "secret dm", 1000, &HashMap::new(), None).unwrap();

    let msgs = db.get_messages(&dm_key, 10, None).unwrap();
    assert_eq!(msgs[0].text, "secret dm");

    // Verify raw storage is encrypted
    let raw = db.get_raw_message_text(&dm_key, 1000).unwrap();
    assert!(raw.starts_with("EAR1:"));
}
