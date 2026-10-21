//! Wire format of an encrypted direct message.
//!
//! ```text
//! version u8 = 1 | kind u8 | sender device key 32 | [sender bundle BUNDLE_LEN, prekey only] | ciphertext
//! ```
//!
//! A prekey envelope carries the sender's signed bundle so the receiver can check that the Olm
//! identity key that set up the session belongs to the sender's device.

use ow_identity::PublicKey;

use crate::{BUNDLE_LEN, CryptoError, PrekeyBundle};

const VERSION: u8 = 1;

/// Envelope kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeKind {
    /// First message(s) of a session: sets it up (Olm pre-key message).
    PreKey = 0,
    /// Regular message of an established session.
    Normal = 1,
}

/// Encrypted message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub(crate) kind: EnvelopeKind,
    pub(crate) sender: PublicKey,
    pub(crate) sender_bundle: Option<PrekeyBundle>,
    pub(crate) ciphertext: Vec<u8>,
}

impl Envelope {
    /// Envelope kind.
    #[must_use]
    pub const fn kind(&self) -> EnvelopeKind {
        self.kind
    }

    /// Sender device key, as claimed. The transport (libp2p peer id) must confirm it.
    #[must_use]
    pub const fn sender(&self) -> &PublicKey {
        &self.sender
    }

    /// Encodes the envelope.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + 32 + BUNDLE_LEN + self.ciphertext.len());
        out.push(VERSION);
        out.push(self.kind as u8);
        out.extend_from_slice(&self.sender.to_bytes());
        if let Some(b) = &self.sender_bundle {
            out.extend_from_slice(&b.to_bytes());
        }
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Decodes an envelope. For pre-key envelopes, checks the sender bundle signature.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Malformed`] or a bundle verification error.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() < 2 + 32 + 1 || bytes[0] != VERSION {
            return Err(CryptoError::Malformed);
        }
        let mut s = [0u8; 32];
        s.copy_from_slice(&bytes[2..34]);
        let sender = PublicKey::from_bytes(&s).map_err(|_| CryptoError::Malformed)?;
        let (kind, sender_bundle, rest) = match bytes[1] {
            0 => {
                let end = 34 + BUNDLE_LEN;
                if bytes.len() <= end {
                    return Err(CryptoError::Malformed);
                }
                let b = PrekeyBundle::from_bytes(&bytes[34..end], &sender)?;
                (EnvelopeKind::PreKey, Some(b), &bytes[end..])
            }
            1 => (EnvelopeKind::Normal, None, &bytes[34..]),
            _ => return Err(CryptoError::Malformed),
        };
        Ok(Self {
            kind,
            sender,
            sender_bundle,
            ciphertext: rest.to_vec(),
        })
    }
}
