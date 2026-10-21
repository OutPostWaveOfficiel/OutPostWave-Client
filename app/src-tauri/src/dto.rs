//! Serializable types sent to the UI (camelCase JSON).

use ow_messaging::{ContactInfo, MessageStatus, MessengerEvent, StoredMessage};
use serde::Serialize;

fn hex(bytes: &[u8]) -> String {
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
}

impl From<&StoredMessage> for MessageDto {
    fn from(m: &StoredMessage) -> Self {
        Self {
            id: m.id,
            msg_id: hex(&m.msg_id),
            outgoing: m.outgoing,
            sent_at: m.sent_at,
            status: status(m.status),
            body: m.body.clone(),
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
    /// Events were lost: reload everything.
    Resync,
}

impl From<MessengerEvent> for EventDto {
    fn from(e: MessengerEvent) -> Self {
        match e {
            MessengerEvent::Message { contact, message } => Self::Message {
                contact: contact.0,
                message: MessageDto::from(&message),
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
