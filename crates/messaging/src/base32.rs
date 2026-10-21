//! Crockford base32, uppercase output (fits QR "alphanumeric" mode), case-insensitive input.

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buf >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buf << (5 - bits)) & 31) as usize]));
    }
    out
}

fn value(c: u8) -> Option<u32> {
    let c = c.to_ascii_uppercase();
    // Crockford aliases: O -> 0, I/L -> 1.
    let c = match c {
        b'O' => b'0',
        b'I' | b'L' => b'1',
        c => c,
    };
    ALPHABET
        .iter()
        .position(|&a| a == c)
        .and_then(|p| u32::try_from(p).ok())
}

pub(crate) fn decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        if c == b'-' || c.is_ascii_whitespace() {
            continue;
        }
        buf = (buf << 5) | value(c)?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buf >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn roundtrip() {
        for len in 0..40u8 {
            let data: Vec<u8> = (0..len).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(decode(&encode(&data)).unwrap(), data);
            assert_eq!(decode(&encode(&data).to_lowercase()).unwrap(), data);
        }
        assert!(decode("U").is_none());
    }
}
