//! ULID-based message ID generation.
//!
//! Each message gets a globally unique, time-sortable identifier.
//! Format: 26-character Crockford base32 string (compatible with IRCv3 `msgid` tag).
//!
//! Structure: 48 bits timestamp (ms since epoch) + 80 bits random.
//!
//! Monotonic: within the same millisecond, the random component is
//! incremented to guarantee sort order matches generation order.

use rand::Rng;
use std::sync::Mutex;

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Monotonic state: last timestamp and random component.
static LAST: Mutex<(u64, u128)> = Mutex::new((0, 0));

/// Generate a new monotonic ULID string.
pub fn generate() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let (ts, rand_bits) = {
        let mut last = LAST.lock().unwrap();
        if now_ms == last.0 {
            // Same millisecond — increment random to maintain ordering.
            // The 80-bit random space is large enough that overflow is
            // effectively impossible in practice.
            last.1 = last.1.wrapping_add(1);
            (now_ms, last.1)
        } else {
            // New millisecond — fresh random
            let mut rng = rand::thread_rng();
            let r: u128 = ((rng.r#gen::<u16>() as u128) << 64) | rng.r#gen::<u64>() as u128;
            // Mask to 80 bits
            let r = r & ((1u128 << 80) - 1);
            *last = (now_ms, r);
            (now_ms, r)
        }
    };

    let mut buf = [0u8; 26];

    // Encode timestamp (10 chars, most significant first)
    let mut t = ts;
    for i in (0..10).rev() {
        buf[i] = CROCKFORD[(t & 0x1F) as usize];
        t >>= 5;
    }

    // Encode 80-bit random (16 chars, most significant first)
    let mut r = rand_bits;
    for i in (10..26).rev() {
        buf[i] = CROCKFORD[(r & 0x1F) as usize];
        r >>= 5;
    }

    // SAFETY: all bytes are ASCII from CROCKFORD alphabet
    unsafe { String::from_utf8_unchecked(buf.to_vec()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_length_and_uniqueness() {
        let a = generate();
        let b = generate();
        assert_eq!(a.len(), 26);
        assert_eq!(b.len(), 26);
        assert_ne!(a, b);
    }

    #[test]
    fn ulid_is_ascii_crockford() {
        let id = generate();
        for c in id.chars() {
            assert!(
                c.is_ascii_digit()
                    || (c.is_ascii_uppercase() && c != 'I' && c != 'L' && c != 'O' && c != 'U'),
                "Invalid Crockford char: {c}"
            );
        }
    }

    #[test]
    fn ulid_monotonic_ordering() {
        let a = generate();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = generate();
        assert!(a < b, "ULIDs should sort chronologically: {a} vs {b}");
    }
}
