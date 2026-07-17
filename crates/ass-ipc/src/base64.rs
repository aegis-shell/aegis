//! Minimal RFC 4648 base64 (standard alphabet, with padding).
//!
//! The IPC wire format is JSON; binary payloads (captured frames) travel as
//! base64 strings. Kept dependency-free: the codec is small and fully
//! tested here.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `data` as a base64 string with padding.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decode a base64 string. Returns `None` on invalid input (bad alphabet,
/// bad padding, or truncated quartet).
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut table = [0xFFu8; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (quartet_index, chunk) in bytes.chunks(4).enumerate() {
        let last = quartet_index == bytes.len() / 4 - 1;
        let pad = if last {
            chunk.iter().filter(|&&c| c == b'=').count()
        } else {
            0
        };
        if pad > 2 || (!last && chunk.contains(&b'=')) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                // Padding may only trail the final quartet.
                if i < 4 - pad {
                    return None;
                }
                n <<= 6;
            } else {
                let v = table[c as usize];
                if v == 0xFF {
                    return None;
                }
                n = (n << 6) | v as u32;
            }
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trip() {
        assert_eq!(encode(b""), "");
        assert_eq!(decode(""), Some(vec![]));
    }

    #[test]
    fn rfc4648_vectors() {
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn binary_round_trip() {
        let data: Vec<u8> = (0..=255).cycle().take(1000).collect();
        assert_eq!(decode(&encode(&data)), Some(data));
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!(decode("Zg="), None); // truncated
        assert_eq!(decode("Zg==Zg=="), None); // interior padding
        assert_eq!(decode("Z!=="), None); // bad alphabet
        assert_eq!(decode("===="), None); // all padding
        assert_eq!(decode("Zg=a"), None); // padding then data
    }
}
