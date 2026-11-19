//! Squad ids: a random UUID v4 (122 random bits) written in lower-case Crockford base32, 26
//! characters such as `1z9k4m7q2x8v5n3b6c0d1e7f8g`. Knowing the id is the only access control,
//! so it has to be unguessable; the encoding only makes the link shorter than the dashed UUID
//! and easy to read out, as it has no `i`, `l`, `o` or `u`.
//!
//! Older squads have UUID ids (`3f2b9c1e-8a4d-…`). Ids are compared as plain strings, so those
//! links keep working.

/// Crockford's base32 alphabet, lower case.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Length of an encoded id: 128 bits in 5-bit characters, so the first holds only 3 bits.
pub const LEN: usize = 26;

/// Writes `bits` as a squad id. The caller supplies the randomness.
#[must_use]
pub fn encode(bits: u128) -> String {
    (0..LEN)
        .rev()
        // Masked to 5 bits, so the conversion can't fail.
        .map(|i| char::from(ALPHABET[usize::try_from((bits >> (5 * i)) & 0x1f).unwrap_or(0)]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Reads an id back, independently of `encode`: one character at a time.
    fn decode(id: &str) -> Option<u128> {
        if id.len() != LEN {
            return None;
        }
        id.bytes().try_fold(0u128, |acc, c| {
            let digit = ALPHABET.iter().position(|&a| a == c)?;
            acc.checked_mul(32)?
                .checked_add(u128::try_from(digit).ok()?)
        })
    }

    #[test]
    fn known_values() {
        assert_eq!(encode(0), "00000000000000000000000000");
        assert_eq!(encode(1), "00000000000000000000000001");
        assert_eq!(encode(31), "0000000000000000000000000z");
        assert_eq!(encode(32), "00000000000000000000000010");
        assert_eq!(encode(u128::MAX), "7zzzzzzzzzzzzzzzzzzzzzzzzz");
        // Only the top character is short: the highest bit lands in it as 4.
        assert_eq!(encode(1 << 127), "40000000000000000000000000");
        assert_eq!(encode(0x1f << 120), "0z000000000000000000000000");
    }

    #[test]
    fn alphabet_is_crockfords() {
        assert_eq!(ALPHABET.len(), 32);
        for c in *b"ilou" {
            assert!(!ALPHABET.contains(&c), "{}", char::from(c));
        }
        let mut sorted = *ALPHABET;
        sorted.sort_unstable();
        assert_eq!(
            &sorted, ALPHABET,
            "ascending, so ids sort like the numbers they hold"
        );
    }

    proptest! {
        #[test]
        fn round_trips(bits: u128) {
            prop_assert_eq!(decode(&encode(bits)), Some(bits));
        }

        #[test]
        fn is_26_url_safe_characters(bits: u128) {
            let id = encode(bits);
            prop_assert_eq!(id.len(), LEN);
            prop_assert!(id.bytes().all(|c| ALPHABET.contains(&c)), "{}", id);
        }

        #[test]
        fn different_bits_give_different_ids(a: u128, b: u128) {
            prop_assume!(a != b);
            prop_assert_ne!(encode(a), encode(b));
        }
    }
}
