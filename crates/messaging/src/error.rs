//! Error type.

use core::fmt;

/// Messaging errors.
#[derive(Debug)]
pub enum MessagingError {
    /// No identity has been created on this device yet.
    NoIdentity,
    /// An identity already exists.
    IdentityExists,
    /// The invite code is invalid.
    InvalidInvite,
    /// The invite is our own.
    OwnInvite,
    /// Unknown contact.
    UnknownContact,
    /// Message text is empty or too long.
    InvalidText,
    /// Backup error.
    Backup(&'static str),
    /// Unknown file transfer.
    UnknownFile,
    /// File system error (file transfers).
    Io(String),
    /// No account server configured.
    NoRegistry,
    /// Account server error.
    Registry(ow_registry::RegistryError),
    /// Identity error.
    Identity(ow_identity::IdentityError),
    /// Storage error.
    Storage(ow_storage::StorageError),
    /// Encryption error.
    Crypto(ow_crypto::CryptoError),
    /// Network error.
    P2p(ow_p2p::P2pError),
}

impl fmt::Display for MessagingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoIdentity => f.write_str("no identity on this device"),
            Self::IdentityExists => f.write_str("an identity already exists on this device"),
            Self::InvalidInvite => f.write_str("invalid invite code"),
            Self::OwnInvite => f.write_str("this is your own invite code"),
            Self::UnknownContact => f.write_str("unknown contact"),
            Self::InvalidText => f.write_str("message is empty or too long"),
            Self::Backup(e) => write!(f, "backup: {e}"),
            Self::UnknownFile => f.write_str("unknown file"),
            Self::Io(e) => write!(f, "file: {e}"),
            Self::NoRegistry => f.write_str("no account server configured"),
            Self::Registry(e) => write!(f, "{e}"),
            Self::Identity(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "{e}"),
            Self::Crypto(e) => write!(f, "{e}"),
            Self::P2p(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MessagingError {}

impl From<ow_identity::IdentityError> for MessagingError {
    fn from(e: ow_identity::IdentityError) -> Self {
        Self::Identity(e)
    }
}
impl From<ow_storage::StorageError> for MessagingError {
    fn from(e: ow_storage::StorageError) -> Self {
        Self::Storage(e)
    }
}
impl From<ow_crypto::CryptoError> for MessagingError {
    fn from(e: ow_crypto::CryptoError) -> Self {
        Self::Crypto(e)
    }
}
impl From<ow_p2p::P2pError> for MessagingError {
    fn from(e: ow_p2p::P2pError) -> Self {
        Self::P2p(e)
    }
}
impl From<ow_registry::RegistryError> for MessagingError {
    fn from(e: ow_registry::RegistryError) -> Self {
        Self::Registry(e)
    }
}
