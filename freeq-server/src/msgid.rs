//! ULID-based message ID generation.
//!
//! Each message gets a globally unique, time-sortable identifier.
//! Format: 26-character Crockford base32 string (compatible with IRCv3 `msgid` tag).
//!
//! Structure: 48 bits timestamp (ms since epoch) + 80 bits random.

use rand::Rng;

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Generate a new ULID string.
pub fn generate() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut rng = rand::thread_rng();
    let rand_hi: u16 = rng.r#gen();
    let rand_lo: u64 = rng.r#gen();

    let mut buf = [0u8; 26];

    // Encode timestamp (10 chars, most significant first)
    let mut ts = now_ms;
    for i in (0..10).rev() {
        buf[i] = CROCKFORD[(ts & 0x1F) as usize];
        ts >>= 5;
    }

    // Encode random: 16 bits from rand_hi (3 chars) + 64 bits from rand_lo (13 chars)
    let mut r = rand_hi as u128 | ((rand_lo as u128) << 16);
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
                c.is_ascii_digit() || (c.is_ascii_uppercase() && c != 'I' && c != 'L' && c != 'O' && c != 'U'),
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
