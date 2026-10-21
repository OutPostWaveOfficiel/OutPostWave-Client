//! Plaintext frame carried inside an encrypted envelope.
//!
//! ```text
//! version u8 = 1 | kind u8 | msg_id 16 | sent_at i64 BE (Unix ms) | flags u8
//! [intro, if flags & 1: name_len u8 | name | master_pub 32 | cert_len u16 BE | cert]
//! body (UTF-8, rest of the frame)
//! ```

use crate::MessagingError;

const VERSION: u8 = 1;
const KIND_TEXT: u8 = 1;
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

/// Text message frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextFrame {
    pub msg_id: [u8; 16],
    pub sent_at: i64,
    pub intro: Option<Intro>,
    pub body: String,
}

impl TextFrame {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(27 + self.body.len());
        out.extend_from_slice(&[VERSION, KIND_TEXT]);
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
        if head != [VERSION, KIND_TEXT] {
            return Err(bad());
        }
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
        if b.len() > MAX_TEXT_LEN {
            return Err(bad());
        }
        let body = String::from_utf8(b.to_vec()).map_err(|_| bad())?;
        Ok(Self {
            msg_id,
            sent_at: i64::from_be_bytes(ts),
            intro,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Intro, TextFrame};

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
            let f = TextFrame {
                msg_id: [7; 16],
                sent_at: -5,
                intro,
                body: "héllo 👋".into(),
            };
            assert_eq!(TextFrame::decode(&f.encode()).unwrap(), f);
        }
        assert!(TextFrame::decode(&[1, 1, 0]).is_err());
    }
}
