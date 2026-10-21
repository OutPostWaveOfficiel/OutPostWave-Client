//! Encrypted local storage (SQLCipher).
//!
//! The whole database file is encrypted (AES-256, SQLCipher 4 defaults) with a random 32-byte
//! [`DbKey`]. The key is passed raw (no passphrase KDF) for a fast startup; it is kept in the OS
//! credential store (feature `os-keystore`), never next to the database.
//!
//! Privacy settings: `secure_delete` (deleted content is overwritten), temporary data in memory.

mod error;
mod key;
#[cfg(feature = "os-keystore")]
pub mod keystore;
mod migrations;
mod model;
mod store;

pub use error::StorageError;
pub use key::DbKey;
pub use model::{
    Contact, ContactId, ConversationId, LocalIdentity, MessageStatus, NewContact, NewMessage,
    StoredMessage,
};
pub use store::Storage;
