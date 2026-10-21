//! Invite codes: `OW1:` + Crockford base32 of the binary invite.
//!
//! ```text
//! version u8 = 1 | name_len u8 | name | master_pub 32 | cert_len u16 BE | cert | bundle 174
//! | addr_count u8 | (addr_len u8 | multiaddr bytes)*
//! ```
//!
//! Everything that matters is signed (certificate by the master key, bundle by the device key);
//! addresses are only connection hints.

use ow_crypto::{BUNDLE_LEN, PrekeyBundle};
use ow_identity::{DeviceCertificate, Handle, PublicKey};
use ow_p2p::Multiaddr;

use crate::{MessagingError, base32};

const PREFIX: &str = "OW1:";
const MAX_ADDRS: usize = 8;

/// A verified invite.
#[derive(Debug, Clone)]
pub struct Invite {
    /// Handle of the inviting user.
    pub handle: Handle,
    /// Master public key.
    pub master: PublicKey,
    /// Device certificate (verified).
    pub cert: DeviceCertificate,
    /// Prekey bundle of that device (verified).
    pub bundle: PrekeyBundle,
    /// Network address hints.
    pub addrs: Vec<Multiaddr>,
}

impl Invite {
    /// Encodes the invite as an `OW1:` code.
    #[must_use]
    pub fn to_code(&self) -> String {
        let mut b = vec![1u8];
        let name = self.handle.name().as_bytes();
        b.push(u8::try_from(name.len()).unwrap_or(0));
        b.extend_from_slice(name);
        b.extend_from_slice(&self.master.to_bytes());
        let cert = self.cert.to_bytes();
        b.extend_from_slice(&u16::try_from(cert.len()).unwrap_or(0).to_be_bytes());
        b.extend_from_slice(&cert);
        b.extend_from_slice(&self.bundle.to_bytes());
        let addrs: Vec<Vec<u8>> = self
            .addrs
            .iter()
            .map(Multiaddr::to_vec)
            .filter(|a| a.len() <= usize::from(u8::MAX))
            .take(MAX_ADDRS)
            .collect();
        b.push(u8::try_from(addrs.len()).unwrap_or(0));
        for a in addrs {
            b.push(u8::try_from(a.len()).unwrap_or(0));
            b.extend_from_slice(&a);
        }
        format!("{PREFIX}{}", base32::encode(&b))
    }

    /// Parses and verifies an invite code.
    ///
    /// # Errors
    ///
    /// [`MessagingError::InvalidInvite`] if the code is malformed or any signature or binding
    /// fails.
    pub fn parse(code: &str) -> Result<Self, MessagingError> {
        let bad = || MessagingError::InvalidInvite;
        let code = code.trim();
        let body = code
            .get(..PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(PREFIX))
            .and_then(|_| code.get(PREFIX.len()..))
            .ok_or_else(bad)?;
        let raw = base32::decode(body).ok_or_else(bad)?;
        let mut r = raw.as_slice();
        let mut take = |n: usize| -> Result<&[u8], MessagingError> {
            if r.len() < n {
                return Err(bad());
            }
            let (h, t) = r.split_at(n);
            r = t;
            Ok(h)
        };
        if take(1)? != [1] {
            return Err(bad());
        }
        let name_len = usize::from(take(1)?[0]);
        let name = core::str::from_utf8(take(name_len)?)
            .map_err(|_| bad())?
            .to_owned();
        let mut mk = [0u8; 32];
        mk.copy_from_slice(take(32)?);
        let master = PublicKey::from_bytes(&mk).map_err(|_| bad())?;
        let cl = take(2)?;
        let cert_len = usize::from(u16::from_be_bytes([cl[0], cl[1]]));
        let cert = DeviceCertificate::from_bytes(take(cert_len)?).map_err(|_| bad())?;
        let bundle =
            PrekeyBundle::from_bytes(take(BUNDLE_LEN)?, cert.device()).map_err(|_| bad())?;
        let count = usize::from(take(1)?[0]);
        let mut addrs = Vec::new();
        for _ in 0..count.min(MAX_ADDRS) {
            let len = usize::from(take(1)?[0]);
            if let Ok(a) = Multiaddr::try_from(take(len)?.to_vec()) {
                addrs.push(a);
            }
        }
        if cert.master() != &master {
            return Err(bad());
        }
        let handle = Handle::new(&name).map_err(|_| bad())?;
        Ok(Self {
            handle,
            master,
            cert,
            bundle,
            addrs,
        })
    }
}
