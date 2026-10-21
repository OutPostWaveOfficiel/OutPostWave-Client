//! Per-device Olm account.

use core::fmt;

use ow_identity::DeviceKey;
use vodozemac::Curve25519PublicKey;
use vodozemac::olm::{Account, AccountPickle, SessionConfig};

use crate::{CryptoError, KeyKind, PrekeyBundle};

/// Olm session configuration (v1: the interoperable, audited format).
pub(crate) fn session_config() -> SessionConfig {
    SessionConfig::version_1()
}

/// The Olm account of this device: Curve25519 identity key, one-time and fallback keys.
pub struct CryptoAccount {
    pub(crate) inner: Account,
}

impl CryptoAccount {
    /// Creates a new account with a fresh fallback key.
    #[must_use]
    pub fn new() -> Self {
        let mut inner = Account::new();
        inner.generate_fallback_key();
        Self { inner }
    }

    /// Olm identity key (Curve25519).
    #[must_use]
    pub fn identity_key(&self) -> [u8; 32] {
        self.inner.curve25519_key().to_bytes()
    }

    /// Bundle with the current fallback key, signed by `device`. Suitable for invite codes.
    #[must_use]
    pub fn fallback_bundle(&mut self, device: &DeviceKey, now: u64) -> PrekeyBundle {
        let key = match self.inner.fallback_key().values().next() {
            Some(k) => *k,
            None => {
                self.inner.generate_fallback_key();
                *self
                    .inner
                    .fallback_key()
                    .values()
                    .next()
                    .unwrap_or(&self.inner.curve25519_key())
            }
        };
        self.inner.mark_keys_as_published();
        PrekeyBundle::sign(
            device,
            self.identity_key(),
            key.to_bytes(),
            KeyKind::Fallback,
            now,
        )
    }

    /// Bundle with a fresh one-time key, signed by `device` (for peers asking online).
    #[must_use]
    pub fn one_time_bundle(&mut self, device: &DeviceKey, now: u64) -> PrekeyBundle {
        let result = self.inner.generate_one_time_keys(1);
        let key = result
            .created
            .first()
            .copied()
            .unwrap_or_else(|| self.inner.curve25519_key());
        self.inner.mark_keys_as_published();
        PrekeyBundle::sign(
            device,
            self.identity_key(),
            key.to_bytes(),
            KeyKind::OneTime,
            now,
        )
    }

    /// Replaces the fallback key (call periodically, e.g. weekly). The previous one keeps
    /// working until the next rotation, for messages already in flight.
    pub fn rotate_fallback_key(&mut self) {
        self.inner.generate_fallback_key();
    }

    /// Serializes the account, encrypted with `pickle_key`.
    #[must_use]
    pub fn pickle(&self, pickle_key: &[u8; 32]) -> String {
        self.inner.pickle().encrypt(pickle_key)
    }

    /// Restores an account.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Pickle`] if the data or key is wrong.
    pub fn from_pickle(pickled: &str, pickle_key: &[u8; 32]) -> Result<Self, CryptoError> {
        let p = AccountPickle::from_encrypted(pickled, pickle_key)
            .map_err(|e| CryptoError::Pickle(e.to_string()))?;
        Ok(Self {
            inner: Account::from_pickle(p),
        })
    }
}

impl Default for CryptoAccount {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CryptoAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CryptoAccount({})",
            self.inner.curve25519_key().to_base64()
        )
    }
}

pub(crate) fn curve(bytes: &[u8; 32]) -> Curve25519PublicKey {
    Curve25519PublicKey::from_bytes(*bytes)
}
