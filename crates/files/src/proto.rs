//! Requests and responses of the file protocol (`/outpostwave/file/1`).
//!
//! ```text
//! request:  version u8 = 1 | kind u8 | file_id 32 | arg u32 BE
//!           kind 1 = manifest page `arg`, 2 = have (arg 0), 3 = chunk `arg`
//! response: status u8 | body
//!           manifest: total_len u32 BE | page bytes ; have: bitmap ; chunk: ciphertext
//! ```

use crate::FileId;

const VERSION: u8 = 1;

/// A request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Page `n` of the manifest (pages of 1 MiB).
    Manifest(FileId, u32),
    /// Bitmap of the chunks the source holds.
    Have(FileId),
    /// Encrypted chunk `n`.
    Chunk(FileId, u32),
}

/// Response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Success.
    Ok,
    /// The source does not know this file (or the requester may not have it).
    Unknown,
    /// The source does not have this chunk (yet).
    Missing,
    /// Too many requests: retry later.
    Busy,
    /// The source file changed since it was offered: the transfer cannot complete.
    Changed,
}

impl Status {
    const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Unknown => 1,
            Self::Missing => 2,
            Self::Busy => 3,
            Self::Changed => 4,
        }
    }

    const fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            0 => Self::Ok,
            1 => Self::Unknown,
            2 => Self::Missing,
            3 => Self::Busy,
            4 => Self::Changed,
            _ => return None,
        })
    }
}

/// A response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Status.
    pub status: Status,
    /// Body (empty unless `status` is [`Status::Ok`]).
    pub body: Vec<u8>,
}

impl Response {
    /// Error response.
    #[must_use]
    pub const fn error(status: Status) -> Self {
        Self {
            status,
            body: Vec::new(),
        }
    }

    /// Encodes the response.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.body.len());
        out.push(self.status.code());
        out.extend_from_slice(&self.body);
        out
    }

    /// Decodes a response.
    #[must_use]
    pub fn decode(mut b: Vec<u8>) -> Option<Self> {
        let status = Status::from_code(*b.first()?)?;
        b.remove(0);
        Some(Self { status, body: b })
    }
}

impl Request {
    /// File concerned.
    #[must_use]
    pub const fn file_id(&self) -> &FileId {
        match self {
            Self::Manifest(id, _) | Self::Have(id) | Self::Chunk(id, _) => id,
        }
    }

    /// Encodes the request.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let (kind, id, arg) = match *self {
            Self::Manifest(id, n) => (1, id, n),
            Self::Have(id) => (2, id, 0),
            Self::Chunk(id, n) => (3, id, n),
        };
        let mut out = Vec::with_capacity(38);
        out.extend_from_slice(&[VERSION, kind]);
        out.extend_from_slice(&id);
        out.extend_from_slice(&arg.to_be_bytes());
        out
    }

    /// Decodes a request.
    #[must_use]
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != 38 || b[0] != VERSION {
            return None;
        }
        let id: FileId = b[2..34].try_into().ok()?;
        let arg = u32::from_be_bytes(b[34..38].try_into().ok()?);
        Some(match b[1] {
            1 => Self::Manifest(id, arg),
            2 => Self::Have(id),
            3 => Self::Chunk(id, arg),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for r in [
            Request::Manifest([1; 32], 2),
            Request::Have([3; 32]),
            Request::Chunk([4; 32], 9),
        ] {
            assert_eq!(Request::decode(&r.encode()), Some(r));
        }
        assert_eq!(Request::decode(&[1, 9, 0]), None);
        let resp = Response {
            status: Status::Ok,
            body: vec![1, 2],
        };
        assert_eq!(Response::decode(resp.encode()), Some(resp));
        assert_eq!(Response::decode(vec![77]), None);
    }
}
