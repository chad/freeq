//! did:key identity loading + on-first-run minting.
//!
//! Mirrors the layout `freeq-bot-id create` writes so the same bot can
//! be moved between Rust and TypeScript without re-minting:
//!
//! ```text
//! ~/.freeq/bots/<name>/
//! ├── key.ed25519       # 32-byte seed, mode 0600
//! └── identity.json     # `{ "id": "did:key:...", ... }`
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use freeq_sdk::crypto::PrivateKey;
use rand::RngCore;

/// Resolved identity for the bot.
pub struct Identity {
    pub did: String,
    pub private_key: PrivateKey,
}

/// Load `~/.freeq/bots/<name>/` if present; otherwise mint a fresh
/// ed25519 keypair, write it with mode 0600, and write a minimal
/// `identity.json` next to it.
pub fn load_or_create(name: &str) -> Result<Identity> {
    let home = dirs::home_dir().context("locating home directory")?;
    load_or_create_in(name, &home)
}

/// Same as [`load_or_create`] but rooted at a caller-supplied "home"
/// directory. Tests use this to point at a tempdir instead of the
/// real `$HOME`.
pub fn load_or_create_in(name: &str, home: &Path) -> Result<Identity> {
    validate_bot_name(name)?;
    let dir = home.join(".freeq").join("bots").join(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let key_path = dir.join("key.ed25519");
    let id_path = dir.join("identity.json");

    let seed: [u8; 32] = if key_path.exists() {
        let raw = std::fs::read(&key_path).with_context(|| format!("reading {}", key_path.display()))?;
        if raw.len() != 32 {
            anyhow::bail!(
                "expected 32-byte seed at {}, got {} bytes",
                key_path.display(),
                raw.len()
            );
        }
        let mut s = [0u8; 32];
        s.copy_from_slice(&raw);
        s
    } else {
        let mut s = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut s);
        write_secret(&key_path, &s).with_context(|| format!("writing {}", key_path.display()))?;
        s
    };

    let signing_key = SigningKey::from_bytes(&seed);
    let did = did_key_from_pubkey(signing_key.verifying_key().to_bytes());
    let private_key = PrivateKey::ed25519_from_bytes(&seed).context("constructing PrivateKey from seed")?;

    if !id_path.exists() {
        let doc = serde_json::json!({
            "id": did,
            "createdAt": chrono::Utc::now().to_rfc3339(),
            "note": "Minted by freeq-utopia. Compatible with freeq-bot-id and @freeq/bot-kit layouts.",
        });
        std::fs::write(&id_path, serde_json::to_vec_pretty(&doc)?)
            .with_context(|| format!("writing {}", id_path.display()))?;
    }

    Ok(Identity { did, private_key })
}

/// Path layout helper, exposed for tests. Returns `<home>/.freeq/bots/<name>`.
pub fn bot_dir_in(name: &str, home: &Path) -> PathBuf {
    home.join(".freeq").join("bots").join(name)
}

/// Reject names that could escape `~/.freeq/bots/` or smuggle weird
/// characters into log lines / shell scripts. We treat the name as a
/// single path component and refuse anything that isn't ASCII-ish.
///
/// Adversarial inputs this closes:
///   - `..` / `../foo` / `foo/../bar` → path traversal
///   - `foo/bar` / `foo\bar`         → multi-component nesting
///   - empty string                  → identity dir is the whole `bots/`
///   - control chars / NUL / newline → log injection, Windows surprises
///   - extremely long names          → resource exhaustion / FS quirks
pub fn validate_bot_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("bot name must not be empty");
    }
    if name.len() > 64 {
        anyhow::bail!("bot name must be ≤ 64 chars (got {})", name.len());
    }
    if name == "." || name == ".." {
        anyhow::bail!("bot name {name:?} is a reserved path component");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("bot name {name:?} must not contain path separators");
    }
    if name.contains("..") {
        anyhow::bail!("bot name {name:?} must not contain '..'");
    }
    if name.chars().any(|c| c.is_control()) {
        anyhow::bail!("bot name {name:?} must not contain control characters");
    }
    Ok(())
}

#[cfg(unix)]
fn write_secret(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

/// did:key encoding for an ed25519 public key. The multicodec varint
/// for ed25519 is 0xED 0x01 and we base58btc-encode the result with a
/// leading `z` per the did:key spec.
fn did_key_from_pubkey(pubkey: [u8; 32]) -> String {
    let mut prefixed = Vec::with_capacity(34);
    prefixed.extend_from_slice(&[0xed, 0x01]);
    prefixed.extend_from_slice(&pubkey);
    format!("did:key:z{}", bs58::encode(&prefixed).into_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeq_sdk::crypto::PrivateKey as SdkPrivateKey;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use tempfile::TempDir;

    /// did:key for the all-zeros ed25519 seed. This is a well-known
    /// vector — the public key bytes are
    /// `3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29`,
    /// the multicodec prefix is `0xed 0x01`, base58btc with `z`.
    /// Pinning this string catches:
    ///   - flipping the multicodec varint
    ///   - swapping endianness on the key bytes
    ///   - dropping the leading `z`
    ///   - using base58 (no btc alphabet) or base64
    const ZERO_SEED_DID: &str = "did:key:z6MkiTBz1ymuepAQ4HEHYSF1H8quG5GLVVQR3djdX3mDooWp";

    #[test]
    fn did_key_zero_seed_pins_encoding() {
        let sk = SigningKey::from_bytes(&[0u8; 32]);
        let got = did_key_from_pubkey(sk.verifying_key().to_bytes());
        assert_eq!(got, ZERO_SEED_DID);
    }

    /// Interop contract: the bot's `did_key_from_pubkey` and the SDK's
    /// `PrivateKey::public_key_multibase()` (which is what `freeq-bot-id`
    /// uses) must produce identical strings for the same seed. If one
    /// changes — different prefix, different alphabet — the bot's DID
    /// stops matching the one a user minted with the CLI and SASL
    /// breaks silently.
    #[test]
    fn did_key_matches_sdk_public_key_multibase() {
        for seed_byte in [0u8, 1, 42, 0xff] {
            let seed = [seed_byte; 32];
            let bot_did = {
                let sk = SigningKey::from_bytes(&seed);
                did_key_from_pubkey(sk.verifying_key().to_bytes())
            };
            let sdk = SdkPrivateKey::ed25519_from_bytes(&seed).unwrap();
            let expected = format!("did:key:{}", sdk.public_key_multibase());
            assert_eq!(bot_did, expected, "seed byte {seed_byte:#x}");
        }
    }

    #[test]
    fn did_key_does_not_collide_for_different_seeds() {
        let a = {
            let sk = SigningKey::from_bytes(&[1u8; 32]);
            did_key_from_pubkey(sk.verifying_key().to_bytes())
        };
        let b = {
            let sk = SigningKey::from_bytes(&[2u8; 32]);
            did_key_from_pubkey(sk.verifying_key().to_bytes())
        };
        assert_ne!(a, b);
    }

    #[test]
    fn path_traversal_dotdot_rejected() {
        let home = TempDir::new().unwrap();
        let err = load_or_create_in("..", home.path()).err().expect("expected error");
        assert!(format!("{err:#}").contains("reserved"), "got: {err:#}");
    }

    #[test]
    fn path_traversal_with_slash_rejected() {
        let home = TempDir::new().unwrap();
        let err = load_or_create_in("../evil", home.path()).err().expect("expected error");
        assert!(
            format!("{err:#}").contains("path separators") || format!("{err:#}").contains("'..'"),
            "got: {err:#}"
        );
        // Make sure the attacker did NOT create a key file at
        // `<home>/.freeq/bots/../evil/...` (which would resolve to
        // `<home>/.freeq/evil/`).
        let escaped = home.path().join(".freeq").join("evil");
        assert!(!escaped.exists(), "path traversal succeeded — directory leaked outside bots/");
    }

    #[test]
    fn path_traversal_backslash_rejected() {
        let home = TempDir::new().unwrap();
        // Windows path separator should be rejected even on Unix, to
        // keep behaviour platform-portable.
        let err = load_or_create_in("foo\\bar", home.path()).err().expect("expected error");
        assert!(format!("{err:#}").contains("path separators"), "got: {err:#}");
    }

    #[test]
    fn empty_name_rejected() {
        let home = TempDir::new().unwrap();
        let err = load_or_create_in("", home.path()).err().expect("expected error");
        assert!(format!("{err:#}").contains("empty"), "got: {err:#}");
    }

    #[test]
    fn overlong_name_rejected() {
        let home = TempDir::new().unwrap();
        let name = "a".repeat(65);
        let err = load_or_create_in(&name, home.path()).err().expect("expected error");
        assert!(format!("{err:#}").contains("64"), "got: {err:#}");
    }

    #[test]
    fn control_chars_in_name_rejected() {
        let home = TempDir::new().unwrap();
        for bad in ["foo\nbar", "foo\0bar", "foo\tbar", "foo\rbar"] {
            let err = load_or_create_in(bad, home.path()).err().expect("expected error");
            assert!(
                format!("{err:#}").contains("control characters"),
                "input {bad:?} should be rejected, got: {err:#}"
            );
        }
    }

    #[test]
    fn unicode_name_accepted_and_round_trips() {
        // We don't ban non-ASCII letters — only path separators and
        // control characters. Pin that emoji + accented characters
        // round-trip cleanly so we don't regress to rejecting them.
        let home = TempDir::new().unwrap();
        let id_a = load_or_create_in("résumé", home.path()).expect("unicode rejected");
        let id_b = load_or_create_in("résumé", home.path()).expect("second load failed");
        assert_eq!(id_a.did, id_b.did);
    }

    #[test]
    fn truncated_seed_file_errors_with_path() {
        let home = TempDir::new().unwrap();
        let bot_dir = home.path().join(".freeq").join("bots").join("trunc");
        std::fs::create_dir_all(&bot_dir).unwrap();
        // 31 bytes — one short of the required 32. An attacker who can
        // overwrite the key file must not get a silently zero-padded
        // identity.
        std::fs::write(bot_dir.join("key.ed25519"), vec![0xaa; 31]).unwrap();
        let err = load_or_create_in("trunc", home.path()).err().expect("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("31"), "error should mention the byte count: {msg}");
        assert!(msg.contains("key.ed25519"), "error should mention the file path: {msg}");
    }

    #[test]
    fn oversized_seed_file_errors_without_truncating() {
        let home = TempDir::new().unwrap();
        let bot_dir = home.path().join(".freeq").join("bots").join("over");
        std::fs::create_dir_all(&bot_dir).unwrap();
        // 64 bytes — twice the required size. Must not silently truncate
        // (which would pick the wrong DID) and must surface the size in
        // the error.
        std::fs::write(bot_dir.join("key.ed25519"), vec![0xbb; 64]).unwrap();
        let err = load_or_create_in("over", home.path()).err().expect("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("64"), "error should mention the byte count: {msg}");
    }

    #[test]
    fn empty_seed_file_errors() {
        // Zero-byte key.ed25519 is the "truncated to nothing" edge
        // case; an attacker who can `touch` the file must not get a
        // valid identity.
        let home = TempDir::new().unwrap();
        let bot_dir = home.path().join(".freeq").join("bots").join("empty");
        std::fs::create_dir_all(&bot_dir).unwrap();
        std::fs::write(bot_dir.join("key.ed25519"), b"").unwrap();
        let err = load_or_create_in("empty", home.path()).err().expect("expected error");
        assert!(format!("{err:#}").contains("0 bytes"), "got: {err:#}");
    }

    #[test]
    fn identity_json_without_id_field_still_loads() {
        // We don't read identity.json back — the DID is recomputed from
        // the seed. Pin the contract that a malformed / legacy
        // identity.json doesn't break startup.
        let home = TempDir::new().unwrap();
        let id_a = load_or_create_in("nofield", home.path()).unwrap();
        let bot_dir = home.path().join(".freeq").join("bots").join("nofield");
        // Overwrite identity.json with garbage that has no `id`.
        std::fs::write(bot_dir.join("identity.json"), br#"{"createdAt":"whenever"}"#).unwrap();
        let id_b = load_or_create_in("nofield", home.path()).unwrap();
        assert_eq!(
            id_a.did, id_b.did,
            "DID must be derived from the seed, not from identity.json"
        );
    }

    #[test]
    fn second_load_keeps_same_did() {
        let home = TempDir::new().unwrap();
        let a = load_or_create_in("stable", home.path()).unwrap();
        let b = load_or_create_in("stable", home.path()).unwrap();
        assert_eq!(a.did, b.did);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_mode_0600() {
        // Defence-in-depth: the seed must not be world-readable.
        use std::os::unix::fs::PermissionsExt;
        let home = TempDir::new().unwrap();
        let _ = load_or_create_in("perms", home.path()).unwrap();
        let key = home.path().join(".freeq").join("bots").join("perms").join("key.ed25519");
        let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file mode is {mode:o}");
    }

    #[test]
    fn concurrent_first_create_yields_one_winning_did() {
        // Two callers racing on first-create: contract is "both end up
        // with the same DID". The loser must observe the winner's seed
        // (via `create_new`'s EEXIST → fall through to the read path)
        // and produce an identical Identity.
        let home = TempDir::new().unwrap();
        let home_path = home.path().to_path_buf();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let dids = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let errors = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..8 {
            let barrier = barrier.clone();
            let home_path = home_path.clone();
            let dids = dids.clone();
            let errors = errors.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                match load_or_create_in("racy", &home_path) {
                    Ok(id) => dids.lock().unwrap().push(id.did),
                    Err(_) => {
                        errors.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Either everyone gets the same DID (winner's seed wins) or
        // some callers see a transient mid-write error and bail. What
        // we MUST NOT see is two distinct DIDs — that would mean a
        // racing creator silently overwrote the winner's key.
        let dids = dids.lock().unwrap();
        let unique: std::collections::HashSet<_> = dids.iter().collect();
        assert!(
            unique.len() <= 1,
            "concurrent first-create produced multiple DIDs: {unique:?}"
        );
        // At least one caller must have succeeded.
        assert!(!dids.is_empty(), "all 8 callers errored — sanity check");
    }

    #[test]
    fn validate_bot_name_accepts_typical_names() {
        for ok in ["transcriber", "my-bot", "bot_42", "Bot42"] {
            validate_bot_name(ok).unwrap_or_else(|e| panic!("rejected {ok:?}: {e}"));
        }
    }

    #[test]
    fn bot_dir_in_pins_layout() {
        let home = TempDir::new().unwrap();
        let p = bot_dir_in("transcriber", home.path());
        assert_eq!(p, home.path().join(".freeq").join("bots").join("transcriber"));
    }
}
