//! Database key in the OS credential store.
//!
//! Windows: Credential Manager. macOS / iOS: Keychain. Linux: Secret Service (GNOME Keyring,
//! KWallet). Android support will use the Android Keystore (later step).

use zeroize::Zeroize;

use crate::{DbKey, StorageError};

/// Credential service name.
const SERVICE: &str = "app.outpostwave.client";

fn entry(profile: &str) -> Result<keyring::Entry, StorageError> {
    keyring::Entry::new(SERVICE, &format!("db-key:{profile}"))
        .map_err(|e| StorageError::Keystore(e.to_string()))
}

/// Loads the database key of `profile`, or creates and stores a new one.
///
/// # Errors
///
/// Returns [`StorageError::Keystore`] if the credential store is unavailable.
pub fn load_or_create(profile: &str) -> Result<DbKey, StorageError> {
    let entry = entry(profile)?;
    match entry.get_secret() {
        Ok(mut bytes) => {
            let key = <[u8; 32]>::try_from(bytes.as_slice())
                .map(DbKey::from_bytes)
                .map_err(|_| StorageError::InvalidData("database key length"));
            bytes.zeroize();
            key
        }
        Err(keyring::Error::NoEntry) => {
            let key = DbKey::generate()?;
            entry
                .set_secret(key.as_bytes())
                .map_err(|e| StorageError::Keystore(e.to_string()))?;
            Ok(key)
        }
        Err(e) => Err(StorageError::Keystore(e.to_string())),
    }
}

/// Deletes the database key of `profile`. The database becomes unreadable forever.
///
/// # Errors
///
/// Returns [`StorageError::Keystore`] on credential store errors.
pub fn delete(profile: &str) -> Result<(), StorageError> {
    match entry(profile)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(StorageError::Keystore(e.to_string())),
    }
}
