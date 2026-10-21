//! Direct messaging: glues identity, encrypted storage, end-to-end encryption and P2P transport.
//!
//! Flow of a message:
//!
//! ```text
//! text ─▶ frame (wire.rs) ─▶ Olm envelope (ow-crypto) ─▶ /outpostwave/dm/1 (ow-p2p) ─▶ peer
//!                                                                     ◀── "ok" acknowledgement
//! ```
//!
//! Contacts are added with an **invite code** (`OW1:…`, also shown as a QR code) carrying the
//! handle, master key, device certificate, a signed prekey bundle and network addresses. The
//! first messages to a new contact carry an *introduction* (handle + device certificate) so the
//! receiver can add the sender back.
//!
//! Messages to an offline contact stay `Pending` and are re-sent when the contact's device comes
//! online (discovered or connected).

mod backup;
mod base32;
mod error;
mod invite;
mod messenger;
mod wire;

pub use backup::MIN_PASSWORD_LEN;
pub use error::MessagingError;
pub use invite::Invite;
pub use messenger::{
    AUTO_DOWNLOAD_MAX, ContactInfo, FileInfo, Messenger, MessengerEvent, NewProfile, ProfileSetup,
};
pub use ow_identity::RecoveryPhrase;
pub use ow_registry::{AccountInfo, RegistryClient, RegistryError};
pub use ow_storage::{ContactId, FileState, MessageStatus, StoredMessage};
pub use wire::MAX_TEXT_LEN;
