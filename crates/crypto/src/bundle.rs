//! Signed prekey bundle: binds a device's Olm keys to its device key.

use ow_identity::{DeviceKey, PublicKey};

use crate::CryptoError;

const MAGIC: &[u8; 4] = b"OWPK";
const VERSION: u8 = 1;
const SIGNING_CONTEXT: &[u8] = b"OutpostWave prekey bundle v1\0";
/// Encoded size of a bundle, in bytes.
pub const BUNDLE_LEN: usize = 4 + 1 + 32 + 32 + 32 + 1 + 8 + 64;

/// Kind of the prekey included in a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// Long-lived fallback key (reusable, rotated periodically). Used in invite codes.
    Fallback = 0,
    /// Single-use one-time key (best forward secrecy).
    OneTime = 1,
}

/// A device's Olm identity key and one prekey, signed by the device key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrekeyBundle {
    device: PublicKey,
    identity_key: [u8; 32],
    prekey: [u8; 32],
    kind: KeyKind,
    created_at: u64,
    signature: [u8; 64],
}

impl PrekeyBundle {
    pub(crate) fn sign(
        device: &DeviceKey,
        identity_key: [u8; 32],
        prekey: [u8; 32],
        kind: KeyKind,
        created_at: u64,
    ) -> Self {
        let mut b = Self {
            device: device.public_key(),
            identity_key,
            prekey,
            kind,
            created_at,
            signature: [0; 64],
        };
        b.signature = device.sign(&b.signed_message());
        b
    }

    /// Device that published the bundle.
    #[must_use]
    pub const fn device(&self) -> &PublicKey {
        &self.device
    }

    /// Olm identity key (Curve25519).
    #[must_use]
    pub const fn identity_key(&self) -> &[u8; 32] {
        &self.identity_key
    }

    /// Prekey (Curve25519).
    #[must_use]
    pub const fn prekey(&self) -> &[u8; 32] {
        &self.prekey
    }

    /// Prekey kind.
    #[must_use]
    pub const fn kind(&self) -> KeyKind {
        self.kind
    }

    /// Creation time (Unix seconds).
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Encodes the bundle (`BUNDLE_LEN` bytes).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.unsigned_bytes();
        out.extend_from_slice(&self.signature);
        out
    }

    /// Decodes a bundle and checks that it is signed by `expected_device`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Malformed`], [`CryptoError::WrongDevice`] or
    /// [`CryptoError::InvalidSignature`].
    pub fn from_bytes(bytes: &[u8], expected_device: &PublicKey) -> Result<Self, CryptoError> {
        if bytes.len() != BUNDLE_LEN || &bytes[..4] != MAGIC || bytes[4] != VERSION {
            return Err(CryptoError::Malformed);
        }
        let arr = |r: core::ops::Range<usize>| -> [u8; 32] {
            let mut a = [0u8; 32];
            a.copy_from_slice(&bytes[r]);
            a
        };
        let device = PublicKey::from_bytes(&arr(5..37)).map_err(|_| CryptoError::Malformed)?;
        if &device != expected_device {
            return Err(CryptoError::WrongDevice);
        }
        let kind = match bytes[101] {
            0 => KeyKind::Fallback,
            1 => KeyKind::OneTime,
            _ => return Err(CryptoError::Malformed),
        };
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&bytes[102..110]);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&bytes[110..174]);
        let b = Self {
            device,
            identity_key: arr(37..69),
            prekey: arr(69..101),
            kind,
            created_at: u64::from_be_bytes(ts),
            signature,
        };
        b.device
            .verify(&b.signed_message(), &b.signature)
            .map_err(|_| CryptoError::InvalidSignature)?;
        Ok(b)
    }

    fn unsigned_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(BUNDLE_LEN);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.device.to_bytes());
        out.extend_from_slice(&self.identity_key);
        out.extend_from_slice(&self.prekey);
        out.push(self.kind as u8);
        out.extend_from_slice(&self.created_at.to_be_bytes());
        out
    }

    fn signed_message(&self) -> Vec<u8> {
        let mut m = SIGNING_CONTEXT.to_vec();
        m.extend_from_slice(&self.unsigned_bytes());
        m
    }
}
