//! Lowercase hex encoding.
//!
//! Five call sites across `gascan-core`, `gascand` and `gascan-apple` each built
//! this with `bytes.iter().map(|b| format!("{b:02x}")).collect()`, which allocates
//! a `String` per byte and trips `clippy::format_collect` under the toolchain
//! `rust-toolchain.toml` pins. One implementation replaces all five.
//!
//! Written with a nibble lookup rather than `write!` because this crate denies
//! `clippy::expect_used`, `clippy::panic` and `clippy::unwrap_used`
//! (`lib.rs:2`), and `write!` into a `String` returns a `Result` that would need
//! one of them to discharge. Indexing a 16-byte table with a nibble cannot go out
//! of bounds, so there is no error to discharge at all.

/// Lowercase hex digits, indexed by nibble value.
const DIGITS: [u8; 16] = *b"0123456789abcdef";

/// Encodes `bytes` as lowercase hex, two characters per byte.
///
/// Byte-for-byte identical to the `format!("{byte:02x}")`-per-byte idiom it
/// replaces, so callers persisting or transmitting the result are unaffected.
#[must_use]
pub fn lower(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // `>> 4` and `& 0x0f` both yield 0..=15, so both indexes are in bounds.
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::lower;

    #[test]
    fn empty_input_encodes_to_an_empty_string() {
        assert_eq!(lower(&[]), "");
    }

    #[test]
    fn each_byte_becomes_exactly_two_lowercase_characters() {
        assert_eq!(lower(&[0x00]), "00");
        assert_eq!(lower(&[0x0f]), "0f");
        assert_eq!(lower(&[0xa5]), "a5");
        assert_eq!(lower(&[0xff]), "ff");
        assert_eq!(lower(&[0x0a, 0xb3, 0x00, 0xff]), "0ab300ff");
    }

    #[test]
    fn length_is_always_twice_the_input() {
        for length in 0_usize..=32 {
            let bytes = vec![0x5a_u8; length];
            assert_eq!(lower(&bytes).len(), length * 2);
        }
    }

    /// Guards the replacement of the five original call sites: this must agree
    /// with the idiom it replaced across every possible byte value.
    ///
    /// The lint is allowed here, and only here, because reproducing the exact
    /// idiom being replaced is the assertion. Rewriting it to satisfy the lint
    /// would compare `lower` against itself and prove nothing.
    #[test]
    #[allow(clippy::format_collect)]
    fn agrees_with_the_replaced_format_idiom_for_all_byte_values() {
        let every_byte: Vec<u8> = (0..=u8::MAX).collect();
        let expected: String = every_byte
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(lower(&every_byte), expected);
    }
}
