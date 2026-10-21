//! Public keys and fingerprints.

use core::fmt;

use crate::{IdentityError, base32};

/// Domain-separation context for fingerprints.
const FINGERPRINT_CONTEXT: &str = "OutpostWave 2026-10 key fingerprint v1";

/// An Ed25519 public key (master or device).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(pub(crate) ed25519_dalek::VerifyingKey);

impl PublicKey {
    /// Parses a public key from its 32-byte encoding.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidPublicKey`] if the bytes are not a valid point.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, IdentityError> {
        ed25519_dalek::VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|_| IdentityError::InvalidPublicKey)
    }

    /// Returns the 32-byte encoding.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Verifies a signature made over `message` with the matching private key.
    ///
    /// Uses strict verification (rejects weak keys and malleable signatures).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidSignature`] if verification fails.
    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<(), IdentityError> {
        let sig = ed25519_dalek::Signature::from_bytes(signature);
        self.0
            .verify_strict(message, &sig)
            .map_err(|_| IdentityError::InvalidSignature)
    }

    /// Returns the fingerprint of this key.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        let mut hasher = blake3::Hasher::new_derive_key(FINGERPRINT_CONTEXT);
        hasher.update(&self.to_bytes());
        Fingerprint(*hasher.finalize().as_bytes())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl fmt::Display for PublicKey {
    /// Crockford base32, 52 characters.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base32::encode(&self.to_bytes()))
    }
}

/// BLAKE3 fingerprint (32 bytes) of a public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub(crate) [u8; 32]);

impl Fingerprint {
    /// Raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base32::encode(&self.0))
    }
}
