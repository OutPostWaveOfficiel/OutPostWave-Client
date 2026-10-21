//! Error type.

use core::fmt;

/// Encryption errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// Malformed or unsupported binary data.
    Malformed,
    /// A signature or key binding did not verify.
    InvalidSignature,
    /// The bundle or envelope belongs to another device than expected.
    WrongDevice,
    /// No session can decrypt this message (unknown session, replay or tampering).
    Decryption(String),
    /// Session creation failed.
    Session(String),
    /// Encryption failed.
    Encryption(String),
    /// Stored state could not be restored.
    Pickle(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("malformed data"),
            Self::InvalidSignature => f.write_str("invalid signature"),
            Self::WrongDevice => f.write_str("data belongs to another device"),
            Self::Decryption(e) => write!(f, "decryption failed: {e}"),
            Self::Session(e) => write!(f, "session creation failed: {e}"),
            Self::Encryption(e) => write!(f, "encryption failed: {e}"),
            Self::Pickle(e) => write!(f, "cannot restore crypto state: {e}"),
        }
    }
}

impl std::error::Error for CryptoError {}
