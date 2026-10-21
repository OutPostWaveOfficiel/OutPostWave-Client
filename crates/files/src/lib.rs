//! P2P file transfer: encrypted BLAKE3 chunks, resume, multi-source swarm.
//!
//! A file is cut in chunks of [`CHUNK_SIZE`] bytes. Each chunk is encrypted with a random
//! per-file key ([`FileKey`], XChaCha20-Poly1305, nonce = chunk index) and the **ciphertext** is
//! hashed with BLAKE3. The hashes form a [`Manifest`] signed by the sender's device; the file id
//! is the BLAKE3 hash of the manifest. The file key and id travel inside the end-to-end encrypted
//! offer message, so:
//!
//! - any peer holding chunks can serve them (multi-source, like BitTorrent) without being able to
//!   read them unless it received the key;
//! - every chunk is verified against the manifest: a source cannot inject forged data;
//! - a transfer resumes exactly where it stopped ([`Bitmap`] of received chunks).
//!
//! The network is abstracted by [`Transport`]; [`download`] drives a swarm download and
//! [`respond`] answers requests from a [`Shared`] file. See `spec/v0/files.md` in
//! OutpostWave-Protocol.

mod bitmap;
mod crypto;
mod download;
mod manifest;
mod part;
mod proto;
mod scheduler;
mod serve;

pub use bitmap::Bitmap;
pub use crypto::FileKey;
pub use download::{Download, Outcome, Progress, Transport, download, fetch_manifest};
pub use manifest::{FileId, Manifest, ManifestError, prepare};
pub use part::{PartFile, safe_file_name, unique_path};
pub use proto::{Request, Response, Status};
pub use serve::{Shared, respond};

/// Chunk size: 1 MiB.
pub const CHUNK_SIZE: u32 = 1024 * 1024;
/// Hashes per manifest page (1 MiB of hashes).
pub const HASHES_PER_PAGE: usize = 32 * 1024;
/// Maximum size of a response on the file protocol (one chunk plus headers).
pub const MAX_RESPONSE: usize = CHUNK_SIZE as usize + 64 * 1024;
