//! Error type.

use core::fmt;

/// P2P errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2pError {
    /// The device key is not a valid Ed25519 secret.
    InvalidKey,
    /// The transport could not be built.
    Transport(String),
    /// Dialing the peer failed.
    Dial(String),
    /// Sending a message failed (peer unreachable, timeout, refused...).
    Send(String),
    /// The payload exceeds [`MAX_PAYLOAD`](crate::MAX_PAYLOAD).
    PayloadTooLarge,
    /// The node task has stopped.
    Stopped,
}

impl fmt::Display for P2pError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey => f.write_str("invalid device key"),
            Self::Transport(e) => write!(f, "transport error: {e}"),
            Self::Dial(e) => write!(f, "dial failed: {e}"),
            Self::Send(e) => write!(f, "send failed: {e}"),
            Self::PayloadTooLarge => f.write_str("payload too large"),
            Self::Stopped => f.write_str("p2p node stopped"),
        }
    }
}

impl std::error::Error for P2pError {}
