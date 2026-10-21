//! Encrypted backups (contacts, history, settings), stored on the account registry.
//!
//! The registry only sees ciphertext. The key is derived from the user's password with
//! **Argon2id** (64 MiB, 3 passes) and the content is sealed with **XChaCha20-Poly1305**.
//!
//! ```text
//! "OWBK" | version u8 = 1 | salt 16 | m_cost u32 BE | t_cost u32 BE | p_cost u32 BE | nonce 24
//! | ciphertext (JSON of BackupData, see below)
//! ```

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::MessagingError;

const MAGIC: &[u8; 4] = b"OWBK";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 4 + 1 + 16 + 12 + 24;
/// Minimum password length.
pub const MIN_PASSWORD_LEN: usize = 10;
const M_COST: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// Key derived from the backup password, with its derivation parameters.
pub(crate) struct BackupKey {
    salt: [u8; 16],
    params: [u32; 3],
    key: Zeroizing<[u8; 32]>,
}

fn bad() -> MessagingError {
    MessagingError::Backup("invalid or corrupted backup")
}

impl BackupKey {
    fn derive(password: &str, salt: [u8; 16], params: [u32; 3]) -> Result<Self, MessagingError> {
        let p = Params::new(params[0], params[1], params[2], Some(32)).map_err(|_| bad())?;
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, p)
            .hash_password_into(password.as_bytes(), &salt, key.as_mut())
            .map_err(|_| bad())?;
        Ok(Self { salt, params, key })
    }

    /// New key with a fresh salt (slow: ~0.5 s). Checks the password length.
    pub(crate) fn new(password: &str) -> Result<Self, MessagingError> {
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(MessagingError::Backup("password too short"));
        }
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|_| MessagingError::Backup("RNG failure"))?;
        Self::derive(password, salt, [M_COST, T_COST, P_COST])
    }

    /// Local storage form (kept in the encrypted database): `salt | params | key`.
    pub(crate) fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(16 + 12 + 32));
        out.extend_from_slice(&self.salt);
        for p in self.params {
            out.extend_from_slice(&p.to_be_bytes());
        }
        out.extend_from_slice(self.key.as_ref());
        out
    }

    pub(crate) fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != 60 {
            return None;
        }
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&b[..16]);
        let u = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&b[28..60]);
        Some(Self {
            salt,
            params: [u(16), u(20), u(24)],
            key,
        })
    }

    /// Encrypts a backup.
    pub(crate) fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, MessagingError> {
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|_| MessagingError::Backup("RNG failure"))?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_ref()).map_err(|_| bad())?;
        let ct = cipher
            .encrypt(&XNonce::from(nonce), plaintext)
            .map_err(|_| bad())?;
        let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.salt);
        for p in self.params {
            out.extend_from_slice(&p.to_be_bytes());
        }
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypts a backup with the password (slow: re-derives the key).
    pub(crate) fn open(
        password: &str,
        blob: &[u8],
    ) -> Result<(Self, Zeroizing<Vec<u8>>), MessagingError> {
        if blob.len() < HEADER_LEN || &blob[..4] != MAGIC || blob[4] != VERSION {
            return Err(bad());
        }
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&blob[5..21]);
        let u = |i: usize| u32::from_be_bytes([blob[i], blob[i + 1], blob[i + 2], blob[i + 3]]);
        let params = [u(21), u(25), u(29)];
        // Refuse absurd parameters (a malicious server could try to exhaust memory or CPU).
        if params[0] > 1024 * 1024 || params[1] > 16 || params[2] > 16 {
            return Err(bad());
        }
        let key = Self::derive(password, salt, params)?;
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&blob[33..57]);
        let cipher = XChaCha20Poly1305::new_from_slice(key.key.as_ref()).map_err(|_| bad())?;
        let plain = cipher
            .decrypt(&XNonce::from(nonce), &blob[HEADER_LEN..])
            .map_err(|_| MessagingError::Backup("wrong password"))?;
        Ok((key, Zeroizing::new(plain)))
    }
}

/// Backup content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupData {
    pub version: u32,
    pub created_at: i64,
    pub read_receipts: bool,
    pub contacts: Vec<BackupContact>,
}

/// A contact with its devices and history. Binary values are base64.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupContact {
    pub handle: String,
    pub master_pub: String,
    pub verified: bool,
    pub devices: Vec<BackupDevice>,
    pub messages: Vec<BackupMessage>,
}

/// A contact device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupDevice {
    pub cert: String,
    pub bundle: Option<String>,
}

/// A message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupMessage {
    pub msg_id: String,
    pub sender_device: String,
    pub outgoing: bool,
    pub sent_at: i64,
    pub received_at: Option<i64>,
    pub status: u8,
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::BackupKey;

    #[test]
    fn seal_open_roundtrip() {
        let key = BackupKey::new("correct horse battery").unwrap();
        let blob = key.seal(b"secret content").unwrap();
        let (k2, plain) = BackupKey::open("correct horse battery", &blob).unwrap();
        assert_eq!(&plain[..], b"secret content");
        // Re-sealing with the restored key works and stays decryptable.
        let blob2 = k2.seal(b"v2").unwrap();
        assert_eq!(
            &BackupKey::open("correct horse battery", &blob2).unwrap().1[..],
            b"v2"
        );
        // Local storage form.
        let k3 = BackupKey::from_bytes(&key.to_bytes()).unwrap();
        assert_eq!(
            &BackupKey::open("correct horse battery", &k3.seal(b"x").unwrap())
                .unwrap()
                .1[..],
            b"x"
        );
    }

    #[test]
    fn wrong_password_and_tampering() {
        let key = BackupKey::new("correct horse battery").unwrap();
        let blob = key.seal(b"secret").unwrap();
        assert!(BackupKey::open("wrong password!!", &blob).is_err());
        let mut t = blob.clone();
        let last = t.len() - 1;
        t[last] ^= 1;
        assert!(BackupKey::open("correct horse battery", &t).is_err());
        assert!(BackupKey::new("short").is_err());
    }
}
