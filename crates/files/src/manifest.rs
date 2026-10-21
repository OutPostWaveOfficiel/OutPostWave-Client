//! Signed list of chunk hashes.
//!
//! ```text
//! "OWFM" | version u8 = 1 | size u64 BE | chunk_size u32 BE | count u32 BE
//! | BLAKE3(ciphertext chunk) 32 × count | signer device key 32 | Ed25519 signature 64
//! ```
//!
//! The signature covers every byte before it. The file id is BLAKE3 of the whole manifest.

use std::io::Read;
use std::path::Path;

use ow_identity::{DeviceKey, PublicKey};

use crate::{CHUNK_SIZE, FileKey};

const MAGIC: &[u8; 4] = b"OWFM";
const VERSION: u8 = 1;
const HEADER: usize = 4 + 1 + 8 + 4 + 4;
const TRAILER: usize = 32 + 64;

/// File id: BLAKE3 hash of the manifest.
pub type FileId = [u8; 32];

/// Manifest errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    /// Malformed bytes.
    Malformed,
    /// The bytes do not hash to the expected file id.
    WrongId,
    /// Bad signature.
    BadSignature,
}

impl core::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed manifest",
            Self::WrongId => "manifest does not match the file id",
            Self::BadSignature => "invalid manifest signature",
        })
    }
}

impl std::error::Error for ManifestError {}

/// Parsed and verified manifest.
#[derive(Clone, PartialEq, Eq)]
pub struct Manifest {
    size: u64,
    chunk_size: u32,
    hashes: Vec<[u8; 32]>,
    signer: [u8; 32],
    bytes: Vec<u8>,
    id: FileId,
}

impl core::fmt::Debug for Manifest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Manifest({} bytes, {} chunks)",
            self.size,
            self.hashes.len()
        )
    }
}

fn chunk_count(size: u64, chunk_size: u32) -> Option<u32> {
    u32::try_from(size.div_ceil(u64::from(chunk_size))).ok()
}

impl Manifest {
    /// Builds and signs a manifest.
    ///
    /// # Panics
    ///
    /// Panics if `hashes` does not match `size` and `chunk_size` (programming error).
    #[must_use]
    pub fn sign(size: u64, chunk_size: u32, hashes: Vec<[u8; 32]>, device: &DeviceKey) -> Self {
        let count = chunk_count(size, chunk_size).expect("too many chunks");
        assert_eq!(hashes.len(), count as usize, "hash count");
        let mut bytes = Vec::with_capacity(HEADER + hashes.len() * 32 + TRAILER);
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(&chunk_size.to_be_bytes());
        bytes.extend_from_slice(&count.to_be_bytes());
        for h in &hashes {
            bytes.extend_from_slice(h);
        }
        let signer = device.public_key().to_bytes();
        bytes.extend_from_slice(&signer);
        let sig = device.sign(&bytes);
        bytes.extend_from_slice(&sig);
        let id = *blake3::hash(&bytes).as_bytes();
        Self {
            size,
            chunk_size,
            hashes,
            signer,
            bytes,
            id,
        }
    }

    /// Parses and verifies a manifest received for `expected`.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] if the bytes are malformed, do not hash to `expected` or the signature
    /// is invalid.
    pub fn parse(bytes: &[u8], expected: &FileId) -> Result<Self, ManifestError> {
        if blake3::hash(bytes).as_bytes() != expected {
            return Err(ManifestError::WrongId);
        }
        if bytes.len() < HEADER + TRAILER || &bytes[..4] != MAGIC || bytes[4] != VERSION {
            return Err(ManifestError::Malformed);
        }
        let size = u64::from_be_bytes(
            bytes[5..13]
                .try_into()
                .map_err(|_| ManifestError::Malformed)?,
        );
        let chunk_size = u32::from_be_bytes(
            bytes[13..17]
                .try_into()
                .map_err(|_| ManifestError::Malformed)?,
        );
        let count = u32::from_be_bytes(
            bytes[17..21]
                .try_into()
                .map_err(|_| ManifestError::Malformed)?,
        );
        if chunk_size == 0
            || chunk_size > CHUNK_SIZE
            || chunk_count(size, chunk_size) != Some(count)
            || bytes.len() != HEADER + count as usize * 32 + TRAILER
        {
            return Err(ManifestError::Malformed);
        }
        let hashes_end = HEADER + count as usize * 32;
        let hashes = bytes[HEADER..hashes_end]
            .chunks_exact(32)
            .map(|c| c.try_into().unwrap_or([0; 32]))
            .collect();
        let mut signer = [0u8; 32];
        signer.copy_from_slice(&bytes[hashes_end..hashes_end + 32]);
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&bytes[hashes_end + 32..]);
        PublicKey::from_bytes(&signer)
            .and_then(|k| k.verify(&bytes[..hashes_end + 32], &sig))
            .map_err(|_| ManifestError::BadSignature)?;
        Ok(Self {
            size,
            chunk_size,
            hashes,
            signer,
            bytes: bytes.to_vec(),
            id: *expected,
        })
    }

    /// File id.
    #[must_use]
    pub const fn id(&self) -> &FileId {
        &self.id
    }

    /// Encoded manifest.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// File size in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Chunk size.
    #[must_use]
    pub const fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    /// Number of chunks.
    #[must_use]
    pub fn count(&self) -> u32 {
        u32::try_from(self.hashes.len()).unwrap_or(u32::MAX)
    }

    /// Device that signed the manifest.
    #[must_use]
    pub const fn signer(&self) -> &[u8; 32] {
        &self.signer
    }

    /// Expected hash of ciphertext chunk `i`.
    #[must_use]
    pub fn hash(&self, i: u32) -> Option<&[u8; 32]> {
        self.hashes.get(i as usize)
    }

    /// Plaintext length of chunk `i`.
    #[must_use]
    pub fn chunk_len(&self, i: u32) -> usize {
        let start = u64::from(i) * u64::from(self.chunk_size);
        usize::try_from(
            self.size
                .saturating_sub(start)
                .min(u64::from(self.chunk_size)),
        )
        .unwrap_or(0)
    }

    /// Byte offset of chunk `i` in the file.
    #[must_use]
    pub fn offset(&self, i: u32) -> u64 {
        u64::from(i) * u64::from(self.chunk_size)
    }
}

/// Reads `path`, encrypts every chunk with `key` and builds the signed manifest. Blocking: run it
/// off the async runtime. `progress` receives the number of bytes processed so far.
///
/// # Errors
///
/// I/O errors (or a file larger than 4 Gi chunks).
pub fn prepare(
    path: &Path,
    key: &FileKey,
    device: &DeviceKey,
    mut progress: impl FnMut(u64),
) -> std::io::Result<Manifest> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let count =
        chunk_count(size, CHUNK_SIZE).ok_or_else(|| std::io::Error::other("file too large"))?;
    let mut hashes = Vec::with_capacity(count as usize);
    let mut buf = vec![0u8; CHUNK_SIZE as usize];
    let mut done = 0u64;
    for i in 0..count {
        let len = usize::try_from((size - done).min(u64::from(CHUNK_SIZE))).unwrap_or(0);
        file.read_exact(&mut buf[..len])?;
        hashes.push(*blake3::hash(&key.seal(i, &buf[..len])).as_bytes());
        done += len as u64;
        progress(done);
    }
    // The file must not have grown meanwhile.
    if file.read(&mut [0u8; 1])? != 0 {
        return Err(std::io::Error::other("file changed while reading"));
    }
    Ok(Manifest::sign(size, CHUNK_SIZE, hashes, device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_parse() {
        let dev = DeviceKey::generate().unwrap();
        let m = Manifest::sign(2_500_000, CHUNK_SIZE, vec![[1; 32], [2; 32], [3; 32]], &dev);
        let p = Manifest::parse(m.as_bytes(), m.id()).unwrap();
        assert_eq!(p, m);
        assert_eq!(p.count(), 3);
        assert_eq!(p.chunk_len(2), 2_500_000 - 2 * CHUNK_SIZE as usize);
        assert_eq!(p.signer(), &dev.public_key().to_bytes());
        let mut t = m.as_bytes().to_vec();
        t[30] ^= 1;
        assert_eq!(Manifest::parse(&t, m.id()), Err(ManifestError::WrongId));
        let id = *blake3::hash(&t).as_bytes();
        assert_eq!(Manifest::parse(&t, &id), Err(ManifestError::BadSignature));
        // Empty file.
        let e = Manifest::sign(0, CHUNK_SIZE, vec![], &dev);
        assert_eq!(Manifest::parse(e.as_bytes(), e.id()).unwrap().count(), 0);
    }

    #[test]
    fn prepare_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let key = FileKey::generate().unwrap();
        let dev = DeviceKey::generate().unwrap();
        let mut last = 0;
        let m = prepare(&path, &key, &dev, |d| last = d).unwrap();
        assert_eq!(last, 3_000_000);
        assert_eq!(m.count(), 3);
        let c1 = key.seal(1, &data[CHUNK_SIZE as usize..2 * CHUNK_SIZE as usize]);
        assert_eq!(m.hash(1).unwrap(), blake3::hash(&c1).as_bytes());
    }
}
