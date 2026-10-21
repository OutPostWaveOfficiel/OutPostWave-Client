//! Data types stored in the database. Keys are raw bytes so this crate stays independent of
//! the identity and crypto crates.

use zeroize::{Zeroize, ZeroizeOnDrop};

/// This device's identity. Secrets are zeroized on drop.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct LocalIdentity {
    /// Chosen handle name (without the tag).
    pub handle_name: String,
    /// Device Ed25519 secret key.
    pub device_secret: [u8; 32],
    /// Device certificate (binary encoding from `ow-identity`).
    pub device_cert: Vec<u8>,
    /// Master secret key, kept only on devices allowed to certify new devices.
    pub master_secret: Option<[u8; 32]>,
    /// Creation time (Unix ms).
    pub created_at: i64,
}

impl core::fmt::Debug for LocalIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalIdentity")
            .field("handle_name", &self.handle_name)
            .field("has_master_secret", &self.master_secret.is_some())
            .finish_non_exhaustive()
    }
}

/// Row id of a contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContactId(pub i64);

/// Row id of a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConversationId(pub i64);

/// Contact to add.
#[derive(Debug, Clone)]
pub struct NewContact {
    /// Master public key.
    pub master_pub: [u8; 32],
    /// Full handle `name#tag`.
    pub handle: String,
    /// Optional local nickname.
    pub display_name: Option<String>,
    /// Time added (Unix ms).
    pub added_at: i64,
}

/// Stored contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// Row id.
    pub id: ContactId,
    /// Master public key.
    pub master_pub: [u8; 32],
    /// Full handle `name#tag`.
    pub handle: String,
    /// Optional local nickname.
    pub display_name: Option<String>,
    /// Safety number verified by the user.
    pub verified: bool,
    /// Time added (Unix ms).
    pub added_at: i64,
}

/// Delivery status of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessageStatus {
    /// Outgoing, not yet sent.
    Pending = 0,
    /// Sent to the network (peer or relay).
    Sent = 1,
    /// Acknowledged by the recipient's device.
    Delivered = 2,
    /// Read by the recipient.
    Read = 3,
    /// Incoming message.
    Received = 4,
}

impl MessageStatus {
    pub(crate) fn from_i64(v: i64) -> Option<Self> {
        Some(match v {
            0 => Self::Pending,
            1 => Self::Sent,
            2 => Self::Delivered,
            3 => Self::Read,
            4 => Self::Received,
            _ => return None,
        })
    }
}

/// Message to insert.
#[derive(Debug, Clone)]
pub struct NewMessage {
    /// Conversation.
    pub conversation: ConversationId,
    /// Random 128-bit message id (deduplication across devices and retries).
    pub msg_id: [u8; 16],
    /// Sender device public key.
    pub sender_device: [u8; 32],
    /// `true` if sent by this user.
    pub outgoing: bool,
    /// Sender timestamp (Unix ms).
    pub sent_at: i64,
    /// Local receive time (Unix ms), for incoming messages.
    pub received_at: Option<i64>,
    /// Initial status.
    pub status: MessageStatus,
    /// Text content.
    pub body: String,
}

/// Stored message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    /// Row id (stable local ordering key).
    pub id: i64,
    /// Conversation.
    pub conversation: ConversationId,
    /// Message id.
    pub msg_id: [u8; 16],
    /// Sender device public key.
    pub sender_device: [u8; 32],
    /// `true` if sent by this user.
    pub outgoing: bool,
    /// Sender timestamp (Unix ms).
    pub sent_at: i64,
    /// Local receive time (Unix ms).
    pub received_at: Option<i64>,
    /// Current status.
    pub status: MessageStatus,
    /// Text content.
    pub body: String,
}
