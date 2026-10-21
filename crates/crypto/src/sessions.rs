//! Olm sessions with one remote device.

use core::fmt;

use ow_identity::{DeviceKey, PublicKey};
use vodozemac::olm::{OlmMessage, Session, SessionPickle};

use crate::account::{curve, session_config};
use crate::{CryptoAccount, CryptoError, Envelope, EnvelopeKind, PrekeyBundle};

/// Maximum number of sessions kept per peer device (older ones are dropped).
const MAX_SESSIONS: usize = 4;

/// All Olm sessions with one remote device, most recently used first.
///
/// Several sessions can exist when both sides start one at the same time; incoming messages are
/// tried against each, and the one that works becomes the sending session.
pub struct PeerSessions {
    peer: PublicKey,
    sessions: Vec<Session>,
}

impl PeerSessions {
    /// No session yet with `peer` (device key).
    #[must_use]
    pub const fn new(peer: PublicKey) -> Self {
        Self {
            peer,
            sessions: Vec::new(),
        }
    }

    /// Remote device key.
    #[must_use]
    pub const fn peer(&self) -> &PublicKey {
        &self.peer
    }

    /// `true` if at least one session exists.
    #[must_use]
    pub fn is_established(&self) -> bool {
        !self.sessions.is_empty()
    }

    /// Starts a new outbound session from the peer's (already verified) bundle.
    ///
    /// # Errors
    ///
    /// [`CryptoError::WrongDevice`] if the bundle is from another device, or
    /// [`CryptoError::Session`].
    pub fn start(
        &mut self,
        account: &CryptoAccount,
        peer_bundle: &PrekeyBundle,
    ) -> Result<(), CryptoError> {
        if peer_bundle.device() != &self.peer {
            return Err(CryptoError::WrongDevice);
        }
        let session = account
            .inner
            .create_outbound_session(
                session_config(),
                curve(peer_bundle.identity_key()),
                curve(peer_bundle.prekey()),
            )
            .map_err(|e| CryptoError::Session(e.to_string()))?;
        self.push_front(session);
        Ok(())
    }

    /// Encrypts `plaintext` for the peer.
    ///
    /// While the peer has not answered yet, Olm produces pre-key messages; they embed our signed
    /// bundle (hence the `account` and `device` arguments).
    ///
    /// # Errors
    ///
    /// [`CryptoError::Session`] if no session exists, or [`CryptoError::Encryption`].
    pub fn encrypt(
        &mut self,
        account: &mut CryptoAccount,
        device: &DeviceKey,
        plaintext: &[u8],
        now: u64,
    ) -> Result<Envelope, CryptoError> {
        let session = self
            .sessions
            .first_mut()
            .ok_or_else(|| CryptoError::Session("no session with this device".into()))?;
        let message = session
            .encrypt(plaintext)
            .map_err(|e| CryptoError::Encryption(e.to_string()))?;
        let (kind_code, ciphertext) = message.to_parts();
        let (kind, sender_bundle) = if kind_code == 0 {
            (
                EnvelopeKind::PreKey,
                Some(account.fallback_bundle(device, now)),
            )
        } else {
            (EnvelopeKind::Normal, None)
        };
        Ok(Envelope {
            kind,
            sender: device.public_key(),
            sender_bundle,
            ciphertext,
        })
    }

    /// Decrypts an envelope received from the peer.
    ///
    /// `transport_sender` is the device key authenticated by the transport (libp2p peer id): it
    /// must match both the envelope and this peer.
    ///
    /// # Errors
    ///
    /// [`CryptoError::WrongDevice`], [`CryptoError::InvalidSignature`] (identity key not bound to
    /// the device) or [`CryptoError::Decryption`] (unknown session, replay, tampering).
    pub fn decrypt(
        &mut self,
        account: &mut CryptoAccount,
        envelope: &Envelope,
        transport_sender: &PublicKey,
    ) -> Result<Vec<u8>, CryptoError> {
        if envelope.sender() != transport_sender || transport_sender != &self.peer {
            return Err(CryptoError::WrongDevice);
        }
        let message = OlmMessage::from_parts(envelope.kind as usize, &envelope.ciphertext)
            .map_err(|_| CryptoError::Malformed)?;

        // Existing sessions first.
        for i in 0..self.sessions.len() {
            let matches = match &message {
                OlmMessage::PreKey(m) => m.session_id() == self.sessions[i].session_id(),
                OlmMessage::Normal(_) => true,
            };
            if matches {
                if let Ok(plaintext) = self.sessions[i].decrypt(&message) {
                    let s = self.sessions.remove(i);
                    self.sessions.insert(0, s);
                    return Ok(plaintext);
                }
            }
        }

        // New inbound session (pre-key messages only).
        let OlmMessage::PreKey(pre_key) = &message else {
            return Err(CryptoError::Decryption("no matching session".into()));
        };
        let bundle = envelope
            .sender_bundle
            .as_ref()
            .ok_or(CryptoError::Malformed)?;
        // The Olm identity key used in the handshake must be the one the device signed.
        if pre_key.identity_key().to_bytes() != *bundle.identity_key() {
            return Err(CryptoError::InvalidSignature);
        }
        let result = account
            .inner
            .create_inbound_session(session_config(), curve(bundle.identity_key()), pre_key)
            .map_err(|e| CryptoError::Decryption(e.to_string()))?;
        self.push_front(result.session);
        Ok(result.plaintext)
    }

    fn push_front(&mut self, s: Session) {
        self.sessions.insert(0, s);
        self.sessions.truncate(MAX_SESSIONS);
    }

    /// Serializes all sessions, encrypted with `pickle_key` (one line per session).
    #[must_use]
    pub fn pickle(&self, pickle_key: &[u8; 32]) -> String {
        self.sessions
            .iter()
            .map(|s| s.pickle().encrypt(pickle_key))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Restores sessions with `peer`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Pickle`] if the data or key is wrong.
    pub fn from_pickle(
        peer: PublicKey,
        pickled: &str,
        pickle_key: &[u8; 32],
    ) -> Result<Self, CryptoError> {
        let sessions = pickled
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                SessionPickle::from_encrypted(l, pickle_key)
                    .map(Session::from_pickle)
                    .map_err(|e| CryptoError::Pickle(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { peer, sessions })
    }
}

impl fmt::Debug for PeerSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerSessions")
            .field("peer", &self.peer)
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}
