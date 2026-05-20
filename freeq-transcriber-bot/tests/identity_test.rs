//! Identity loading / minting tests.
//!
//! All of these run against a tempdir injected through
//! [`identity::load_or_create_in`] so we don't touch `$HOME/.freeq`.

use freeq_transcriber_bot::identity;

/// Fresh tempdir → a brand new key + identity.json + mode 0600 on unix.
#[test]
fn mints_fresh_identity_on_first_run() {
    let tmp = tempfile::tempdir().unwrap();
    let ident = identity::load_or_create_in("alpha", tmp.path()).expect("mint");

    let dir = identity::bot_dir_in("alpha", tmp.path());
    let key_path = dir.join("key.ed25519");
    let id_path = dir.join("identity.json");
    assert!(key_path.exists(), "key file missing");
    assert!(id_path.exists(), "identity.json missing");

    let seed = std::fs::read(&key_path).unwrap();
    assert_eq!(seed.len(), 32, "ed25519 seed must be exactly 32 bytes");

    assert!(ident.did.starts_with("did:key:z"), "DID is not did:key: {}", ident.did);

    // identity.json is valid JSON and contains the DID we returned.
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&id_path).unwrap()).unwrap();
    assert_eq!(json["id"].as_str(), Some(ident.did.as_str()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file must be mode 0600, got {mode:o}");
    }
}

/// Calling twice in the same dir → identical DID (idempotent).
#[test]
fn second_call_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let a = identity::load_or_create_in("idem", tmp.path()).unwrap();
    let b = identity::load_or_create_in("idem", tmp.path()).unwrap();
    assert_eq!(a.did, b.did, "DID must be stable across calls");
    // Private key bytes should also match.
    assert_eq!(a.private_key.secret_bytes(), b.private_key.secret_bytes());
}

/// A pre-existing 32-byte seed deterministically produces the same DID
/// as what `freeq-bot-id` would write. We don't shell out to that crate;
/// instead we replicate its derivation (multicodec 0xED 0x01 || pubkey,
/// base58btc, "z"-prefix) and assert ours matches.
#[test]
fn deterministic_did_from_existing_seed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = identity::bot_dir_in("det", tmp.path());
    std::fs::create_dir_all(&dir).unwrap();

    // Known seed (all 0x42), pre-written.
    let seed = [0x42u8; 32];
    let key_path = dir.join("key.ed25519");
    std::fs::write(&key_path, seed).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&key_path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&key_path, perms).unwrap();
    }

    let ident = identity::load_or_create_in("det", tmp.path()).unwrap();

    // Compute the expected DID locally.
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pubkey = signing.verifying_key().to_bytes();
    let mut prefixed = Vec::with_capacity(34);
    prefixed.extend_from_slice(&[0xed, 0x01]);
    prefixed.extend_from_slice(&pubkey);
    let expected_did = format!("did:key:z{}", bs58::encode(&prefixed).into_string());

    assert_eq!(ident.did, expected_did);

    // And again it should be stable on the second call.
    let again = identity::load_or_create_in("det", tmp.path()).unwrap();
    assert_eq!(again.did, expected_did);
}

/// Corrupted seed (wrong length) → clean error naming the path.
#[test]
fn corrupted_seed_errors_with_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = identity::bot_dir_in("bad", tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    let key_path = dir.join("key.ed25519");
    std::fs::write(&key_path, b"not-32-bytes").unwrap();

    let err = identity::load_or_create_in("bad", tmp.path())
        .map(|_| ())
        .expect_err("must reject short seed");
    let msg = format!("{err:#}");
    assert!(msg.contains("32-byte"), "error should mention 32-byte: {msg}");
    assert!(
        msg.contains(&key_path.display().to_string()),
        "error should name the bad path: {msg}",
    );
}

/// Two different bot names under the same home → distinct DIDs.
#[test]
fn distinct_names_yield_distinct_dids() {
    let tmp = tempfile::tempdir().unwrap();
    let a = identity::load_or_create_in("one", tmp.path()).unwrap();
    let b = identity::load_or_create_in("two", tmp.path()).unwrap();
    assert_ne!(a.did, b.did);
}
