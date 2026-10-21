//! Plaintext frames carried inside an encrypted envelope.
//!
//! Every frame starts with `version u8 = 1 | kind u8`.
//!
//! Text (kind 1):
//!
//! ```text
//! version u8 = 1 | kind u8 | msg_id 16 | sent_at i64 BE (Unix ms) | flags u8
//! [intro, if flags & 1: name_len u8 | name | master_pub 32 | cert_len u16 BE | cert]
//! body (UTF-8, rest of the frame)
//! ```
//!
//! Read receipt (kind 2): `version | kind | count u16 BE | msg_id 16 × count`.
//!
//! File offer (kind 3): like text, with `file_id 32 | size u64 BE | file_key 32` before the body,
//! which holds the file name. See `spec/v0/files.md`.

use crate::MessagingError;

const VERSION: u8 = 1;
const KIND_TEXT: u8 = 1;
const KIND_RECEIPT: u8 = 2;
const KIND_FILE: u8 = 3;
/// Maximum number of message ids in one read receipt.
pub(crate) const MAX_RECEIPT_IDS: usize = 512;
const FLAG_INTRO: u8 = 1;
/// Maximum text length, in bytes.
pub const MAX_TEXT_LEN: usize = 16 * 1024;

/// Sender introduction: lets an unknown receiver verify and add the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Intro {
    pub name: String,
    pub master_pub: [u8; 32],
    pub cert: Vec<u8>,
}

/// File attached to a message (the body is the file name).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileRef {
    pub file_id: [u8; 32],
    pub size: u64,
    pub key: [u8; 32],
}

impl core::fmt::Debug for FileRef {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "FileRef({} bytes)", self.size)
    }
}

/// Text message frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextFrame {
    pub msg_id: [u8; 16],
    pub sent_at: i64,
    pub intro: Option<Intro>,
    pub file: Option<FileRef>,
    pub body: String,
}

impl TextFrame {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(27 + self.body.len());
        out.extend_from_slice(&[
            VERSION,
            if self.file.is_some() {
                KIND_FILE
            } else {
                KIND_TEXT
            },
        ]);
        out.extend_from_slice(&self.msg_id);
        out.extend_from_slice(&self.sent_at.to_be_bytes());
        match &self.intro {
            Some(i) => {
                out.push(FLAG_INTRO);
                out.push(u8::try_from(i.name.len()).unwrap_or(0));
                out.extend_from_slice(i.name.as_bytes());
                out.extend_from_slice(&i.master_pub);
                out.extend_from_slice(&u16::try_from(i.cert.len()).unwrap_or(0).to_be_bytes());
                out.extend_from_slice(&i.cert);
            }
            None => out.push(0),
        }
        if let Some(f) = &self.file {
            out.extend_from_slice(&f.file_id);
            out.extend_from_slice(&f.size.to_be_bytes());
            out.extend_from_slice(&f.key);
        }
        out.extend_from_slice(self.body.as_bytes());
        out
    }

    pub(crate) fn decode(b: &[u8]) -> Result<Self, MessagingError> {
        let bad = || MessagingError::Crypto(ow_crypto::CryptoError::Malformed);
        let take = |b: &mut &[u8], n: usize| -> Result<Vec<u8>, MessagingError> {
            if b.len() < n {
                return Err(bad());
            }
            let (h, t) = b.split_at(n);
            *b = t;
            Ok(h.to_vec())
        };
        let mut b = b;
        let head = take(&mut b, 2)?;
        if head != [VERSION, KIND_TEXT] && head != [VERSION, KIND_FILE] {
            return Err(bad());
        }
        let is_file = head[1] == KIND_FILE;
        let mut msg_id = [0u8; 16];
        msg_id.copy_from_slice(&take(&mut b, 16)?);
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&take(&mut b, 8)?);
        let flags = take(&mut b, 1)?[0];
        let intro = if flags & FLAG_INTRO != 0 {
            let name_len = usize::from(take(&mut b, 1)?[0]);
            let name = String::from_utf8(take(&mut b, name_len)?).map_err(|_| bad())?;
            let mut master_pub = [0u8; 32];
            master_pub.copy_from_slice(&take(&mut b, 32)?);
            let cl = take(&mut b, 2)?;
            let cert = take(&mut b, usize::from(u16::from_be_bytes([cl[0], cl[1]])))?;
            Some(Intro {
                name,
                master_pub,
                cert,
            })
        } else {
            None
        };
        let file = if is_file {
            let mut file_id = [0u8; 32];
            file_id.copy_from_slice(&take(&mut b, 32)?);
            let mut size = [0u8; 8];
            size.copy_from_slice(&take(&mut b, 8)?);
            let mut key = [0u8; 32];
            key.copy_from_slice(&take(&mut b, 32)?);
            Some(FileRef {
                file_id,
                size: u64::from_be_bytes(size),
                key,
            })
        } else {
            None
        };
        if b.len() > MAX_TEXT_LEN {
            return Err(bad());
        }
        let body = String::from_utf8(b.to_vec()).map_err(|_| bad())?;
        Ok(Self {
            msg_id,
            sent_at: i64::from_be_bytes(ts),
            intro,
            file,
            body,
        })
    }
}

/// Any frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    /// Text message.
    Text(TextFrame),
    /// The listed messages were read.
    Receipt(Vec<[u8; 16]>),
}

impl Frame {
    pub(crate) fn encode_receipt(ids: &[[u8; 16]]) -> Vec<u8> {
        let ids = &ids[..ids.len().min(MAX_RECEIPT_IDS)];
        let mut out = vec![VERSION, KIND_RECEIPT];
        out.extend_from_slice(&u16::try_from(ids.len()).unwrap_or(0).to_be_bytes());
        for id in ids {
            out.extend_from_slice(id);
        }
        out
    }

    pub(crate) fn decode(b: &[u8]) -> Result<Self, MessagingError> {
        let bad = || MessagingError::Crypto(ow_crypto::CryptoError::Malformed);
        match b {
            [VERSION, KIND_TEXT | KIND_FILE, ..] => TextFrame::decode(b).map(Self::Text),
            [VERSION, KIND_RECEIPT, c0, c1, rest @ ..] => {
                let count = usize::from(u16::from_be_bytes([*c0, *c1]));
                if count > MAX_RECEIPT_IDS || rest.len() != count * 16 {
                    return Err(bad());
                }
                Ok(Self::Receipt(
                    rest.chunks_exact(16)
                        .map(|c| {
                            let mut id = [0u8; 16];
                            id.copy_from_slice(c);
                            id
                        })
                        .collect(),
                ))
            }
            _ => Err(bad()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileRef, Frame, Intro, TextFrame};

    #[test]
    fn receipt_roundtrip() {
        let ids = vec![[1; 16], [2; 16]];
        assert_eq!(
            Frame::decode(&Frame::encode_receipt(&ids)).unwrap(),
            Frame::Receipt(ids)
        );
        assert!(Frame::decode(&[1, 2, 0, 1, 0]).is_err());
    }

    #[test]
    fn roundtrip() {
        for intro in [
            None,
            Some(Intro {
                name: "bob".into(),
                master_pub: [3; 32],
                cert: vec![9; 150],
            }),
        ] {
            for file in [
                None,
                Some(FileRef {
                    file_id: [4; 32],
                    size: 1 << 40,
                    key: [5; 32],
                }),
            ] {
                let f = TextFrame {
                    msg_id: [7; 16],
                    sent_at: -5,
                    intro: intro.clone(),
                    file,
                    body: "héllo 👋".into(),
                };
                assert_eq!(TextFrame::decode(&f.encode()).unwrap(), f);
            }
        }
        assert!(TextFrame::decode(&[1, 1, 0]).is_err());
    }
}
