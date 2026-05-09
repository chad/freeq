//! Unit tests for `InviteExceptionEntry` matching semantics.
//!
//! Pure-logic tests of the `matches()` predicate — no server, no SDK,
//! no async runtime. Mirrors the matching contract that the +I JOIN
//! admission gate relies on.

use freeq_server::server::InviteExceptionEntry;

fn entry(mask: &str) -> InviteExceptionEntry {
    InviteExceptionEntry {
        mask: mask.to_string(),
        set_by: "op".to_string(),
        set_at: 0,
    }
}

#[test]
fn matches_did_exact() {
    let e = entry("did:plc:alice");
    assert!(e.matches("alice!a@host", Some("did:plc:alice")));
    assert!(!e.matches("alice!a@host", Some("did:plc:bob")));
    // DID-form entries do not match when no DID is supplied.
    assert!(!e.matches("alice!a@host", None));
}

#[test]
fn matches_hostmask_wildcard() {
    let e = entry("*!*@trusted.example");
    assert!(e.matches("alice!a@trusted.example", None));
    // Case-insensitive.
    assert!(e.matches("BOB!b@TRUSTED.EXAMPLE", None));
    // Different host — no match.
    assert!(!e.matches("alice!a@elsewhere.example", None));
    // Hostmask entries match irrespective of any supplied DID.
    assert!(e.matches("alice!a@trusted.example", Some("did:plc:bob")));
}

#[test]
fn matches_anchors_did_prefix() {
    // "did:" prefix triggers DID-only matching, even if the hostmask
    // happens to be a verbatim copy of the DID string.
    let e = entry("did:plc:alice");
    assert!(!e.matches("did:plc:alice", None));
}
