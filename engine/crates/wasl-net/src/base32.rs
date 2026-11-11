//! Wasl's 32-symbol alphabet and a strict bit-packing base32 over it.
//!
//! The alphabet is `0123456789ABCDEFGHJKMNPQRSTVWXYZ` (Crockford's): the letters `I`, `L`, `O`
//! and `U` are never written. With 36 letters and digits a 32-symbol set must keep one of each
//! look-alike pair, so Wasl writes only the digits `0` and `1`, and when a person *types* a code
//! the look-alikes read as the symbol they resemble (`O`→`0`, `I`/`L`→`1`, any case). No two
//! written symbols are confusable, and a misread look-alike still decodes to the same value.
//! Machine-read text (QR payloads) is parsed strictly: upper case, written symbols only.

/// The 32 written symbols; a symbol's index is its 5-bit value.
pub(crate) const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The 5-bit value of a written symbol (strict: upper case, alphabet only).
pub(crate) fn value_strict(c: u8) -> Option<u8> {
    ALPHABET.iter().position(|&a| a == c).map(|i| i as u8)
}

/// The 5-bit value of a typed symbol: any case, look-alikes folded (`O`→0, `I`/`L`→1).
pub(crate) fn value_typed(c: u8) -> Option<u8> {
    match c.to_ascii_uppercase() {
        b'O' => Some(0),
        b'I' | b'L' => Some(1),
        u => value_strict(u),
    }
}

/// Packs bytes into 5-bit values, most significant bit first; the last value is zero-padded.
pub(crate) fn to_values(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity((bytes.len() * 8).div_ceil(5));
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 31) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 31) as u8);
    }
    out
}

/// Unpacks exactly `n_bytes` bytes from 5-bit values. Fails unless the value count is exactly
/// the packed length, every value is < 32, and the padding bits are zero (one text per value).
pub(crate) fn from_values(values: &[u8], n_bytes: usize) -> Option<Vec<u8>> {
    if values.len() != (n_bytes * 8).div_ceil(5) {
        return None;
    }
    let mut out = Vec::with_capacity(n_bytes);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &v in values {
        if v >= 32 {
            return None;
        }
        acc = (acc << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (acc == 0 && out.len() == n_bytes).then_some(out)
}

/// Writes 5-bit values as symbols.
pub(crate) fn write(values: &[u8]) -> String {
    values
        .iter()
        .map(|&v| ALPHABET[usize::from(v & 31)] as char)
        .collect()
}

/// Encodes bytes as written symbols.
pub(crate) fn encode(bytes: &[u8]) -> String {
    write(&to_values(bytes))
}

/// Decodes strictly written symbols into exactly `n_bytes` bytes.
pub(crate) fn decode_strict(text: &str, n_bytes: usize) -> Option<Vec<u8>> {
    let values: Option<Vec<u8>> = text.bytes().map(value_strict).collect();
    from_values(&values?, n_bytes)
}

/// Reads typed text into 5-bit values, skipping `-` and spaces; fails on any other symbol.
pub(crate) fn read_typed(text: &str) -> Option<Vec<u8>> {
    text.bytes()
        .filter(|c| !matches!(c, b'-' | b' '))
        .map(value_typed)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_has_no_look_alikes() {
        for c in b"ILOU" {
            assert!(!ALPHABET.contains(c));
        }
        let mut sorted = ALPHABET.to_vec();
        sorted.dedup();
        assert_eq!(sorted.len(), 32);
    }

    #[test]
    fn round_trips_every_length() {
        for n in 0..40usize {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let text = encode(&bytes);
            assert_eq!(decode_strict(&text, n).unwrap(), bytes);
        }
    }

    #[test]
    fn rejects_nonzero_padding_and_wrong_length() {
        // 1 byte = 2 symbols with 2 padding bits.
        assert_eq!(decode_strict("Z0", 1), Some(vec![0xF8]));
        assert_eq!(decode_strict("ZX", 1), None);
        assert_eq!(decode_strict("ZW", 1), Some(vec![0xFF]));
        assert_eq!(decode_strict("ZZ", 1), None);
        assert_eq!(decode_strict("ZW0", 1), None);
        assert_eq!(decode_strict("zw", 1), None);
    }

    #[test]
    fn typed_folds_look_alikes() {
        assert_eq!(read_typed("o-i l"), Some(vec![0, 1, 1]));
        assert_eq!(read_typed("u"), None);
    }
}
