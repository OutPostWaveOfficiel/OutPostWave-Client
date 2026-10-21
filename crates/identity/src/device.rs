//! Device keys and certificates (multi-device support).

use core::fmt;

use ed25519_dalek::Signer;
use zeroize::Zeroize;

use crate::{IdentityError, MasterKey, PublicKey};

/// Maximum length of a device name, in bytes (UTF-8).
pub const MAX_DEVICE_NAME_LEN: usize = 64;

/// Magic bytes and format version of the binary certificate encoding.
const MAGIC: &[u8; 4] = b"OWDC";
const VERSION: u8 = 1;
/// Domain separation for certificate signatures.
const SIGNING_CONTEXT: &[u8] = b"OutpostWave device certificate v1\0";

/// A device's own Ed25519 key. Generated randomly on each device; zeroized on drop.
pub struct DeviceKey(ed25519_dalek::SigningKey);

impl DeviceKey {
    /// Generates a new random device key from the OS RNG.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Rng`] if the OS RNG is unavailable.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|_| IdentityError::Rng)?;
        let key = ed25519_dalek::SigningKey::from_bytes(&secret);
        secret.zeroize();
        Ok(Self(key))
    }

    /// Restores a device key from its 32-byte secret (loaded from encrypted storage).
    #[must_use]
    pub fn from_secret_bytes(secret: &[u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(secret))
    }

    /// Returns the 32-byte secret, for encrypted storage only. Callers must zeroize it.
    #[must_use]
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Public key of this device.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    /// Signs a message with the device key.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message).to_bytes()
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceKey({})", self.public_key())
    }
}

/// Statement, signed by the master key, that a device key belongs to the identity.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCertificate {
    master: PublicKey,
    device: PublicKey,
    name: String,
    created_at: u64,
    signature: [u8; 64],
}

impl DeviceCertificate {
    /// Certifies `device` with `master`.
    ///
    /// `created_at` is a Unix timestamp in seconds.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidDeviceName`] if `name` is empty, too long or contains
    /// control characters.
    pub fn issue(
        master: &MasterKey,
        device: &PublicKey,
        name: &str,
        created_at: u64,
    ) -> Result<Self, IdentityError> {
        validate_device_name(name)?;
        let mut cert = Self {
            master: master.public_key(),
            device: *device,
            name: name.to_owned(),
            created_at,
            signature: [0; 64],
        };
        cert.signature = master.sign(&cert.signed_message());
        Ok(cert)
    }

    /// Master public key (the identity).
    #[must_use]
    pub const fn master(&self) -> &PublicKey {
        &self.master
    }

    /// Certified device public key.
    #[must_use]
    pub const fn device(&self) -> &PublicKey {
        &self.device
    }

    /// Human-readable device name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Creation time (Unix seconds).
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Checks the master signature.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidSignature`] if the certificate was not signed by its
    /// master key.
    pub fn verify(&self) -> Result<(), IdentityError> {
        self.master.verify(&self.signed_message(), &self.signature)
    }

    /// Canonical binary encoding:
    /// `"OWDC" | version u8 | master 32 | device 32 | created_at u64 BE | name_len u8 | name | sig 64`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.unsigned_bytes();
        out.extend_from_slice(&self.signature);
        out
    }

    /// Decodes and **verifies** a certificate.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Malformed`] on bad encoding, or a key/signature error.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, IdentityError> {
        let mut r = Reader(bytes);
        if r.take(4)? != MAGIC || r.take(1)? != [VERSION] {
            return Err(IdentityError::Malformed);
        }
        let master = PublicKey::from_bytes(&r.array::<32>()?)?;
        let device = PublicKey::from_bytes(&r.array::<32>()?)?;
        let created_at = u64::from_be_bytes(r.array::<8>()?);
        let name_len = usize::from(r.take(1)?[0]);
        let name = core::str::from_utf8(r.take(name_len)?)
            .map_err(|_| IdentityError::Malformed)?
            .to_owned();
        validate_device_name(&name)?;
        let signature = r.array::<64>()?;
        if !r.0.is_empty() {
            return Err(IdentityError::Malformed);
        }
        let cert = Self {
            master,
            device,
            name,
            created_at,
            signature,
        };
        cert.verify()?;
        Ok(cert)
    }

    fn unsigned_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 1 + 32 + 32 + 8 + 1 + self.name.len() + 64);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.master.to_bytes());
        out.extend_from_slice(&self.device.to_bytes());
        out.extend_from_slice(&self.created_at.to_be_bytes());
        // Length is bounded by validate_device_name (<= 64).
        out.push(u8::try_from(self.name.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(self.name.as_bytes());
        out
    }

    fn signed_message(&self) -> Vec<u8> {
        let mut msg = SIGNING_CONTEXT.to_vec();
        msg.extend_from_slice(&self.unsigned_bytes());
        msg
    }
}

impl fmt::Debug for DeviceCertificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCertificate")
            .field("master", &self.master)
            .field("device", &self.device)
            .field("name", &self.name)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

fn validate_device_name(name: &str) -> Result<(), IdentityError> {
    if name.trim().is_empty()
        || name.len() > MAX_DEVICE_NAME_LEN
        || name.chars().any(char::is_control)
    {
        return Err(IdentityError::InvalidDeviceName);
    }
    Ok(())
}

/// Minimal bounds-checked byte reader.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], IdentityError> {
        if self.0.len() < n {
            return Err(IdentityError::Malformed);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], IdentityError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceCertificate, DeviceKey};
    use crate::{IdentityError, MasterKey, RecoveryPhrase};

    fn setup() -> (MasterKey, DeviceKey) {
        let master = MasterKey::from_phrase(&RecoveryPhrase::generate().unwrap());
        (master, DeviceKey::generate().unwrap())
    }

    #[test]
    fn issue_encode_decode_verify() {
        let (master, device) = setup();
        let cert =
            DeviceCertificate::issue(&master, &device.public_key(), "PC de Jean", 1_792_000_000)
                .unwrap();
        cert.verify().unwrap();
        let back = DeviceCertificate::from_bytes(&cert.to_bytes()).unwrap();
        assert_eq!(back, cert);
        assert_eq!(back.name(), "PC de Jean");
    }

    #[test]
    fn tampering_is_detected() {
        let (master, device) = setup();
        let cert = DeviceCertificate::issue(&master, &device.public_key(), "phone", 1).unwrap();
        let bytes = cert.to_bytes();
        // Flip one bit in every position: decoding must never succeed.
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 1;
            assert!(DeviceCertificate::from_bytes(&b).is_err(), "byte {i}");
        }
        assert_eq!(
            DeviceCertificate::from_bytes(&bytes[..bytes.len() - 1]),
            Err(IdentityError::Malformed)
        );
    }

    #[test]
    fn rejects_bad_names() {
        let (master, device) = setup();
        for name in ["", "   ", "a\nb", &"x".repeat(65)] {
            assert_eq!(
                DeviceCertificate::issue(&master, &device.public_key(), name, 0),
                Err(IdentityError::InvalidDeviceName)
            );
        }
    }

    #[test]
    fn device_key_roundtrip() {
        let k = DeviceKey::generate().unwrap();
        let k2 = DeviceKey::from_secret_bytes(&k.to_secret_bytes());
        assert_eq!(k.public_key(), k2.public_key());
        let sig = k.sign(b"hi");
        k2.public_key().verify(b"hi", &sig).unwrap();
    }
}
