//! The messenger service.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use ow_crypto::{CryptoAccount, Envelope, PeerSessions, PrekeyBundle, pickle_key};
use ow_identity::{
    DeviceCertificate, DeviceKey, Handle, MasterKey, PublicKey, RecoveryPhrase, safety_number,
};
use ow_p2p::{Multiaddr, P2pConfig, P2pEvent, P2pHandle, PeerId, device_key_of, peer_id_of};
use ow_storage::{
    ContactId, LocalIdentity, MessageStatus, NewContact, NewMessage, Storage, StoredMessage,
};
use tokio::sync::{broadcast, mpsc};
use zeroize::Zeroizing;

use crate::wire::{Intro, MAX_TEXT_LEN, TextFrame};
use crate::{Invite, MessagingError, base32};

type Result<T> = core::result::Result<T, MessagingError>;

const ACK_OK: &[u8] = b"ok";
const ACK_ERR: &[u8] = b"err";
const SECRET_ACCOUNT: &str = "olm:account";

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

/// One-shot identity creation on a fresh device.
#[derive(Debug, Clone, Copy)]
pub struct ProfileSetup;

impl ProfileSetup {
    /// Creates a new identity. Returns the recovery phrase, to show **once** to the user.
    ///
    /// # Errors
    ///
    /// [`MessagingError::IdentityExists`], invalid name or device name, storage errors.
    pub fn create(storage: &Storage, name: &str, device_name: &str) -> Result<RecoveryPhrase> {
        if storage.load_local_identity()?.is_some() {
            return Err(MessagingError::IdentityExists);
        }
        let phrase = RecoveryPhrase::generate()?;
        Self::install(storage, &phrase, name, device_name)?;
        Ok(phrase)
    }

    /// Restores an identity from its recovery phrase on a new device.
    ///
    /// Contacts and history are not restored (they live on the other devices).
    ///
    /// # Errors
    ///
    /// [`MessagingError::IdentityExists`], invalid phrase, name or device name, storage errors.
    pub fn restore(storage: &Storage, phrase: &str, name: &str, device_name: &str) -> Result<()> {
        if storage.load_local_identity()?.is_some() {
            return Err(MessagingError::IdentityExists);
        }
        let phrase = RecoveryPhrase::parse(phrase)?;
        Self::install(storage, &phrase, name, device_name)
    }

    fn install(
        storage: &Storage,
        phrase: &RecoveryPhrase,
        name: &str,
        device_name: &str,
    ) -> Result<()> {
        let master = MasterKey::from_phrase(phrase);
        let handle = Handle::new(name, &master.public_key())?;
        let device = DeviceKey::generate()?;
        let cert = DeviceCertificate::issue(&master, &device.public_key(), device_name, now_s())?;
        let account = CryptoAccount::new();
        storage.put_secret(
            SECRET_ACCOUNT,
            account.pickle(&pickle_key(&device)).as_bytes(),
            now_ms(),
        )?;
        // The master secret is not kept: adding a device requires the recovery phrase.
        let mut device_secret = device.to_secret_bytes();
        let id = LocalIdentity {
            handle_name: handle.name().to_owned(),
            device_secret,
            device_cert: cert.to_bytes(),
            master_secret: None,
            created_at: now_ms(),
        };
        zeroize::Zeroize::zeroize(&mut device_secret);
        storage.save_local_identity(&id)?;
        Ok(())
    }
}

/// Contact as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    /// Row id.
    pub id: ContactId,
    /// Handle `name#tag`.
    pub handle: String,
    /// Local nickname.
    pub display_name: Option<String>,
    /// Safety number verified by the user.
    pub verified: bool,
    /// At least one of the contact's devices is connected.
    pub online: bool,
    /// Safety number to compare out of band.
    pub safety_number: String,
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
    delivering: HashSet<i64>,
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
        let handle = Handle::new(&id.handle_name, &master)?;
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
            events,
        });
        inner.save_account()?;
        tokio::spawn(Inner::event_loop(Arc::clone(&inner), p2p_events));
        tokio::spawn(Inner::reconnect_all(Arc::clone(&inner)));
        Ok((Self { inner }, rx))
    }

    /// Subscribes to events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<MessengerEvent> {
        self.inner.events.subscribe()
    }

    /// Our handle `name#tag`.
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
        let inner = &self.inner;
        if invite.master == inner.master {
            return Err(MessagingError::OwnInvite);
        }
        let device = invite.cert.device().to_bytes();
        let id = {
            let s = lock(&inner.storage);
            let id = s.upsert_contact(&NewContact {
                master_pub: invite.master.to_bytes(),
                handle: invite.handle.to_string(),
                display_name: None,
                added_at: now_ms(),
            })?;
            s.add_contact_device(id, &device, &invite.cert.to_bytes())?;
            s.put_secret(
                &key_name("bundle", &device),
                &invite.bundle.to_bytes(),
                now_ms(),
            )?;
            s.dm_conversation(id, now_ms())?;
            id
        };
        inner.remember_addrs(&device, invite.addrs);
        let _ = inner.events.send(MessengerEvent::ContactsChanged);
        Inner::spawn_delivery(inner, id);
        self.contact(id)
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
        lock(&self.inner.storage).set_contact_verified(id, verified)?;
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
            lock(&self.net)
                .addrs
                .entry(peer)
                .or_default()
                .extend(saved.clone());
            let p2p = self.p2p.clone();
            tokio::spawn(async move {
                let _ = p2p.dial(peer, saved).await;
            });
        }
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
        let dev = PublicKey::from_bytes(device)?;
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
        let envelope = peer.encrypt(&mut crypto.account, &self.device, &frame, now_s())?;
        self.save_crypto(crypto, device)?;
        Ok(envelope.to_bytes())
    }

    fn spawn_delivery(self: &Arc<Self>, contact: ContactId) {
        if !lock(&self.net).delivering.insert(contact.0) {
            return; // Already in progress.
        }
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = inner.deliver(contact).await {
                tracing::debug!(%e, contact = contact.0, "delivery postponed");
            }
            lock(&inner.net).delivering.remove(&contact.0);
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
        if pending.is_empty() {
            return Ok(());
        }
        for device in devices {
            let peer = peer_id_of(&device)?;
            let addrs = lock(&self.net)
                .addrs
                .get(&peer)
                .cloned()
                .unwrap_or_default();
            self.p2p.dial(peer, addrs).await?;
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
        let frame = TextFrame::decode(&plaintext)?;

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
                let handle = Handle::new(&intro.name, cert.master())?;
                let id = s.upsert_contact(&NewContact {
                    master_pub: intro.master_pub,
                    handle: handle.to_string(),
                    display_name: None,
                    added_at: now_ms(),
                })?;
                s.add_contact_device(id, &device, &intro.cert)?;
                let _ = self.events.send(MessengerEvent::ContactsChanged);
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
