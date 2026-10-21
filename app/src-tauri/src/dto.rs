//! Serializable types sent to the UI (camelCase JSON).

use ow_messaging::{
    ContactInfo, FileInfo, FileState, MessageStatus, Messenger, MessengerEvent, StoredMessage,
};
use serde::Serialize;

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn status(s: MessageStatus) -> &'static str {
    match s {
        MessageStatus::Pending => "pending",
        MessageStatus::Sent => "sent",
        MessageStatus::Delivered => "delivered",
        MessageStatus::Read => "read",
        MessageStatus::Received => "received",
    }
}

/// Parses a 32-byte id in hex.
pub(crate) fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn file_state(s: FileState) -> &'static str {
    match s {
        FileState::Offered => "offered",
        FileState::Downloading => "downloading",
        FileState::Paused => "paused",
        FileState::Waiting => "waiting",
        FileState::Complete => "complete",
        FileState::Failed => "failed",
    }
}

/// File transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    file_id: String,
    name: String,
    size: u64,
    outgoing: bool,
    state: &'static str,
    bytes: u64,
    rate: f64,
    /// A local file can be opened (complete download, or our original).
    openable: bool,
}

impl From<FileInfo> for FileDto {
    fn from(f: FileInfo) -> Self {
        Self {
            file_id: hex(&f.file_id),
            name: f.name,
            size: f.size,
            outgoing: f.outgoing,
            state: file_state(f.state),
            bytes: f.bytes,
            rate: f.rate,
            openable: f.path.is_some_and(|p| p.exists()),
        }
    }
}

/// Invite shown to the user.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteDto {
    /// Our handle.
    pub handle: String,
    /// `OW1:` code.
    pub code: String,
    /// QR code of the invite, as SVG markup.
    pub qr_svg: String,
}

/// Contact.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactDto {
    id: i64,
    handle: String,
    display_name: Option<String>,
    verified: bool,
    online: bool,
    safety_number: String,
    key_changed: bool,
    name_unconfirmed: bool,
    unread: u32,
}

impl From<ContactInfo> for ContactDto {
    fn from(c: ContactInfo) -> Self {
        Self {
            id: c.id.0,
            handle: c.handle,
            display_name: c.display_name,
            verified: c.verified,
            online: c.online,
            safety_number: c.safety_number,
            key_changed: c.key_changed,
            name_unconfirmed: c.name_unconfirmed,
            unread: c.unread,
        }
    }
}

/// Message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    id: i64,
    msg_id: String,
    outgoing: bool,
    sent_at: i64,
    status: &'static str,
    body: String,
    file: Option<FileDto>,
}

impl MessageDto {
    /// Message with its attached file, if any.
    pub fn new(m: &StoredMessage, messenger: &Messenger) -> Self {
        Self {
            id: m.id,
            msg_id: hex(&m.msg_id),
            outgoing: m.outgoing,
            sent_at: m.sent_at,
            status: status(m.status),
            body: m.body.clone(),
            file: m
                .file_id
                .and_then(|id| messenger.file(&id).ok())
                .map(FileDto::from),
        }
    }
}

/// Event pushed to the UI on the `ow-event` channel.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EventDto {
    /// New message.
    Message { contact: i64, message: MessageDto },
    /// Message status change.
    Status {
        contact: i64,
        msg_id: String,
        status: &'static str,
    },
    /// Contact list changed.
    Contacts,
    /// Contact presence.
    Presence { contact: i64, online: bool },
    /// A contact's keys changed.
    KeyChanged { contact: i64 },
    /// Someone unknown wrote to us.
    ContactRequest { contact: i64 },
    /// File transfer progress or state change.
    File { contact: i64, file: FileDto },
    /// Events were lost: reload everything.
    Resync,
}

impl EventDto {
    /// Converts a messenger event.
    pub fn new(e: MessengerEvent, messenger: &Messenger) -> Self {
        match e {
            MessengerEvent::Message { contact, message } => Self::Message {
                contact: contact.0,
                message: MessageDto::new(&message, messenger),
            },
            MessengerEvent::File { contact, file } => Self::File {
                contact: contact.0,
                file: FileDto::from(file),
            },
            MessengerEvent::Status {
                contact,
                msg_id,
                status: s,
            } => Self::Status {
                contact: contact.0,
                msg_id: hex(&msg_id),
                status: status(s),
            },
            MessengerEvent::ContactsChanged => Self::Contacts,
            MessengerEvent::KeyChanged { contact } => Self::KeyChanged { contact: contact.0 },
            MessengerEvent::ContactRequest { contact } => {
                Self::ContactRequest { contact: contact.0 }
            }
            MessengerEvent::Presence { contact, online } => Self::Presence {
                contact: contact.0,
                online,
            },
        }
    }
}
