//! 24-word recovery phrase (BIP-39, English word list).

use core::fmt;

use zeroize::Zeroize;

use crate::IdentityError;

/// Number of words in a recovery phrase.
pub const WORD_COUNT: usize = 24;

/// A BIP-39 recovery phrase (256 bits of entropy).
///
/// It deterministically derives the [`MasterKey`](crate::MasterKey). It is the only way to
/// recover an identity: it must be shown to the user once and never stored in clear or logged.
/// `Debug` is redacted on purpose and there is no `Display`.
pub struct RecoveryPhrase(bip39::Mnemonic);

impl RecoveryPhrase {
    /// Generates a new random phrase from the OS RNG.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Rng`] if the OS RNG is unavailable.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut entropy = [0u8; 32];
        getrandom::fill(&mut entropy).map_err(|_| IdentityError::Rng)?;
        let mnemonic = bip39::Mnemonic::from_entropy(&entropy);
        entropy.zeroize();
        mnemonic.map(Self).map_err(|_| IdentityError::InvalidPhrase)
    }

    /// Parses a phrase typed by the user (case and extra spaces are tolerated).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidPhrase`] on unknown words, wrong word count or bad
    /// checksum.
    pub fn parse(input: &str) -> Result<Self, IdentityError> {
        let mut normalized = input
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ");
        let mnemonic = bip39::Mnemonic::parse_normalized(&normalized);
        normalized.zeroize();
        match mnemonic {
            Ok(m) if m.word_count() == WORD_COUNT => Ok(Self(m)),
            _ => Err(IdentityError::InvalidPhrase),
        }
    }

    /// The words, in order, for display to the user.
    pub fn words(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.words()
    }

    /// 64-byte BIP-39 seed (empty passphrase). Callers must zeroize it.
    pub(crate) fn seed(&self) -> [u8; 64] {
        self.0.to_seed("")
    }
}

impl fmt::Debug for RecoveryPhrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryPhrase(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::{RecoveryPhrase, WORD_COUNT};

    #[test]
    fn generate_has_24_words_and_reparses() {
        let p = RecoveryPhrase::generate().unwrap();
        let words: Vec<_> = p.words().collect();
        assert_eq!(words.len(), WORD_COUNT);
        let typed = format!("  {}  ", words.join("   ").to_uppercase());
        let q = RecoveryPhrase::parse(&typed).unwrap();
        assert_eq!(q.words().collect::<Vec<_>>(), words);
    }

    #[test]
    fn rejects_bad_phrases() {
        assert!(RecoveryPhrase::parse("").is_err());
        // Valid 12-word phrase: wrong length for OutpostWave.
        let twelve = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert!(RecoveryPhrase::parse(twelve).is_err());
        // 24 words with a bad checksum.
        let bad = ["abandon"; 24].join(" ");
        assert!(RecoveryPhrase::parse(&bad).is_err());
    }

    #[test]
    fn debug_is_redacted() {
        let p = RecoveryPhrase::generate().unwrap();
        let first = p.words().next().unwrap();
        assert!(!format!("{p:?}").contains(first));
    }
}
