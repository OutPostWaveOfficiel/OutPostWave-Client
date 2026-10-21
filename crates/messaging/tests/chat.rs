//! Alice and Bob chat through real P2P nodes on localhost.

use std::time::Duration;

use ow_messaging::{MessageStatus, Messenger, MessengerEvent, ProfileSetup};
use ow_storage::{DbKey, Storage};
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(20);

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn open(path: &std::path::Path, key: &DbKey) -> Storage {
    Storage::open(path, key).unwrap()
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

async fn wait_for<F: Fn(&MessengerEvent) -> bool>(
    rx: &mut Receiver<MessengerEvent>,
    f: F,
) -> MessengerEvent {
    timeout(T, async {
        loop {
            let e = rx.recv().await.unwrap();
            if f(&e) {
                return e;
            }
        }
    })
    .await
    .expect("event not received in time")
}

async fn invite_when_listening(m: &Messenger) -> String {
    // Wait until the node reports a concrete listen address.
    for _ in 0..100 {
        let code = m.invite_code().unwrap();
        let parsed = ow_messaging::Invite::parse(&code).unwrap();
        if !parsed.addrs.is_empty() {
            return code;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("node never listened");
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_and_offline_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let (ka, kb) = (DbKey::generate().unwrap(), DbKey::generate().unwrap());
    let (pa, pb) = (dir.path().join("alice.db"), dir.path().join("bob.db"));
    let (port_a, port_b) = (free_udp_port(), free_udp_port());

    let s = open(&pa, &ka);
    ProfileSetup::create(&s, "alice", "Alice PC").unwrap();
    let (alice, mut a_rx) = start(s, port_a);
    let s = open(&pb, &kb);
    let phrase = ProfileSetup::create(&s, "Bob", "Bob PC").unwrap();
    assert_eq!(phrase.words().count(), 24);
    let (bob, mut b_rx) = start(s, port_b);
    assert_eq!(bob.handle(), "bob");

    // Alice adds Bob from his invite and writes first.
    let contact_bob = alice
        .add_contact(&invite_when_listening(&bob).await)
        .unwrap();
    assert_eq!(contact_bob.handle, bob.handle());
    assert!(
        alice
            .add_contact(&invite_when_listening(&alice).await)
            .is_err(),
        "own invite"
    );
    let sent = alice.send_text(contact_bob.id, "Salut Bob !").unwrap();
    assert_eq!(sent.status, MessageStatus::Pending);

    // Bob receives it and Alice is added to his contacts automatically.
    let MessengerEvent::Message {
        contact: contact_alice,
        message,
    } = wait_for(&mut b_rx, |e| matches!(e, MessengerEvent::Message { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(message.body, "Salut Bob !");
    assert!(!message.outgoing);
    let alice_seen_by_bob = bob.contact(contact_alice).unwrap();
    assert_eq!(alice_seen_by_bob.handle, alice.handle());
    // Both sides compute the same safety number.
    assert_eq!(alice_seen_by_bob.safety_number, contact_bob.safety_number);

    wait_for(&mut a_rx, |e| {
        matches!(
            e,
            MessengerEvent::Status {
                status: MessageStatus::Delivered,
                ..
            }
        )
    })
    .await;

    // Bob reads it: Alice sees "read".
    assert_eq!(bob.contact(contact_alice).unwrap().unread, 1);
    assert_eq!(bob.mark_read(contact_alice).unwrap(), 1);
    assert_eq!(bob.mark_read(contact_alice).unwrap(), 0);
    assert_eq!(bob.contact(contact_alice).unwrap().unread, 0);
    wait_for(&mut a_rx, |e| {
        matches!(
            e,
            MessengerEvent::Status {
                status: MessageStatus::Read,
                ..
            }
        )
    })
    .await;
    assert_eq!(
        alice.messages(contact_bob.id, None, 1).unwrap()[0].status,
        MessageStatus::Read
    );

    // Bob answers.
    bob.send_text(contact_alice, "Salut Alice").unwrap();
    let e = wait_for(
        &mut a_rx,
        |e| matches!(e, MessengerEvent::Message { message, .. } if !message.outgoing),
    )
    .await;
    let MessengerEvent::Message { message, .. } = e else {
        unreachable!()
    };
    assert_eq!(message.body, "Salut Alice");

    // Bob goes offline; Alice writes; the message stays pending.
    bob.shutdown().await;
    drop(bob);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let offline = alice.send_text(contact_bob.id, "Tu es là ?").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let latest = &alice.messages(contact_bob.id, None, 1).unwrap()[0];
    assert_eq!(latest.msg_id, offline.msg_id);
    assert_eq!(latest.status, MessageStatus::Pending);

    // Bob restarts on the same port: he reconnects to Alice, who delivers the pending message.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (bob, mut b_rx) = start(open(&pb, &kb), port_b);
    let e = wait_for(&mut b_rx, |e| matches!(e, MessengerEvent::Message { .. })).await;
    let MessengerEvent::Message { message, .. } = e else {
        unreachable!()
    };
    assert_eq!(message.body, "Tu es là ?");
    // Read receipts can be disabled.
    bob.set_read_receipts(false).unwrap();
    assert!(!bob.read_receipts().unwrap());
    // History survived the restart (encrypted database).
    assert_eq!(bob.messages(contact_alice, None, 10).unwrap().len(), 3);
}
