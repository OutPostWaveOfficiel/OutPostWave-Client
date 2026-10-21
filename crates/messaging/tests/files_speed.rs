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
#[ignore = "benchmark"]
async fn speed() {
    let dir = tempfile::tempdir().unwrap();
    let (port_a, port_b) = (free_udp_port(), free_udp_port());
    let s = Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap();
    ProfileSetup::create(&s, "alice", "pc").unwrap();
    let (alice, _a_rx) = start(s, port_a);
    let s = Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap();
    ProfileSetup::create(&s, "bob", "pc").unwrap();
    let (bob, mut b_rx) = start(s, port_b);
    bob.set_download_dir(dir.path().join("dl"));
    let c_bob = alice.add_contact(&invite(&bob).await).unwrap();
    bob.add_contact(&invite(&alice).await).unwrap();
    let big = dir.path().join("big.bin");
    let len = 512 * 1024 * 1024;
    std::fs::write(&big, pattern(len, 3)).unwrap();
    let t0 = std::time::Instant::now();
    alice.send_file(c_bob.id, &big).await.unwrap();
    println!("prepare: {:?}", t0.elapsed());
    let id = offer(&mut b_rx).await;
    let t1 = std::time::Instant::now();
    bob.download_file(&id, None).unwrap();
    file_event(&mut b_rx, 300, |f| {
        f.file_id == id && f.state == FileState::Complete
    })
    .await;
    let secs = t1.elapsed().as_secs_f64();
    println!("transfer: {secs:.2}s = {:.0} MB/s", len as f64 / secs / 1e6);
}
