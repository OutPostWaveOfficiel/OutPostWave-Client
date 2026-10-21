//! End-to-end encryption for OutpostWave direct messages.
//!
//! Built on **vodozemac** (Olm = Signal's Double Ratchet + triple Diffie-Hellman), with
//! OutpostWave's trust chain on top:
//!
//! ```text
//! master key ──signs──▶ device certificate ──▶ device key ──signs──▶ prekey bundle (Curve25519)
//! ```
//!
//! - [`CryptoAccount`]: per-device Olm account. Publishes a signed [`PrekeyBundle`].
//! - [`PeerSessions`]: Olm sessions with one remote device (handles simultaneous session setup).
//! - [`Envelope`]: wire format of an encrypted message, carried by `/outpostwave/dm/1`.
//!
//! Group encryption (MLS) comes in roadmap step 4.

mod account;
mod bundle;
mod envelope;
mod error;
mod sessions;

pub use account::CryptoAccount;
pub use bundle::{BUNDLE_LEN, KeyKind, PrekeyBundle};
pub use envelope::{Envelope, EnvelopeKind};
pub use error::CryptoError;
pub use sessions::PeerSessions;

/// Derives the key that encrypts pickled Olm state at rest (defence in depth on top of the
/// encrypted database), from the device secret.
#[must_use]
pub fn pickle_key(device: &ow_identity::DeviceKey) -> zeroize::Zeroizing<[u8; 32]> {
    let mut secret = device.to_secret_bytes();
    let key = blake3::derive_key("OutpostWave 2026-10 olm pickle key v1", &secret);
    zeroize::Zeroize::zeroize(&mut secret);
    zeroize::Zeroizing::new(key)
}
