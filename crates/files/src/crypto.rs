//! Per-file chunk encryption.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Random key of one file (sent only inside the end-to-end encrypted offer).
///
/// Chunk `i` is sealed with XChaCha20-Poly1305, nonce = `i` (u64 little-endian, zero padded),
/// associated data = `i` (u64 big-endian). Encryption is deterministic for a given key, so every
/// source produces the same ciphertext and the same BLAKE3 hash.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct FileKey([u8; 32]);

impl core::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("FileKey(..)")
    }
}

fn nonce(index: u32) -> XNonce {
    let mut n = [0u8; 24];
    n[..8].copy_from_slice(&u64::from(index).to_le_bytes());
    XNonce::from(n)
}

impl FileKey {
    /// New random key.
    ///
    /// # Errors
    ///
    /// Fails if the OS random generator fails.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut k = [0u8; 32];
        getrandom::fill(&mut k)?;
        Ok(Self(k))
    }

    /// Key from its bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&self.0.into())
    }

    /// Encrypts chunk `index`.
    #[must_use]
    pub fn seal(&self, index: u32, plaintext: &[u8]) -> Vec<u8> {
        let aad = u64::from(index).to_be_bytes();
        // Encryption with a valid key cannot fail.
        self.cipher()
            .encrypt(
                &nonce(index),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .unwrap_or_default()
    }

    /// Decrypts chunk `index`. Returns `None` if it was not produced with this key and index.
    #[must_use]
    pub fn open(&self, index: u32, ciphertext: &[u8]) -> Option<Vec<u8>> {
        let aad = u64::from(index).to_be_bytes();
        self.cipher()
            .decrypt(
                &nonce(index),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::FileKey;

    #[test]
    fn seal_open() {
        let k = FileKey::generate().unwrap();
        let c = k.seal(3, b"hello");
        assert_eq!(c, k.seal(3, b"hello"), "deterministic");
        assert_eq!(k.open(3, &c).unwrap(), b"hello");
        assert!(k.open(4, &c).is_none(), "bound to the index");
        assert!(FileKey::generate().unwrap().open(3, &c).is_none());
    }
}
