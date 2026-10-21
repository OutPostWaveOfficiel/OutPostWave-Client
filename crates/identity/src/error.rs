//! Error type.

use core::fmt;

/// Errors returned by identity operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The operating system RNG failed.
    Rng,
    /// The recovery phrase is invalid (unknown word, bad checksum, wrong length).
    InvalidPhrase,
    /// The bytes are not a valid Ed25519 public key.
    InvalidPublicKey,
    /// A signature did not verify.
    InvalidSignature,
    /// A binary structure is malformed or uses an unsupported version.
    Malformed,
    /// The device name is empty or too long.
    InvalidDeviceName,
    /// The handle name breaks the naming rules.
    InvalidName,
    /// The handle is not of the form `name#tag`.
    InvalidHandle,
    /// The handle tag does not match the public key.
    TagMismatch,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rng => "system random number generator failed",
            Self::InvalidPhrase => "invalid recovery phrase",
            Self::InvalidPublicKey => "invalid public key",
            Self::InvalidSignature => "invalid signature",
            Self::Malformed => "malformed data",
            Self::InvalidDeviceName => "invalid device name",
            Self::InvalidName => "invalid name",
            Self::InvalidHandle => "invalid handle, expected name#tag",
            Self::TagMismatch => "handle tag does not match the public key",
        })
    }
}

impl std::error::Error for IdentityError {}
