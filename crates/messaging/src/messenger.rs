//! The messenger service.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use ow_crypto::{CryptoAccount, Envelope, PeerSessions, PrekeyBundle, pickle_key};
use ow_identity::{
    DeviceCertificate, DeviceKey, Handle, MasterKey, PublicKey, RecoveryPhrase, safety_number,
};
use ow_p2p::{
    Multiaddr, P2pConfig, P2pEvent, P2pHandle, PeerId, circuit_addr, device_key_of, peer_id_of,
};
use ow_registry::{AccountInfo, RegistryClient};
use ow_storage::{
    ContactId, LocalIdentity, MessageStatus, NewContact, NewMessage, Storage, StoredMessage,
};
use tokio::sync::{broadcast, mpsc};
use zeroize::Zeroizing;

use crate::backup::{BackupContact, BackupData, BackupDevice, BackupKey, BackupMessage};
use crate::wire::{Frame, Intro, MAX_RECEIPT_IDS, MAX_TEXT_LEN, TextFrame};
use crate::{Invite, MessagingError, base32};

type Result<T> = core::result::Result<T, MessagingError>;

const ACK_OK: &[u8] = b"ok";
const ACK_ERR: &[u8] = b"err";
const SECRET_ACCOUNT: &str = "olm:account";
const SETTING_READ_RECEIPTS: &str = "setting:read_receipts";
const SECRET_BACKUP_KEY: &str = "backup:key";
/// Interval between automatic backups (when something changed).
const BACKUP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// Backup size budget (the registry accepts up to 16 MiB).
const BACKUP_BUDGET: usize = 12 * 1024 * 1024;

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64<const N: usize>(s: &str) -> Option<[u8; N]> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()?
        .try_into()
        .ok()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn key_name(prefix: &str, device: &[u8; 32]) -> String {
    format!("{prefix}:{}", base32::encode(device))
}

fn encode_addrs(addrs: &[Multiaddr]) -> Vec<u8> {
    let mut out = Vec::new();
    for a in addrs.iter().take(16) {
        let b = a.to_vec();
        if let Ok(len) = u8::try_from(b.len()) {
            out.push(len);
            out.extend_from_slice(&b);
        }
    }
    out
}

fn decode_addrs(mut b: &[u8]) -> Vec<Multiaddr> {
    let mut out = Vec::new();
    while let Some((&len, rest)) = b.split_first() {
        let len = usize::from(len);
        if rest.len() < len {
            break;
        }
        if let Ok(a) = Multiaddr::try_from(rest[..len].to_vec()) {
            out.push(a);
        }
        b = &rest[len..];
    }
    out
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock leaves consistent data (every write is a single statement).
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Identity creation on a fresh device.
#[derive(Debug, Clone, Copy)]
pub struct ProfileSetup;

/// An identity prepared in memory, not yet saved. Register it on the registry (to claim the
/// username) before calling [`NewProfile::commit`].
pub struct NewProfile {
    phrase: Option<RecoveryPhrase>,
    handle: Handle,
    device: DeviceKey,
    cert: DeviceCertificate,
    account: CryptoAccount,
}

impl core::fmt::Debug for NewProfile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "NewProfile({})", self.handle)
    }
}

impl ProfileSetup {
    /// Prepares a brand-new identity (random recovery phrase).
    ///
    /// # Errors
    ///
    /// Invalid name or device name, RNG failure.
    pub fn prepare_new(name: &str, device_name: &str) -> Result<NewProfile> {
        let phrase = RecoveryPhrase::generate()?;
        let mut p = Self::prepare(&phrase, name, device_name)?;
        p.phrase = Some(phrase);
        Ok(p)
    }

    /// Prepares this device for an existing identity, from its recovery phrase.
    ///
    /// # Errors
    ///
    /// Invalid phrase, name or device name.
    pub fn prepare_restore(phrase: &str, name: &str, device_name: &str) -> Result<NewProfile> {
        Self::prepare(&RecoveryPhrase::parse(phrase)?, name, device_name)
    }

    fn prepare(phrase: &RecoveryPhrase, name: &str, device_name: &str) -> Result<NewProfile> {
        let master = MasterKey::from_phrase(phrase);
        let handle = Handle::new(name)?;
        let device = DeviceKey::generate()?;
        let cert = DeviceCertificate::issue(&master, &device.public_key(), device_name, now_s())?;
        Ok(NewProfile {
            phrase: None,
            handle,
            device,
            cert,
            account: CryptoAccount::new(),
        })
    }

    /// Creates and saves a new identity without registry (tests, offline mode). Returns the
    /// recovery phrase, to show **once** to the user.
    ///
    /// # Errors
    ///
    /// [`MessagingError::IdentityExists`], invalid name or device name, storage errors.
    pub fn create(storage: &Storage, name: &str, device_name: &str) -> Result<RecoveryPhrase> {
        Self::prepare_new(name, device_name)?
            .commit(storage)?
            .ok_or(MessagingError::NoIdentity)
    }

    /// Restores an identity from its recovery phrase without registry.
    ///
    /// # Errors
    ///
    /// [`MessagingError::IdentityExists`], invalid phrase, name or device name, storage errors.
    pub fn restore(storage: &Storage, phrase: &str, name: &str, device_name: &str) -> Result<()> {
        Self::prepare_restore(phrase, name, device_name)?
            .commit(storage)
            .map(|_| ())
    }
}

impl NewProfile {
    /// Username.
    #[must_use]
    pub const fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Device key.
    #[must_use]
    pub const fn device(&self) -> &DeviceKey {
        &self.device
    }

    /// Device certificate.
    #[must_use]
    pub const fn cert(&self) -> &DeviceCertificate {
        &self.cert
    }

    /// Current fallback prekey bundle (published on the registry).
    pub fn bundle(&mut self) -> PrekeyBundle {
        self.account.fallback_bundle(&self.device, now_s())
    }

    /// Saves the identity. Returns the recovery phrase for new identities (show it **once**).
    ///
    /// # Errors
    ///
    /// [`MessagingError::IdentityExists`] or storage errors.
    pub fn commit(self, storage: &Storage) -> Result<Option<RecoveryPhrase>> {
        if storage.load_local_identity()?.is_some() {
            return Err(MessagingError::IdentityExists);
        }
        storage.put_secret(
            SECRET_ACCOUNT,
            self.account.pickle(&pickle_key(&self.device)).as_bytes(),
            now_ms(),
        )?;
        // The master secret is not kept: adding a device requires the recovery phrase.
        let mut device_secret = self.device.to_secret_bytes();
        let id = LocalIdentity {
            handle_name: self.handle.name().to_owned(),
            device_secret,
            device_cert: self.cert.to_bytes(),
            master_secret: None,
            created_at: now_ms(),
        };
        zeroize::Zeroize::zeroize(&mut device_secret);
        storage.save_local_identity(&id)?;
        Ok(self.phrase)
    }
}

/// Contact as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    /// Row id.
    pub id: ContactId,
    /// Username.
    pub handle: String,
    /// Local nickname.
    pub display_name: Option<String>,
    /// Safety number verified by the user.
    pub verified: bool,
    /// At least one of the contact's devices is connected.
    pub online: bool,
    /// Safety number to compare out of band.
    pub safety_number: String,
    /// The contact's keys changed (account recovered or reinstalled): compare the safety number
    /// again.
    pub key_changed: bool,
    /// The username was announced by the contact but not confirmed by the account server.
    pub name_unconfirmed: bool,
    /// Unread incoming messages.
    pub unread: u32,
}

/// Events for the UI.
#[derive(Debug, Clone)]
pub enum MessengerEvent {
    /// A message was stored (incoming, or outgoing just created).
    Message {
        /// Contact of the conversation.
        contact: ContactId,
        /// The message.
        message: StoredMessage,
    },
    /// The status of an outgoing message changed.
    Status {
        /// Contact of the conversation.
        contact: ContactId,
        /// Message id.
        msg_id: [u8; 16],
        /// New status.
        status: MessageStatus,
    },
    /// The contact list changed (added, updated).
    ContactsChanged,
    /// Someone unknown wrote to us and was added to the contacts.
    ContactRequest {
        /// The new contact.
        contact: ContactId,
    },
    /// A contact's keys changed (verified with the account server).
    KeyChanged {
        /// Contact.
        contact: ContactId,
    },
    /// A contact came online or went offline.
    Presence {
        /// Contact.
        contact: ContactId,
        /// Online state.
        online: bool,
    },
}

struct Crypto {
    account: CryptoAccount,
    peers: HashMap<[u8; 32], PeerSessions>,
}

#[derive(Default)]
struct Net {
    addrs: HashMap<PeerId, Vec<Multiaddr>>,
    listening: Vec<Multiaddr>,
    online: HashSet<PeerId>,
    /// Contacts with a delivery in progress, and whether another pass was requested.
    delivering: HashMap<i64, bool>,
}

struct Inner {
    storage: Mutex<Storage>,
    handle: Handle,
    master: PublicKey,
    device: DeviceKey,
    cert: DeviceCertificate,
    pickle_key: Zeroizing<[u8; 32]>,
    crypto: Mutex<Crypto>,
    net: Mutex<Net>,
    p2p: P2pHandle,
    registry: Option<RegistryClient>,
    /// Something changed since the last backup.
    backup_dirty: std::sync::atomic::AtomicBool,
    /// Relays we hold a reservation on; contacts are tried through them too.
    relays: Vec<Multiaddr>,
    events: broadcast::Sender<MessengerEvent>,
}

/// Running messenger. Cheap to clone.
#[derive(Clone)]
pub struct Messenger {
    inner: Arc<Inner>,
}

impl core::fmt::Debug for Messenger {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Messenger({})", self.inner.handle)
    }
}

impl Messenger {
    /// Starts the messenger: loads the identity and crypto state, starts the P2P node.
    ///
    /// `configure` can adjust the network configuration (listen addresses, mDNS, relays).
    /// Must be called from within a Tokio runtime.
    ///
    /// # Errors
    ///
    /// [`MessagingError::NoIdentity`] if no identity was created, or storage/crypto/network
    /// errors.
    pub fn start(
        storage: Storage,
        registry: Option<RegistryClient>,
        configure: impl FnOnce(&mut P2pConfig),
    ) -> Result<(Self, broadcast::Receiver<MessengerEvent>)> {
        let id = storage
            .load_local_identity()?
            .ok_or(MessagingError::NoIdentity)?;
        let device = DeviceKey::from_secret_bytes(&id.device_secret);
        let cert = DeviceCertificate::from_bytes(&id.device_cert)?;
        if cert.device() != &device.public_key() {
            return Err(MessagingError::Identity(
                ow_identity::IdentityError::InvalidSignature,
            ));
        }
        let master = *cert.master();
        let handle = Handle::new(&id.handle_name)?;
        let pk = pickle_key(&device);
        let account = match storage.get_secret(SECRET_ACCOUNT)? {
            Some(p) => CryptoAccount::from_pickle(
                core::str::from_utf8(&p).map_err(|_| ow_crypto::CryptoError::Malformed)?,
                &pk,
            )?,
            None => CryptoAccount::new(),
        };

        let mut cfg = P2pConfig::new(id.device_secret);
        configure(&mut cfg);
        let relays = cfg.relays.clone();
        let (p2p, p2p_events) = ow_p2p::start(cfg)?;
        let (events, rx) = broadcast::channel(256);

        let inner = Arc::new(Inner {
            storage: Mutex::new(storage),
            handle,
            master,
            device,
            cert,
            pickle_key: pk,
            crypto: Mutex::new(Crypto {
                account,
                peers: HashMap::new(),
            }),
            net: Mutex::new(Net::default()),
            p2p,
            registry,
            backup_dirty: std::sync::atomic::AtomicBool::new(false),
            relays,
            events,
        });
        inner.save_account()?;
        tokio::spawn(Inner::event_loop(Arc::clone(&inner), p2p_events));
        tokio::spawn(Inner::reconnect_all(Arc::clone(&inner)));
        tokio::spawn(Inner::startup_checks(Arc::clone(&inner)));
        tokio::spawn(Inner::backup_loop(Arc::downgrade(&inner)));
        Ok((Self { inner }, rx))
    }

    /// Subscribes to events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<MessengerEvent> {
        self.inner.events.subscribe()
    }

    /// Our username.
    #[must_use]
    pub fn handle(&self) -> String {
        self.inner.handle.to_string()
    }

    /// Network peer id of this device.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.inner.p2p.peer_id()
    }

    /// Our invite code, with the current fallback prekey and network addresses.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn invite_code(&self) -> Result<String> {
        let inner = &self.inner;
        let bundle = lock(&inner.crypto)
            .account
            .fallback_bundle(&inner.device, now_s());
        inner.save_account()?;
        let addrs = lock(&inner.net).listening.clone();
        Ok(Invite {
            handle: inner.handle.clone(),
            master: inner.master,
            cert: inner.cert.clone(),
            bundle,
            addrs,
        }
        .to_code())
    }

    /// Adds a contact from its invite code and tries to connect.
    ///
    /// # Errors
    ///
    /// [`MessagingError::InvalidInvite`], [`MessagingError::OwnInvite`], storage errors.
    pub fn add_contact(&self, code: &str) -> Result<ContactInfo> {
        let invite = Invite::parse(code)?;
        let id = self.add_verified(
            &invite.handle,
            &invite.master,
            &[(invite.cert, invite.bundle)],
            invite.addrs,
        )?;
        // The name in an invite is self-asserted: confirm it with the account server.
        lock(&self.inner.storage).put_secret(&format!("unconfirmed:{}", id.0), &[1], now_ms())?;
        Inner::spawn_name_check(&self.inner, id);
        self.contact(id)
    }

    /// Finds a user by exact username on the registry and adds them as a contact.
    ///
    /// # Errors
    ///
    /// [`MessagingError::Registry`] (not found, unreachable...), [`MessagingError::OwnInvite`],
    /// storage errors.
    pub async fn add_contact_by_name(&self, name: &str) -> Result<ContactInfo> {
        let handle = Handle::new(name)?;
        let found = self.registry()?.lookup(&handle).await?;
        let id = self.add_verified(&found.name, &found.master, &found.devices, Vec::new())?;
        lock(&self.inner.storage).delete_secret(&format!("unconfirmed:{}", id.0))?;
        self.contact(id)
    }

    fn add_verified(
        &self,
        handle: &Handle,
        master: &PublicKey,
        devices: &[(DeviceCertificate, PrekeyBundle)],
        addrs: Vec<Multiaddr>,
    ) -> Result<ContactId> {
        let inner = &self.inner;
        if master == &inner.master {
            return Err(MessagingError::OwnInvite);
        }
        let id = {
            let s = lock(&inner.storage);
            let id = s.upsert_contact(&NewContact {
                master_pub: master.to_bytes(),
                handle: handle.to_string(),
                display_name: None,
                added_at: now_ms(),
            })?;
            for (cert, bundle) in devices {
                let device = cert.device().to_bytes();
                s.add_contact_device(id, &device, &cert.to_bytes())?;
                s.put_secret(&key_name("bundle", &device), &bundle.to_bytes(), now_ms())?;
            }
            s.dm_conversation(id, now_ms())?;
            id
        };
        if let Some((cert, _)) = devices.first() {
            inner.remember_addrs(&cert.device().to_bytes(), addrs);
        }
        let _ = inner.events.send(MessengerEvent::ContactsChanged);
        inner
            .backup_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Inner::spawn_delivery(inner, id);
        Ok(id)
    }

    fn registry(&self) -> Result<&RegistryClient> {
        self.inner
            .registry
            .as_ref()
            .ok_or(MessagingError::NoRegistry)
    }

    /// Our account on the registry (email, search visibility).
    ///
    /// # Errors
    ///
    /// [`MessagingError::NoRegistry`] or registry errors.
    pub async fn account(&self) -> Result<AccountInfo> {
        Ok(self.registry()?.me(&self.inner.device).await?)
    }

    /// Shows or hides us in username search.
    ///
    /// # Errors
    ///
    /// [`MessagingError::NoRegistry`] or registry errors.
    pub async fn set_discoverable(&self, discoverable: bool) -> Result<()> {
        Ok(self
            .registry()?
            .set_discoverable(&self.inner.device, discoverable)
            .await?)
    }

    /// Sets (or removes) the recovery email; a verification code is emailed.
    ///
    /// # Errors
    ///
    /// [`MessagingError::NoRegistry`] or registry errors.
    pub async fn set_email(&self, email: Option<&str>) -> Result<()> {
        Ok(self
            .registry()?
            .set_email(&self.inner.device, email)
            .await?)
    }

    /// Confirms the recovery email with the received code.
    ///
    /// # Errors
    ///
    /// [`MessagingError::NoRegistry`] or registry errors.
    pub async fn verify_email(&self, code: &str) -> Result<()> {
        Ok(self
            .registry()?
            .verify_email(&self.inner.device, code)
            .await?)
    }

    /// Contact details.
    ///
    /// # Errors
    ///
    /// [`MessagingError::UnknownContact`] or storage errors.
    pub fn contact(&self, id: ContactId) -> Result<ContactInfo> {
        let c = lock(&self.inner.storage)
            .contact(id)?
            .ok_or(MessagingError::UnknownContact)?;
        self.inner.contact_info(c)
    }

    /// All contacts.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn contacts(&self) -> Result<Vec<ContactInfo>> {
        let list = lock(&self.inner.storage).contacts()?;
        list.into_iter()
            .map(|c| self.inner.contact_info(c))
            .collect()
    }

    /// Marks a contact's safety number as verified (or not).
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn set_verified(&self, id: ContactId, verified: bool) -> Result<()> {
        {
            let s = lock(&self.inner.storage);
            s.set_contact_verified(id, verified)?;
            if verified {
                s.delete_secret(&format!("keychanged:{}", id.0))?;
            }
        }
        let _ = self.inner.events.send(MessengerEvent::ContactsChanged);
        Ok(())
    }

    /// Messages with a contact, newest first, before row id `before`.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn messages(
        &self,
        contact: ContactId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<StoredMessage>> {
        let s = lock(&self.inner.storage);
        let conv = s.dm_conversation(contact, now_ms())?;
        Ok(s.messages(conv, before, limit)?)
    }

    /// Queues a text message and delivers it as soon as the contact is reachable.
    ///
    /// # Errors
    ///
    /// [`MessagingError::InvalidText`], [`MessagingError::UnknownContact`], storage errors.
    pub fn send_text(&self, contact: ContactId, text: &str) -> Result<StoredMessage> {
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_TEXT_LEN {
            return Err(MessagingError::InvalidText);
        }
        let inner = &self.inner;
        let mut msg_id = [0u8; 16];
        getrandom::fill(&mut msg_id).map_err(|_| MessagingError::InvalidText)?;
        let message = {
            let s = lock(&inner.storage);
            s.contact(contact)?.ok_or(MessagingError::UnknownContact)?;
            let conv = s.dm_conversation(contact, now_ms())?;
            let new = NewMessage {
                conversation: conv,
                msg_id,
                sender_device: inner.device.public_key().to_bytes(),
                outgoing: true,
                sent_at: now_ms(),
                received_at: None,
                status: MessageStatus::Pending,
                body: text.to_owned(),
            };
            let id = s.insert_message(&new)?.unwrap_or_default();
            StoredMessage {
                id,
                conversation: conv,
                msg_id,
                sender_device: new.sender_device,
                outgoing: true,
                sent_at: new.sent_at,
                received_at: None,
                status: MessageStatus::Pending,
                body: new.body,
            }
        };
        let _ = inner.events.send(MessengerEvent::Message {
            contact,
            message: message.clone(),
        });
        Inner::spawn_delivery(inner, contact);
        Ok(message)
    }

    /// Marks the conversation with `contact` as read. If read receipts are enabled, the contact
    /// is told (now, or when it comes back online). Returns the number of newly read messages.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn mark_read(&self, contact: ContactId) -> Result<usize> {
        let inner = &self.inner;
        let ids = {
            let s = lock(&inner.storage);
            let conv = s.dm_conversation(contact, now_ms())?;
            s.mark_incoming_read(conv)?
        };
        if !ids.is_empty() && self.read_receipts()? {
            let key = format!("receipts:{}", contact.0);
            {
                let s = lock(&inner.storage);
                let mut queued = s.get_secret(&key)?.map(|b| b.to_vec()).unwrap_or_default();
                for id in &ids {
                    queued.extend_from_slice(id);
                }
                s.put_secret(&key, &queued, now_ms())?;
            }
            Inner::spawn_delivery(inner, contact);
        }
        Ok(ids.len())
    }

    /// Whether read receipts are sent (default: yes).
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn read_receipts(&self) -> Result<bool> {
        Ok(lock(&self.inner.storage)
            .get_secret(SETTING_READ_RECEIPTS)?
            .is_none_or(|v| v.first() != Some(&0)))
    }

    /// Enables or disables sending read receipts.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn set_read_receipts(&self, enabled: bool) -> Result<()> {
        lock(&self.inner.storage).put_secret(
            SETTING_READ_RECEIPTS,
            &[u8::from(enabled)],
            now_ms(),
        )?;
        Ok(())
    }

    /// Claims our username on the registry for an identity that has no account yet (created
    /// offline or before the registry existed). If the identity already has an account (created
    /// from another device), this device is attached to it instead.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Taken`](ow_registry::RegistryError::Taken) if someone else owns the
    /// name, [`MessagingError::NoRegistry`] or registry errors.
    pub async fn register_account(&self, email: Option<&str>, discoverable: bool) -> Result<()> {
        let registry = self.registry()?;
        let inner = &self.inner;
        let bundle = lock(&inner.crypto)
            .account
            .fallback_bundle(&inner.device, now_s());
        inner.save_account()?;
        match registry
            .register(
                &inner.device,
                &inner.cert,
                &bundle,
                &inner.handle,
                email,
                discoverable,
            )
            .await
        {
            Err(ow_registry::RegistryError::Taken) => {
                // Our identity may already own the name (account created from another device).
                registry
                    .put_device(&inner.device, &inner.cert, &bundle)
                    .await
                    .map_err(|e| match e {
                        ow_registry::RegistryError::Rejected(_)
                        | ow_registry::RegistryError::NotFound => ow_registry::RegistryError::Taken,
                        other => other,
                    })?;
                // Attached: the name must be ours, and the email/visibility asked for applied.
                let me = registry.me(&inner.device).await?;
                if me.name != inner.handle.name() {
                    return Err(ow_registry::RegistryError::Taken.into());
                }
                if let Some(email) = email {
                    registry.set_email(&inner.device, Some(email)).await?;
                }
                registry
                    .set_discoverable(&inner.device, discoverable)
                    .await?;
                Ok(())
            }
            other => Ok(other?),
        }
    }

    /// Reads a boolean preference stored with the profile (UI settings).
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn pref(&self, name: &str, default: bool) -> Result<bool> {
        Ok(lock(&self.inner.storage)
            .get_secret(&format!("pref:{name}"))?
            .map_or(default, |v| v.first() == Some(&1)))
    }

    /// Stores a boolean preference.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn set_pref(&self, name: &str, value: bool) -> Result<()> {
        lock(&self.inner.storage).put_secret(
            &format!("pref:{name}"),
            &[u8::from(value)],
            now_ms(),
        )?;
        Ok(())
    }

    /// Deletes our account on the registry (username, email, backup). Local data is kept.
    ///
    /// # Errors
    ///
    /// [`MessagingError::NoRegistry`] or registry errors.
    pub async fn delete_account(&self) -> Result<()> {
        Ok(self.registry()?.delete_account(&self.inner.device).await?)
    }

    /// Hides the "keys changed" warning of a contact without verifying the safety number.
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn ack_key_change(&self, id: ContactId) -> Result<()> {
        lock(&self.inner.storage).delete_secret(&format!("keychanged:{}", id.0))?;
        let _ = self.inner.events.send(MessengerEvent::ContactsChanged);
        Ok(())
    }

    /// Whether encrypted backups are enabled.
    #[must_use]
    pub fn backup_enabled(&self) -> bool {
        self.inner.flag(SECRET_BACKUP_KEY)
    }

    /// Enables encrypted backups with `password` (at least
    /// [`MIN_PASSWORD_LEN`](crate::MIN_PASSWORD_LEN) characters) and uploads one now.
    ///
    /// # Errors
    ///
    /// [`MessagingError::Backup`] (password too short), registry or storage errors.
    pub async fn set_backup_password(&self, password: &str) -> Result<()> {
        self.registry()?;
        let password = Zeroizing::new(password.to_owned());
        let key = tokio::task::spawn_blocking(move || BackupKey::new(&password))
            .await
            .map_err(|_| MessagingError::Backup("interrupted"))??;
        lock(&self.inner.storage).put_secret(SECRET_BACKUP_KEY, &key.to_bytes(), now_ms())?;
        self.backup_now().await
    }

    /// Disables automatic backups on this device (the last backup stays on the server).
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn disable_backup(&self) -> Result<()> {
        lock(&self.inner.storage).delete_secret(SECRET_BACKUP_KEY)?;
        Ok(())
    }

    /// Uploads an encrypted backup now.
    ///
    /// # Errors
    ///
    /// [`MessagingError::Backup`] if backups are not enabled, registry or storage errors.
    pub async fn backup_now(&self) -> Result<()> {
        self.inner.backup_now().await
    }

    /// Restores contacts and history from the encrypted backup of this account (after recovery
    /// or on a new device), and keeps backups enabled with the same password. Returns the number
    /// of restored contacts.
    ///
    /// # Errors
    ///
    /// [`MessagingError::Backup`] (wrong password, no backup...), registry or storage errors.
    pub async fn restore_backup(&self, password: &str) -> Result<usize> {
        let blob = match self.registry()?.get_backup(&self.inner.device).await {
            Err(ow_registry::RegistryError::NotFound) => {
                return Err(MessagingError::Backup("no backup on the server"));
            }
            other => other?,
        };
        let password = Zeroizing::new(password.to_owned());
        let (key, plain) = tokio::task::spawn_blocking(move || BackupKey::open(&password, &blob))
            .await
            .map_err(|_| MessagingError::Backup("interrupted"))??;
        let data: BackupData = serde_json::from_slice(&plain)
            .map_err(|_| MessagingError::Backup("unreadable content"))?;
        let count = self.inner.import_backup(&data)?;
        lock(&self.inner.storage).put_secret(SECRET_BACKUP_KEY, &key.to_bytes(), now_ms())?;
        let _ = self.inner.events.send(MessengerEvent::ContactsChanged);
        Inner::reconnect_all(Arc::clone(&self.inner)).await;
        Ok(count)
    }

    /// Stops the network node.
    pub async fn shutdown(&self) {
        self.inner.p2p.shutdown().await;
    }
}

impl Inner {
    /// Saves address hints of a device (memory + database), most recent first.
    fn remember_addrs(&self, device: &[u8; 32], new: Vec<Multiaddr>) {
        let Ok(peer) = peer_id_of(device) else { return };
        let all = {
            let mut net = lock(&self.net);
            let list = net.addrs.entry(peer).or_default();
            for a in new.into_iter().rev() {
                list.retain(|x| x != &a);
                list.insert(0, a);
            }
            list.truncate(16);
            list.clone()
        };
        let _ = lock(&self.storage).put_secret(
            &key_name("addrs", device),
            &encode_addrs(&all),
            now_ms(),
        );
    }

    /// At startup: load saved addresses and try to reach every contact device. Connections
    /// trigger delivery of pending messages on both sides.
    async fn reconnect_all(self: Arc<Self>) {
        let devices: Vec<[u8; 32]> = {
            let s = lock(&self.storage);
            s.contacts()
                .unwrap_or_default()
                .iter()
                .flat_map(|c| s.contact_devices(c.id).unwrap_or_default())
                .collect()
        };
        for device in devices {
            let Ok(peer) = peer_id_of(&device) else {
                continue;
            };
            let saved = lock(&self.storage)
                .get_secret(&key_name("addrs", &device))
                .ok()
                .flatten()
                .map(|b| decode_addrs(&b))
                .unwrap_or_default();
            lock(&self.net).addrs.entry(peer).or_default().extend(saved);
            let addrs = self.dial_addrs(peer);
            let p2p = self.p2p.clone();
            tokio::spawn(async move {
                let _ = p2p.dial(peer, addrs).await;
            });
        }
    }

    fn flag(&self, name: &str) -> bool {
        lock(&self.storage)
            .get_secret(name)
            .ok()
            .flatten()
            .is_some()
    }

    fn spawn_name_check(self: &Arc<Self>, id: ContactId) {
        if self.registry.is_none() {
            return;
        }
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = inner.check_name(id).await {
                tracing::debug!(%e, contact = id.0, "username check postponed");
            }
        });
    }

    /// Confirms an announced username with the account server. If the server confirms it and
    /// another contact had this name with older keys, that contact's keys changed: merge.
    async fn check_name(self: &Arc<Self>, id: ContactId) -> Result<()> {
        let Some(registry) = &self.registry else {
            return Ok(());
        };
        let Some(contact) = lock(&self.storage).contact(id)? else {
            return Ok(());
        };
        let Ok(handle) = Handle::new(&contact.handle) else {
            return Ok(());
        };
        let found = match registry.lookup(&handle).await {
            Ok(found) => found,
            // Not discoverable or unknown: stays unconfirmed.
            Err(ow_registry::RegistryError::NotFound) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if found.master.to_bytes() != contact.master_pub {
            // Someone announces a name that belongs to another identity: keep the warning.
            tracing::warn!(
                contact = id.0,
                "announced username does not match the account server"
            );
            return Ok(());
        }
        let s = lock(&self.storage);
        s.delete_secret(&format!("unconfirmed:{}", id.0))?;
        let older = s.contacts()?.into_iter().find(|c| {
            c.id != id && c.handle == contact.handle && c.master_pub != contact.master_pub
        });
        if let Some(old) = older {
            s.merge_contact(id, old.id, &contact.master_pub)?;
            s.put_secret(&format!("keychanged:{}", old.id.0), &[1], now_ms())?;
            s.delete_secret(&format!("unconfirmed:{}", old.id.0))?;
            drop(s);
            let _ = self
                .events
                .send(MessengerEvent::KeyChanged { contact: old.id });
        } else {
            drop(s);
        }
        let _ = self.events.send(MessengerEvent::ContactsChanged);
        Ok(())
    }

    /// At startup: re-check unconfirmed usernames (the server may have been unreachable).
    async fn startup_checks(self: Arc<Self>) {
        // Collect first: `flag` takes the storage lock again (std mutexes are not reentrant).
        let contacts = lock(&self.storage).contacts().unwrap_or_default();
        let ids: Vec<ContactId> = contacts
            .into_iter()
            .map(|c| c.id)
            .filter(|id| self.flag(&format!("unconfirmed:{}", id.0)))
            .collect();
        for id in ids {
            let _ = self.check_name(id).await;
        }
    }

    /// Uploads a backup every [`BACKUP_INTERVAL`] when something changed.
    async fn backup_loop(weak: std::sync::Weak<Self>) {
        loop {
            tokio::time::sleep(BACKUP_INTERVAL).await;
            let Some(inner) = weak.upgrade() else { return };
            if inner
                .backup_dirty
                .load(std::sync::atomic::Ordering::Relaxed)
                && inner.flag(SECRET_BACKUP_KEY)
            {
                match inner.backup_now().await {
                    Ok(()) => tracing::debug!("automatic backup uploaded"),
                    Err(e) => tracing::debug!(%e, "automatic backup postponed"),
                }
            }
        }
    }

    /// Builds the backup content, shrinking the history until it fits the budget.
    fn export_backup(&self) -> Result<Vec<u8>> {
        let mut per_contact = 20_000u32;
        loop {
            let data = {
                let s = lock(&self.storage);
                let read_receipts = s
                    .get_secret(SETTING_READ_RECEIPTS)?
                    .is_none_or(|v| v.first() != Some(&0));
                let mut contacts = Vec::new();
                for c in s.contacts()? {
                    let mut devices = Vec::new();
                    for d in s.contact_devices(c.id)? {
                        let Some(cert) = s.contact_device_cert(&d)? else {
                            continue;
                        };
                        devices.push(BackupDevice {
                            cert: b64(&cert),
                            bundle: s.get_secret(&key_name("bundle", &d))?.map(|b| b64(&b)),
                        });
                    }
                    let messages = s
                        .contact_messages(c.id, per_contact)?
                        .into_iter()
                        .map(|m| BackupMessage {
                            msg_id: b64(&m.msg_id),
                            sender_device: b64(&m.sender_device),
                            outgoing: m.outgoing,
                            sent_at: m.sent_at,
                            received_at: m.received_at,
                            status: m.status as u8,
                            body: m.body,
                        })
                        .collect();
                    contacts.push(BackupContact {
                        handle: c.handle,
                        master_pub: b64(&c.master_pub),
                        verified: c.verified,
                        devices,
                        messages,
                    });
                }
                BackupData {
                    version: 1,
                    created_at: now_ms(),
                    read_receipts,
                    contacts,
                }
            };
            let json = serde_json::to_vec(&data).map_err(|_| MessagingError::Backup("encoding"))?;
            if json.len() <= BACKUP_BUDGET || per_contact <= 100 {
                return Ok(json);
            }
            per_contact /= 2;
        }
    }

    async fn backup_now(&self) -> Result<()> {
        let registry = self.registry.as_ref().ok_or(MessagingError::NoRegistry)?;
        let key = lock(&self.storage)
            .get_secret(SECRET_BACKUP_KEY)?
            .and_then(|b| BackupKey::from_bytes(&b))
            .ok_or(MessagingError::Backup("backups are not enabled"))?;
        self.backup_dirty
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let json = Zeroizing::new(self.export_backup()?);
        let blob = key.seal(&json)?;
        if let Err(e) = registry.put_backup(&self.device, blob).await {
            self.backup_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return Err(e.into());
        }
        Ok(())
    }

    /// Imports contacts and history. Existing data is kept (messages are deduplicated).
    fn import_backup(&self, data: &BackupData) -> Result<usize> {
        use base64::Engine;
        let engine = &base64::engine::general_purpose::STANDARD;
        let s = lock(&self.storage);
        s.put_secret(
            SETTING_READ_RECEIPTS,
            &[u8::from(data.read_receipts)],
            now_ms(),
        )?;
        let mut count = 0;
        for c in &data.contacts {
            let Some(master) = unb64::<32>(&c.master_pub) else {
                continue;
            };
            if master == self.master.to_bytes() {
                continue;
            }
            let id = s.upsert_contact(&NewContact {
                master_pub: master,
                handle: c.handle.clone(),
                display_name: None,
                added_at: now_ms(),
            })?;
            if c.verified {
                s.set_contact_verified(id, true)?;
            }
            for d in &c.devices {
                let Ok(cert_bytes) = engine.decode(&d.cert) else {
                    continue;
                };
                // Only certificates of this contact's identity are restored.
                let Ok(cert) = DeviceCertificate::from_bytes(&cert_bytes) else {
                    continue;
                };
                if cert.master().to_bytes() != master {
                    continue;
                }
                let device = cert.device().to_bytes();
                s.add_contact_device(id, &device, &cert_bytes)?;
                let bundle = d.bundle.as_deref().and_then(|b| engine.decode(b).ok());
                if let Some(bundle) = bundle {
                    if PrekeyBundle::from_bytes(&bundle, cert.device()).is_ok() {
                        s.put_secret(&key_name("bundle", &device), &bundle, now_ms())?;
                    }
                }
            }
            let conv = s.dm_conversation(id, now_ms())?;
            for m in &c.messages {
                let (Some(msg_id), Some(sender)) =
                    (unb64::<16>(&m.msg_id), unb64::<32>(&m.sender_device))
                else {
                    continue;
                };
                let status = match m.status {
                    0 => MessageStatus::Pending,
                    1 => MessageStatus::Sent,
                    2 => MessageStatus::Delivered,
                    3 => MessageStatus::Read,
                    _ => MessageStatus::Received,
                };
                s.insert_message(&NewMessage {
                    conversation: conv,
                    msg_id,
                    sender_device: sender,
                    outgoing: m.outgoing,
                    sent_at: m.sent_at,
                    received_at: m.received_at,
                    status,
                    body: m.body.clone(),
                })?;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Known direct addresses of `peer`, then circuits through our relays.
    fn dial_addrs(&self, peer: PeerId) -> Vec<Multiaddr> {
        let mut addrs = lock(&self.net)
            .addrs
            .get(&peer)
            .cloned()
            .unwrap_or_default();
        addrs.extend(self.relays.iter().map(|r| circuit_addr(r, peer)));
        addrs
    }

    fn contact_info(&self, c: ow_storage::Contact) -> Result<ContactInfo> {
        let devices = lock(&self.storage).contact_devices(c.id)?;
        let online = {
            let net = lock(&self.net);
            devices
                .iter()
                .filter_map(|d| peer_id_of(d).ok())
                .any(|p| net.online.contains(&p))
        };
        let safety = PublicKey::from_bytes(&c.master_pub)
            .map(|k| safety_number(&self.master, &k))
            .unwrap_or_default();
        Ok(ContactInfo {
            id: c.id,
            handle: c.handle,
            display_name: c.display_name,
            verified: c.verified,
            online,
            safety_number: safety,
            key_changed: self.flag(&format!("keychanged:{}", c.id.0)),
            name_unconfirmed: self.flag(&format!("unconfirmed:{}", c.id.0)),
            unread: lock(&self.storage).unread_count(c.id)?,
        })
    }

    fn save_account(&self) -> Result<()> {
        let pickled = lock(&self.crypto).account.pickle(&self.pickle_key);
        lock(&self.storage).put_secret(SECRET_ACCOUNT, pickled.as_bytes(), now_ms())?;
        Ok(())
    }

    /// Returns the sessions with `device`, loading them from storage on first use.
    fn sessions<'a>(&self, crypto: &'a mut Crypto, device: &PublicKey) -> &'a mut PeerSessions {
        let key = device.to_bytes();
        crypto.peers.entry(key).or_insert_with(|| {
            lock(&self.storage)
                .get_secret(&key_name("olm:s", &key))
                .ok()
                .flatten()
                .and_then(|p| {
                    let text = core::str::from_utf8(&p).ok()?.to_owned();
                    PeerSessions::from_pickle(*device, &text, &self.pickle_key).ok()
                })
                .unwrap_or_else(|| PeerSessions::new(*device))
        })
    }

    /// Persists the account and the sessions with `device`.
    fn save_crypto(&self, crypto: &Crypto, device: &[u8; 32]) -> Result<()> {
        let account = crypto.account.pickle(&self.pickle_key);
        let sessions = crypto.peers.get(device).map(|p| p.pickle(&self.pickle_key));
        let s = lock(&self.storage);
        s.put_secret(SECRET_ACCOUNT, account.as_bytes(), now_ms())?;
        if let Some(p) = sessions {
            s.put_secret(&key_name("olm:s", device), p.as_bytes(), now_ms())?;
        }
        Ok(())
    }

    /// Encrypts a text frame for one device.
    fn encrypt_for(&self, device: &[u8; 32], message: &StoredMessage) -> Result<Vec<u8>> {
        let (confirmed, bundle) = {
            let s = lock(&self.storage);
            (
                s.get_secret(&key_name("confirmed", device))?.is_some(),
                s.get_secret(&key_name("bundle", device))?,
            )
        };
        let intro = (!confirmed).then(|| Intro {
            name: self.handle.name().to_owned(),
            master_pub: self.master.to_bytes(),
            cert: self.cert.to_bytes(),
        });
        let frame = TextFrame {
            msg_id: message.msg_id,
            sent_at: message.sent_at,
            intro,
            body: message.body.clone(),
        }
        .encode();
        self.encrypt_payload(device, &frame, bundle)
    }

    /// Encrypts a frame for one device, starting a session from its bundle if needed.
    fn encrypt_payload(
        &self,
        device: &[u8; 32],
        frame: &[u8],
        bundle: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<Vec<u8>> {
        let dev = PublicKey::from_bytes(device)?;
        let mut crypto = lock(&self.crypto);
        let crypto = &mut *crypto;
        if !self.sessions(crypto, &dev).is_established() {
            let bundle =
                bundle.ok_or(ow_crypto::CryptoError::Session("no prekey bundle".into()))?;
            let bundle = PrekeyBundle::from_bytes(&bundle, &dev)?;
            let account = &crypto.account;
            let peer = crypto
                .peers
                .get_mut(device)
                .ok_or(MessagingError::UnknownContact)?;
            peer.start(account, &bundle)?;
        }
        let peer = crypto
            .peers
            .get_mut(device)
            .ok_or(MessagingError::UnknownContact)?;
        let envelope = peer.encrypt(&mut crypto.account, &self.device, frame, now_s())?;
        self.save_crypto(crypto, device)?;
        Ok(envelope.to_bytes())
    }

    /// Starts delivering pending messages and receipts to `contact`. If a delivery is already
    /// running, it runs one more pass when done (so nothing queued meanwhile is missed).
    fn spawn_delivery(self: &Arc<Self>, contact: ContactId) {
        {
            let mut net = lock(&self.net);
            if let Some(rerun) = net.delivering.get_mut(&contact.0) {
                *rerun = true;
                return;
            }
            net.delivering.insert(contact.0, false);
        }
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(e) = inner.deliver(contact).await {
                    tracing::debug!(%e, contact = contact.0, "delivery postponed");
                }
                let mut net = lock(&inner.net);
                match net.delivering.get_mut(&contact.0) {
                    Some(rerun) if *rerun => *rerun = false,
                    _ => {
                        net.delivering.remove(&contact.0);
                        break;
                    }
                }
            }
        });
    }

    /// Sends all undelivered messages of a conversation, in order. Stops at the first failure;
    /// the remaining messages are retried when the contact is seen again.
    async fn deliver(self: &Arc<Self>, contact: ContactId) -> Result<()> {
        let (devices, pending) = {
            let s = lock(&self.storage);
            let conv = s.dm_conversation(contact, now_ms())?;
            (s.contact_devices(contact)?, s.undelivered(conv)?)
        };
        let receipts_key = format!("receipts:{}", contact.0);
        let receipts: Vec<[u8; 16]> = lock(&self.storage)
            .get_secret(&receipts_key)?
            .map(|b| {
                b.chunks_exact(16)
                    .map(|c| {
                        let mut id = [0u8; 16];
                        id.copy_from_slice(c);
                        id
                    })
                    .collect()
            })
            .unwrap_or_default();
        if pending.is_empty() && receipts.is_empty() {
            return Ok(());
        }
        for device in devices {
            let peer = peer_id_of(&device)?;
            self.p2p.dial(peer, self.dial_addrs(peer)).await?;
            for m in &pending {
                let envelope = self.encrypt_for(&device, m)?;
                let reply = self.p2p.send(peer, envelope).await?;
                if reply == ACK_OK {
                    lock(&self.storage)
                        .advance_message_status(&m.msg_id, MessageStatus::Delivered)?;
                    let _ = self.events.send(MessengerEvent::Status {
                        contact,
                        msg_id: m.msg_id,
                        status: MessageStatus::Delivered,
                    });
                } else {
                    return Err(MessagingError::P2p(ow_p2p::P2pError::Send(
                        "rejected".into(),
                    )));
                }
            }
            for chunk in receipts.chunks(MAX_RECEIPT_IDS) {
                let bundle = lock(&self.storage).get_secret(&key_name("bundle", &device))?;
                let envelope =
                    self.encrypt_payload(&device, &Frame::encode_receipt(chunk), bundle)?;
                self.p2p.send(peer, envelope).await?;
            }
        }
        if !receipts.is_empty() {
            lock(&self.storage).delete_secret(&receipts_key)?;
        }
        Ok(())
    }

    /// Decrypts and stores an incoming message.
    fn receive(self: &Arc<Self>, from: PeerId, payload: &[u8]) -> Result<()> {
        let device = device_key_of(&from).ok_or(ow_crypto::CryptoError::WrongDevice)?;
        let dev = PublicKey::from_bytes(&device)?;
        let envelope = Envelope::from_bytes(payload)?;

        let mut crypto = lock(&self.crypto);
        let crypto = &mut *crypto;
        self.sessions(crypto, &dev);
        let peer = crypto
            .peers
            .get_mut(&device)
            .ok_or(MessagingError::UnknownContact)?;
        let plaintext = Zeroizing::new(peer.decrypt(&mut crypto.account, &envelope, &dev)?);
        let frame = match Frame::decode(&plaintext)? {
            Frame::Text(frame) => frame,
            Frame::Receipt(ids) => {
                let s = lock(&self.storage);
                let contact = s
                    .contact_by_device(&device)?
                    .ok_or(MessagingError::UnknownContact)?
                    .id;
                drop(s);
                self.save_crypto(crypto, &device)?;
                let s = lock(&self.storage);
                for msg_id in ids {
                    s.advance_message_status(&msg_id, MessageStatus::Read)?;
                    let _ = self.events.send(MessengerEvent::Status {
                        contact,
                        msg_id,
                        status: MessageStatus::Read,
                    });
                }
                return Ok(());
            }
        };

        let mut new_from_intro = None;
        let s = lock(&self.storage);
        let contact = match s.contact_by_device(&device)? {
            Some(c) => c.id,
            None => {
                // Unknown device: it must introduce itself with a valid certificate.
                let intro = frame.intro.as_ref().ok_or(MessagingError::UnknownContact)?;
                let cert = DeviceCertificate::from_bytes(&intro.cert)?;
                if cert.device() != &dev || cert.master().to_bytes() != intro.master_pub {
                    return Err(MessagingError::Crypto(ow_crypto::CryptoError::WrongDevice));
                }
                let handle = Handle::new(&intro.name)?;
                let id = s.upsert_contact(&NewContact {
                    master_pub: intro.master_pub,
                    handle: handle.to_string(),
                    display_name: None,
                    added_at: now_ms(),
                })?;
                s.add_contact_device(id, &device, &intro.cert)?;
                // Announced by the sender: unconfirmed until the account server agrees.
                s.put_secret(&format!("unconfirmed:{}", id.0), &[1], now_ms())?;
                let _ = self
                    .events
                    .send(MessengerEvent::ContactRequest { contact: id });
                new_from_intro = Some(id);
                id
            }
        };
        drop(s);
        // Persist crypto state only once the sender is accepted.
        self.save_crypto(crypto, &device)?;

        let s = lock(&self.storage);
        s.put_secret(&key_name("confirmed", &device), &[1], now_ms())?;
        let conv = s.dm_conversation(contact, now_ms())?;
        let new = NewMessage {
            conversation: conv,
            msg_id: frame.msg_id,
            sender_device: device,
            outgoing: false,
            sent_at: frame.sent_at,
            received_at: Some(now_ms()),
            status: MessageStatus::Received,
            body: frame.body,
        };
        if let Some(id) = s.insert_message(&new)? {
            let message = StoredMessage {
                id,
                conversation: conv,
                msg_id: new.msg_id,
                sender_device: device,
                outgoing: false,
                sent_at: new.sent_at,
                received_at: new.received_at,
                status: MessageStatus::Received,
                body: new.body,
            };
            let _ = self
                .events
                .send(MessengerEvent::Message { contact, message });
        }
        drop(s);
        self.backup_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(id) = new_from_intro {
            self.spawn_name_check(id);
        }
        Ok(())
    }

    fn contact_of_peer(&self, peer: &PeerId) -> Option<ContactId> {
        let device = device_key_of(peer)?;
        lock(&self.storage)
            .contact_by_device(&device)
            .ok()
            .flatten()
            .map(|c| c.id)
    }

    async fn event_loop(self: Arc<Self>, mut events: mpsc::Receiver<P2pEvent>) {
        while let Some(event) = events.recv().await {
            match event {
                P2pEvent::Listening(addr) => {
                    let unspecified = addr.iter().any(|p| match p {
                        ow_p2p::multiaddr::Protocol::Ip4(ip) => ip.is_unspecified(),
                        ow_p2p::multiaddr::Protocol::Ip6(ip) => ip.is_unspecified(),
                        _ => false,
                    });
                    if !unspecified {
                        lock(&self.net).listening.push(addr);
                    }
                }
                P2pEvent::Message {
                    from,
                    payload,
                    token,
                } => {
                    let reply = match self.receive(from, &payload) {
                        Ok(()) => ACK_OK,
                        Err(e) => {
                            tracing::warn!(%e, %from, "incoming message rejected");
                            ACK_ERR
                        }
                    };
                    let _ = self.p2p.reply(token, reply.to_vec()).await;
                }
                P2pEvent::PeerConnected { peer, .. } => {
                    lock(&self.net).online.insert(peer);
                    if let Some(contact) = self.contact_of_peer(&peer) {
                        let _ = self.events.send(MessengerEvent::Presence {
                            contact,
                            online: true,
                        });
                        self.spawn_delivery(contact);
                    }
                }
                P2pEvent::PeerDisconnected(peer) => {
                    lock(&self.net).online.remove(&peer);
                    if let Some(contact) = self.contact_of_peer(&peer) {
                        let _ = self.events.send(MessengerEvent::Presence {
                            contact,
                            online: false,
                        });
                    }
                }
                P2pEvent::Discovered { peer, addrs } => {
                    if let Some(device) = device_key_of(&peer) {
                        self.remember_addrs(&device, addrs);
                    }
                    if let Some(contact) = self.contact_of_peer(&peer) {
                        self.spawn_delivery(contact);
                    }
                }
            }
        }
    }
}
