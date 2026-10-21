//! Set of chunk indices (received, or available on a source).

/// Fixed-size bit set, one bit per chunk (bit `i` = byte `i / 8`, LSB first).
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    bits: Vec<u8>,
    len: u32,
    ones: u32,
}

impl core::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Bitmap({}/{})", self.ones, self.len)
    }
}

impl Bitmap {
    /// Empty set for `len` chunks.
    #[must_use]
    pub fn new(len: u32) -> Self {
        Self {
            bits: vec![0; (len as usize).div_ceil(8)],
            len,
            ones: 0,
        }
    }

    /// Full set for `len` chunks.
    #[must_use]
    pub fn full(len: u32) -> Self {
        let mut b = Self::new(len);
        for i in 0..len {
            b.set(i);
        }
        b
    }

    /// Decodes a bitmap of `len` chunks. Returns `None` if the size does not match or bits beyond
    /// `len` are set.
    #[must_use]
    pub fn from_bytes(bytes: &[u8], len: u32) -> Option<Self> {
        if bytes.len() != (len as usize).div_ceil(8) {
            return None;
        }
        let extra = (bytes.len() * 8) as u64 - u64::from(len);
        if extra > 0 {
            let last = *bytes.last()?;
            if last >> (8 - extra) != 0 {
                return None;
            }
        }
        let ones = bytes.iter().map(|b| b.count_ones()).sum();
        Some(Self {
            bits: bytes.to_vec(),
            len,
            ones,
        })
    }

    /// Encoded form.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    /// Number of chunks.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// No chunk at all (empty file).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of set bits.
    #[must_use]
    pub const fn count(&self) -> u32 {
        self.ones
    }

    /// All bits set.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.ones == self.len
    }

    /// Whether chunk `i` is in the set.
    #[must_use]
    pub fn get(&self, i: u32) -> bool {
        i < self.len && self.bits[(i / 8) as usize] & (1 << (i % 8)) != 0
    }

    /// Removes chunk `i`. Returns `false` if it was not there.
    pub fn clear(&mut self, i: u32) -> bool {
        if !self.get(i) {
            return false;
        }
        self.bits[(i / 8) as usize] &= !(1 << (i % 8));
        self.ones -= 1;
        true
    }

    /// Adds chunk `i`. Returns `false` if it was already there or out of range.
    pub fn set(&mut self, i: u32) -> bool {
        if i >= self.len || self.get(i) {
            return false;
        }
        self.bits[(i / 8) as usize] |= 1 << (i % 8);
        self.ones += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Bitmap;

    #[test]
    fn roundtrip() {
        let mut b = Bitmap::new(11);
        assert!(b.set(0));
        assert!(b.set(10));
        assert!(!b.set(10));
        assert!(!b.set(11));
        assert!(b.clear(0) && !b.clear(0) && b.set(0));
        assert_eq!(b.count(), 2);
        let c = Bitmap::from_bytes(b.as_bytes(), 11).unwrap();
        assert_eq!(b, c);
        assert!(c.get(10) && !c.get(5));
        assert!(
            Bitmap::from_bytes(&[0, 0b1000_0000], 11).is_none(),
            "bit beyond len"
        );
        assert!(Bitmap::from_bytes(&[0], 11).is_none());
        assert!(Bitmap::full(11).is_full());
        assert!(Bitmap::new(0).is_full());
    }
}
