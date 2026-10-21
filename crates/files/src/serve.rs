//! Source side: answering requests from a file we hold (original or partial download).

use std::path::PathBuf;

use crate::part::read_range;
use crate::{Bitmap, FileKey, HASHES_PER_PAGE, Manifest, Request, Response, Status};

/// A file this node can serve.
#[derive(Debug, Clone)]
pub struct Shared {
    /// Verified manifest.
    pub manifest: Manifest,
    /// File key (chunks are sealed on the fly; the plaintext stays on disk).
    pub key: FileKey,
    /// Plaintext file: the original (sender) or the `.owpart`/final file (receiver).
    pub path: PathBuf,
    /// Chunks available in `path` (`None`: all of them).
    pub have: Option<Bitmap>,
}

/// Answers `request` for `shared`. Blocking (reads the disk): run it off the async runtime.
/// The caller checks beforehand that the requester may access the file.
#[must_use]
pub fn respond(shared: &Shared, request: &Request) -> Response {
    let m = &shared.manifest;
    if request.file_id() != m.id() {
        return Response::error(Status::Unknown);
    }
    match *request {
        Request::Manifest(_, page) => {
            let bytes = m.as_bytes();
            let page_len = HASHES_PER_PAGE * 32;
            let start = page as usize * page_len;
            if start >= bytes.len() {
                return Response::error(Status::Missing);
            }
            let end = (start + page_len).min(bytes.len());
            let mut body = Vec::with_capacity(4 + end - start);
            body.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
            body.extend_from_slice(&bytes[start..end]);
            Response {
                status: Status::Ok,
                body,
            }
        }
        Request::Have(_) => Response {
            status: Status::Ok,
            body: shared
                .have
                .clone()
                .unwrap_or_else(|| Bitmap::full(m.count()))
                .as_bytes()
                .to_vec(),
        },
        Request::Chunk(_, i) => {
            let Some(expected) = m.hash(i) else {
                return Response::error(Status::Missing);
            };
            if shared.have.as_ref().is_some_and(|h| !h.get(i)) {
                return Response::error(Status::Missing);
            }
            let Ok(plain) = read_range(&shared.path, m.offset(i), m.chunk_len(i)) else {
                // Original moved or deleted: nobody can finish from us.
                return Response::error(if shared.have.is_none() {
                    Status::Changed
                } else {
                    Status::Missing
                });
            };
            let sealed = shared.key.seal(i, &plain);
            if blake3::hash(&sealed).as_bytes() != expected {
                return Response::error(Status::Changed);
            }
            Response {
                status: Status::Ok,
                body: sealed,
            }
        }
    }
}
