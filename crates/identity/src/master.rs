//! Master identity key.

use core::fmt;

use ed25519_dalek::Signer;
use zeroize::Zeroize;

use crate::{PublicKey, RecoveryPhrase};

/// Domain-separation context for deriving the master key from the BIP-39 seed.
const MASTER_KEY_CONTEXT: &str = "OutpostWave 2026-10 master identity key v1";

/// The master identity key. It only signs device certificates.
///
/// The private key is zeroized on drop.
pub struct MasterKey(ed25519_dalek::SigningKey);

impl MasterKey {
    /// Deterministically derives the master key from a recovery phrase.
    #[must_use]
    pub fn from_phrase(phrase: &RecoveryPhrase) -> Self {
        let mut seed = phrase.seed();
        let mut secret = blake3::derive_key(MASTER_KEY_CONTEXT, &seed);
        seed.zeroize();
        let key = ed25519_dalek::SigningKey::from_bytes(&secret);
        secret.zeroize();
        Self(key)
    }

    /// Public part of the master key: the user's long-term identity.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    /// Signs `message` (already domain-separated by the caller).
    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message).to_bytes()
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MasterKey({})", self.public_key())
    }
}

#[cfg(test)]
mod tests {
    use super::MasterKey;
    use crate::RecoveryPhrase;

    #[test]
    fn derivation_is_deterministic() {
        let p = RecoveryPhrase::generate().unwrap();
        let words = p.words().collect::<Vec<_>>().join(" ");
        let a = MasterKey::from_phrase(&p);
        let b = MasterKey::from_phrase(&RecoveryPhrase::parse(&words).unwrap());
        assert_eq!(a.public_key(), b.public_key());
    }

    #[test]
    fn known_vector_is_stable() {
        // Guards against accidental changes of the derivation (would lock users out).
        let words = ["abandon"; 23].join(" ") + " art";
        let key = MasterKey::from_phrase(&RecoveryPhrase::parse(&words).unwrap());
        assert_eq!(
            key.public_key().to_string(),
            "nv8vtmrn1yczd6kg5w5gjx0fbf7pr5697wny2jf463ycnstcg2s0"
        );
    }
}
