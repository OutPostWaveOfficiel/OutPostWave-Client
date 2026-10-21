//! Database encryption key.

use core::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::StorageError;

/// 256-bit database key. Zeroized on drop; `Debug` is redacted.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct DbKey([u8; 32]);

impl DbKey {
    /// Generates a new random key from the OS RNG.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Rng`] if the OS RNG is unavailable.
    pub fn generate() -> Result<Self, StorageError> {
        let mut k = [0u8; 32];
        getrandom::fill(&mut k).map_err(|_| StorageError::Rng)?;
        Ok(Self(k))
    }

    /// Wraps existing key bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Raw key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// `PRAGMA key` value in SQLCipher raw-key syntax: `x'<64 hex>'`. Callers must zeroize it.
    pub(crate) fn pragma_value(&self) -> String {
        let mut s = String::with_capacity(67);
        s.push_str("x'");
        for b in self.0 {
            s.push(char::from(HEX[usize::from(b >> 4)]));
            s.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
        s.push('\'');
        s
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey(<redacted>)")
    }
}
