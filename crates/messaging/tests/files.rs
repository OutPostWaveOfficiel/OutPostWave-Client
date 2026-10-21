//! File transfers between two real P2P nodes on localhost: automatic download of a small file,
//! manual download of a large one, sender going offline mid-transfer and exact resume.

use std::time::Duration;

use ow_messaging::{FileState, Messenger, MessengerEvent, ProfileSetup};
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

/// Waits for a file event matching `f`.
async fn file_event(
    rx: &mut Receiver<MessengerEvent>,
    secs: u64,
    f: impl Fn(&ow_messaging::FileInfo) -> bool,
) -> ow_messaging::FileInfo {
    timeout(Duration::from_secs(secs), async {
        loop {
            match rx.recv().await {
                Ok(MessengerEvent::File { file, .. }) if f(&file) => return file,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .expect("file event not received in time")
}

async fn offer(rx: &mut Receiver<MessengerEvent>) -> [u8; 32] {
    timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(MessengerEvent::Message { message, .. }) = rx.recv().await {
                if let (false, Some(id)) = (message.outgoing, message.file_id) {
                    return id;
                }
            }
        }
    })
    .await
    .expect("offer not received")
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 31 + usize::from(seed)) % 256) as u8)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn send_files_with_interruption_and_resume() {
    let dir = tempfile::tempdir().unwrap();
    let (ka, kb) = (DbKey::generate().unwrap(), DbKey::generate().unwrap());
    let pa = dir.path().join("alice.db");
    let (port_a, port_b) = (free_udp_port(), free_udp_port());
    let s = Storage::open(&pa, &ka).unwrap();
    ProfileSetup::create(&s, "alice", "pc").unwrap();
    let (alice, _a_rx) = start(s, port_a);
    let s = Storage::open(&dir.path().join("bob.db"), &kb).unwrap();
    ProfileSetup::create(&s, "bob", "pc").unwrap();
    let (bob, mut b_rx) = start(s, port_b);
    let downloads = dir.path().join("bob-downloads");
    bob.set_download_dir(downloads.clone());

    let c_bob = alice.add_contact(&invite(&bob).await).unwrap();
    bob.add_contact(&invite(&alice).await).unwrap();

    // 1. Small file: downloaded automatically.
    let small = dir.path().join("notes.txt");
    std::fs::write(&small, pattern(300_000, 1)).unwrap();
    let sent = alice.send_file(c_bob.id, &small).await.unwrap();
    assert_eq!(sent.body, "notes.txt");
    let id = offer(&mut b_rx).await;
    let done = file_event(&mut b_rx, 30, |f| {
        f.file_id == id && f.state == FileState::Complete
    })
    .await;
    let path = done.path.unwrap();
    assert_eq!(path, downloads.join("notes.txt"));
    assert_eq!(std::fs::read(&path).unwrap(), pattern(300_000, 1));

    // 2. Large file: waits for Bob's approval; Alice's upload is limited so it lasts a while.
    let big_len = 30 * 1024 * 1024 + 777;
    let big = dir.path().join("replay.mkv");
    std::fs::write(&big, pattern(big_len, 2)).unwrap();
    alice.set_upload_limit(Some(4 * 1024 * 1024));
    alice.send_file(c_bob.id, &big).await.unwrap();
    let id = offer(&mut b_rx).await;
    let info = bob.file(&id).unwrap();
    assert_eq!(
        (info.state, info.size, info.name.as_str()),
        (FileState::Offered, big_len as u64, "replay.mkv")
    );
    bob.download_file(&id, None).unwrap();
    file_event(&mut b_rx, 30, |f| {
        f.file_id == id && f.bytes >= 6 * 1024 * 1024
    })
    .await;

    // Alice goes offline in the middle of the transfer: Bob pauses ("waiting for Alice").
    alice.shutdown().await;
    drop(alice);
    let waiting = file_event(&mut b_rx, 60, |f| {
        f.file_id == id && f.state == FileState::Waiting
    })
    .await;
    assert!(
        waiting.bytes >= 6 * 1024 * 1024 && waiting.bytes < big_len as u64,
        "{}",
        waiting.bytes
    );
    assert!(downloads.join("replay.mkv.owpart").exists());

    // Alice comes back: the download resumes where it stopped and completes.
    let (alice, _a_rx) = start(Storage::open(&pa, &ka).unwrap(), port_a);
    let resumed = file_event(&mut b_rx, 60, |f| {
        f.file_id == id && f.state == FileState::Downloading
    })
    .await;
    assert!(resumed.bytes >= waiting.bytes, "no restart from zero");
    let done = file_event(&mut b_rx, 60, |f| {
        f.file_id == id && f.state == FileState::Complete
    })
    .await;
    let path = done.path.unwrap();
    assert_eq!(path, downloads.join("replay.mkv"));
    assert!(
        std::fs::read(&path).unwrap() == pattern(big_len, 2),
        "content mismatch"
    );
    assert!(!downloads.join("replay.mkv.owpart").exists());
    drop(alice);
}
