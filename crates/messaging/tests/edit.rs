//! Editing and deleting messages between two real P2P nodes, including while the contact is
//! offline.

use std::time::Duration;

use ow_messaging::{MessagingError, Messenger, MessengerEvent, ProfileSetup};
use ow_storage::{DbKey, Storage};
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(storage: Storage, port: u16) -> (Messenger, Receiver<MessengerEvent>) {
    Messenger::start(storage, None, |cfg| {
        cfg.listen = vec![
            format!("/ip4/127.0.0.1/udp/{port}/quic-v1")
                .parse()
                .unwrap(),
        ];
        cfg.mdns = false;
    })
    .unwrap()
}

async fn invite(m: &Messenger) -> String {
    for _ in 0..100 {
        let code = m.invite_code().unwrap();
        if !ow_messaging::Invite::parse(&code).unwrap().addrs.is_empty() {
            return code;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("node never listened");
}

async fn wait<T>(rx: &mut Receiver<MessengerEvent>, f: impl Fn(&MessengerEvent) -> Option<T>) -> T {
    timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(e) = rx.recv().await {
                if let Some(t) = f(&e) {
                    return t;
                }
            }
        }
    })
    .await
    .expect("event not received in time")
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let kb = DbKey::generate().unwrap();
    let pb = dir.path().join("bob.db");
    let (port_a, port_b) = (free_udp_port(), free_udp_port());
    let s = Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap();
    ProfileSetup::create(&s, "alice", "pc").unwrap();
    let (alice, _a_rx) = start(s, port_a);
    let s = Storage::open(&pb, &kb).unwrap();
    ProfileSetup::create(&s, "bob", "pc").unwrap();
    let (bob, mut b_rx) = start(s, port_b);
    let c_bob = alice.add_contact(&invite(&bob).await).unwrap();
    let c_alice = bob.add_contact(&invite(&alice).await).unwrap().id;

    let m1 = alice.send_text(c_bob.id, "salut, ca va ?").unwrap();
    let m2 = alice.send_text(c_bob.id, "message de trop").unwrap();
    wait(&mut b_rx, |e| {
        matches!(e, MessengerEvent::Message { message, .. } if message.msg_id == m2.msg_id)
            .then_some(())
    })
    .await;

    // Edit: Bob sees the new text, marked as edited.
    tokio::time::sleep(Duration::from_millis(300)).await;
    alice
        .edit_message(c_bob.id, &m1.msg_id, "salut, ça va ?")
        .unwrap();
    let edited = wait(&mut b_rx, |e| match e {
        MessengerEvent::MessageEdited { message, .. } => Some(message.clone()),
        _ => None,
    })
    .await;
    assert_eq!(edited.body, "salut, ça va ?");
    assert!(edited.edited_at.is_some());

    // Only the author may delete for everyone, and only text messages can be edited.
    assert!(matches!(
        bob.delete_message(c_alice, &m1.msg_id, true),
        Err(MessagingError::NotAllowed)
    ));
    assert!(matches!(
        bob.edit_message(c_alice, &m1.msg_id, "hack"),
        Err(MessagingError::NotAllowed)
    ));

    // Delete for everyone.
    alice.delete_message(c_bob.id, &m2.msg_id, true).unwrap();
    let gone = wait(&mut b_rx, |e| match e {
        MessengerEvent::MessageDeleted { msg_id, .. } => Some(*msg_id),
        _ => None,
    })
    .await;
    assert_eq!(gone, m2.msg_id);
    let bodies: Vec<String> = bob
        .messages(c_alice, None, 10)
        .unwrap()
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert_eq!(bodies, vec!["salut, ça va ?"]);
    assert!(
        alice
            .messages(c_bob.id, None, 10)
            .unwrap()
            .iter()
            .all(|m| m.msg_id != m2.msg_id)
    );

    // Delete for me only: Alice still has it.
    bob.delete_message(c_alice, &m1.msg_id, false).unwrap();
    assert!(bob.messages(c_alice, None, 10).unwrap().is_empty());
    assert_eq!(alice.messages(c_bob.id, None, 10).unwrap().len(), 1);

    // Offline: Bob is away, Alice edits; the edit is delivered when he is back.
    let m3 = alice.send_text(c_bob.id, "rdv 21h").unwrap();
    wait(&mut b_rx, |e| {
        matches!(e, MessengerEvent::Message { message, .. } if message.msg_id == m3.msg_id)
            .then_some(())
    })
    .await;
    bob.shutdown().await;
    drop(bob);
    drop(b_rx);
    tokio::time::sleep(Duration::from_millis(500)).await;
    alice
        .edit_message(c_bob.id, &m3.msg_id, "rdv 22h finalement")
        .unwrap();
    let (bob, _rx) = start(Storage::open(&pb, &kb).unwrap(), port_b);
    timeout(Duration::from_secs(30), async {
        loop {
            let m = bob.messages(c_alice, None, 10).unwrap();
            if m.first().is_some_and(|m| m.body == "rdv 22h finalement") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("edit not delivered after reconnection");
}
