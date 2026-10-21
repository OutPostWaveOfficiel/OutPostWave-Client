//! Username registration, search and chat through a relay, against a running registry.
//!
//! Ignored by default. Run with a local registry (OutpostWave-Core-Private):
//! `OW_TEST_REGISTRY=http://127.0.0.1:8787 cargo test -p ow-messaging --test registry -- --ignored`

use std::time::Duration;

use ow_messaging::{
    MessageStatus, MessagingError, Messenger, MessengerEvent, ProfileSetup, RegistryClient,
    RegistryError,
};
use ow_p2p::{Multiaddr, P2pConfig, P2pEvent};
use ow_storage::{DbKey, Storage};
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

fn registry() -> RegistryClient {
    let url = std::env::var("OW_TEST_REGISTRY").expect("set OW_TEST_REGISTRY");
    RegistryClient::new(&url).unwrap()
}

fn unique(prefix: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}{}", n % 1_000_000_000)
}

/// Starts a local relay and returns its address (with `/p2p/<relay id>`).
async fn start_relay() -> Multiaddr {
    let mut cfg = P2pConfig::new([77; 32]);
    cfg.listen = vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()];
    cfg.mdns = false;
    cfg.relay_server = true;
    let (relay, mut events) = ow_p2p::start(cfg).unwrap();
    let addr = loop {
        if let Some(P2pEvent::Listening(a)) = events.recv().await {
            break a;
        }
    };
    let addr = addr.with(ow_p2p::multiaddr::Protocol::P2p(relay.peer_id()));
    tokio::spawn(async move {
        let _keep = relay;
        while events.recv().await.is_some() {}
    });
    addr
}

async fn create_with_phrase(
    name: &str,
    email: Option<&str>,
    relay: &Multiaddr,
) -> (Messenger, Receiver<MessengerEvent>, String) {
    let reg = registry();
    let mut p = ProfileSetup::prepare_new(name, "test").unwrap();
    let bundle = p.bundle();
    reg.register(p.device(), p.cert(), &bundle, p.handle(), email, true)
        .await
        .unwrap();
    let storage = Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap();
    let phrase = p.commit(&storage).unwrap().unwrap();
    let phrase = phrase.words().collect::<Vec<_>>().join(" ");
    let (m, rx) = start(storage, reg, relay);
    (m, rx, phrase)
}

fn start(
    storage: Storage,
    reg: RegistryClient,
    relay: &Multiaddr,
) -> (Messenger, Receiver<MessengerEvent>) {
    Messenger::start(storage, Some(reg), |cfg| {
        cfg.listen = vec![];
        cfg.mdns = false;
        cfg.relays = vec![relay.clone()];
    })
    .unwrap()
}

async fn next_message(rx: &mut Receiver<MessengerEvent>) -> ow_messaging::StoredMessage {
    timeout(Duration::from_secs(30), async {
        loop {
            if let MessengerEvent::Message { message, .. } = rx.recv().await.unwrap() {
                if !message.outgoing {
                    return message;
                }
            }
        }
    })
    .await
    .expect("message not delivered")
}

async fn create(
    name: &str,
    email: Option<&str>,
    relay: &Multiaddr,
) -> (Messenger, Receiver<MessengerEvent>) {
    let reg = registry();
    let mut p = ProfileSetup::prepare_new(name, "test").unwrap();
    let bundle = p.bundle();
    reg.register(p.device(), p.cert(), &bundle, p.handle(), email, true)
        .await
        .unwrap();
    let storage = Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap();
    p.commit(&storage).unwrap();
    Messenger::start(storage, Some(reg), |cfg| {
        // No direct address and no mDNS: contacts are reached through the relay only.
        cfg.listen = vec![];
        cfg.mdns = false;
        cfg.relays = vec![relay.clone()];
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running registry (OW_TEST_REGISTRY)"]
async fn register_search_and_chat() {
    let relay = start_relay().await;
    let (alice_name, bob_name) = (unique("alice"), unique("bob"));
    let (alice, _a_rx) = create(&alice_name, None, &relay).await;
    let (bob, mut b_rx) = create(&bob_name, Some("bob@example.com"), &relay).await;
    // Let both nodes reserve their slot on the relay.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // The name is now taken.
    let mut p = ProfileSetup::prepare_new(&alice_name, "x").unwrap();
    let bundle = p.bundle();
    let err = registry()
        .register(p.device(), p.cert(), &bundle, p.handle(), None, true)
        .await;
    assert_eq!(err, Err(RegistryError::Taken));

    // Settings round-trip.
    let me = bob.account().await.unwrap();
    assert_eq!(me.email.as_deref(), Some("bob@example.com"));
    assert!(!me.email_verified);
    assert!(me.discoverable);

    // Alice finds Bob by username and writes to him (through the relay: no IP was published).
    let contact = alice
        .add_contact_by_name(&bob_name.to_uppercase())
        .await
        .unwrap();
    assert_eq!(contact.handle, bob_name);
    alice.send_text(contact.id, "trouvé par pseudo").unwrap();
    let got = timeout(Duration::from_secs(30), async {
        loop {
            if let MessengerEvent::Message { message, .. } = b_rx.recv().await.unwrap() {
                return message;
            }
        }
    })
    .await
    .expect("message not delivered");
    assert_eq!(got.body, "trouvé par pseudo");
    assert_eq!(got.status, MessageStatus::Received);

    // Bob hides himself: he can no longer be found.
    bob.set_discoverable(false).await.unwrap();
    assert!(matches!(
        alice.add_contact_by_name(&bob_name).await,
        Err(MessagingError::Registry(RegistryError::NotFound))
    ));
    assert!(matches!(
        alice.add_contact_by_name(&alice_name).await,
        Err(MessagingError::OwnInvite)
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running registry (OW_TEST_REGISTRY)"]
async fn backup_key_change_and_impersonation() {
    let relay = start_relay().await;
    let (alice_name, bob_name) = (unique("alice"), unique("bob"));
    let (alice, mut a_rx) = create(&alice_name, None, &relay).await;
    let (bob, mut b_rx, bob_phrase) = create_with_phrase(&bob_name, None, &relay).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let c_bob = alice.add_contact_by_name(&bob_name).await.unwrap();
    assert!(!c_bob.name_unconfirmed, "found through the server");
    alice.send_text(c_bob.id, "salut").unwrap();
    next_message(&mut b_rx).await;
    let c_alice = bob.contacts().unwrap()[0].id;
    // The announced name gets confirmed by the server shortly after.
    for _ in 0..50 {
        if !bob.contact(c_alice).unwrap().name_unconfirmed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!bob.contact(c_alice).unwrap().name_unconfirmed);

    // --- Encrypted backup, restored on a second device of Bob (recovery phrase).
    assert!(bob.set_backup_password("short").await.is_err());
    bob.set_backup_password("un mot de passe solide")
        .await
        .unwrap();
    assert!(bob.backup_enabled());
    let reg = registry();
    let mut p = ProfileSetup::prepare_restore(&bob_phrase, &bob_name, "bob phone").unwrap();
    let bundle = p.bundle();
    reg.put_device(p.device(), p.cert(), &bundle).await.unwrap();
    let storage = Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap();
    p.commit(&storage).unwrap();
    let (bob_phone, _rx) = start(storage, reg, &relay);
    assert!(
        bob_phone
            .restore_backup("mauvais mot de passe")
            .await
            .is_err()
    );
    assert_eq!(
        bob_phone
            .restore_backup("un mot de passe solide")
            .await
            .unwrap(),
        1
    );
    let restored = bob_phone.contacts().unwrap();
    assert_eq!(restored[0].handle, alice_name);
    assert_eq!(
        bob_phone.messages(restored[0].id, None, 10).unwrap()[0].body,
        "salut"
    );
    assert!(
        bob_phone.backup_enabled(),
        "backups continue with the same password"
    );

    // --- Key change: Bob deletes his account and re-registers the same name with new keys.
    bob.delete_account().await.unwrap();
    bob.shutdown().await;
    bob_phone.shutdown().await;
    drop((bob, bob_phone));
    let (bob2, _b2_rx) = create(&bob_name, None, &relay).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let c = bob2.add_contact_by_name(&alice_name).await.unwrap();
    bob2.send_text(c.id, "c'est moi, nouvelles clés").unwrap();
    let e = timeout(Duration::from_secs(30), async {
        loop {
            if let MessengerEvent::KeyChanged { contact } = a_rx.recv().await.unwrap() {
                return contact;
            }
        }
    })
    .await
    .expect("key change not detected");
    assert_eq!(e, c_bob.id, "merged into the existing contact");
    let merged = alice.contact(c_bob.id).unwrap();
    assert!(merged.key_changed);
    assert!(!merged.verified);
    assert_ne!(merged.safety_number, c_bob.safety_number);
    let history = alice.messages(c_bob.id, None, 10).unwrap();
    assert!(
        history
            .iter()
            .any(|m| m.body == "c'est moi, nouvelles clés")
    );
    assert!(history.iter().any(|m| m.body == "salut"));
    assert_eq!(alice.contacts().unwrap().len(), 1);
    alice.ack_key_change(c_bob.id).unwrap();
    assert!(!alice.contact(c_bob.id).unwrap().key_changed);

    // --- Impersonation: someone announces Bob's name with other keys (invite code).
    let (mallory, _m_rx) = create(&unique("mallory"), None, &relay).await;
    let mut fake = ow_messaging::Invite::parse(&mallory.invite_code().unwrap()).unwrap();
    fake.handle = ow_identity::Handle::new(&bob_name).unwrap();
    let fake_contact = alice.add_contact(&fake.to_code()).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        alice.contact(fake_contact.id).unwrap().name_unconfirmed,
        "impersonation flagged"
    );
    assert!(
        !alice.contact(c_bob.id).unwrap().key_changed,
        "real contact untouched"
    );
}
