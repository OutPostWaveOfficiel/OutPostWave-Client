//! OutpostWave identity.
//!
//! - A **master key** (Ed25519) is derived from a 24-word **recovery phrase** (BIP-39).
//!   Its private part never leaves the user's devices.
//! - Each **device** has its own random Ed25519 key, certified by the master key
//!   ([`DeviceCertificate`]). Device keys are what peers see on the network.
//! - A **handle** `name#tag` pairs a unique name (enforced by the registry) with a 4-character
//!   tag derived from the master public key, for quick visual checks.
//! - **Safety numbers** let two contacts verify each other's keys out of band.

mod base32;
mod device;
mod error;
mod handle;
mod keys;
mod master;
mod phrase;
mod safety;

pub use device::{DeviceCertificate, DeviceKey, MAX_DEVICE_NAME_LEN};
pub use error::IdentityError;
pub use handle::{Handle, MAX_NAME_LEN, MIN_NAME_LEN, TAG_LEN};
pub use keys::{Fingerprint, PublicKey};
pub use master::MasterKey;
pub use phrase::{RecoveryPhrase, WORD_COUNT};
pub use safety::safety_number;
