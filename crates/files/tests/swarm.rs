//! Multi-source download with an in-memory transport: honest, partial, slow and malicious
//! sources; interruption and exact resume.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ow_files::{
    Bitmap, Download, FileKey, Outcome, PartFile, Request, Response, Shared, Status, Transport,
    download, fetch_manifest, prepare, respond,
};
use ow_identity::DeviceKey;
use tokio::sync::watch;

#[derive(Clone)]
struct Peer {
    shared: Shared,
    evil: bool,
    delay: Duration,
}

#[derive(Clone, Default)]
struct Net(Arc<HashMap<&'static str, Peer>>);

impl Transport for Net {
    type Source = &'static str;

    async fn request(&self, source: &&'static str, request: Vec<u8>) -> Result<Vec<u8>, String> {
        let peer = self.0.get(source).ok_or("offline")?.clone();
        tokio::time::sleep(peer.delay).await;
        let req = Request::decode(&request).ok_or("bad request")?;
        let mut resp = respond(&peer.shared, &req);
        if peer.evil && matches!(req, Request::Chunk(..)) && resp.status == Status::Ok {
            resp.body[0] ^= 0xff;
        }
        Ok(resp.encode())
    }
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 253) as u8).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_source_resume_and_forgery() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("game-clip.mp4");
    let content = data(9 * 1024 * 1024 + 12345);
    std::fs::write(&original, &content).unwrap();
    let key = FileKey::generate().unwrap();
    let device = DeviceKey::generate().unwrap();
    let manifest = prepare(&original, &key, &device, |_| {}).unwrap();
    assert_eq!(manifest.count(), 10);

    // Source B: a peer that already downloaded half of the chunks (from its own `.owpart`).
    let b_target = dir.path().join("b").join("clip.mp4");
    let b_part = PartFile::open(&b_target, manifest.size()).unwrap();
    let mut b_have = Bitmap::new(manifest.count());
    for i in (0..manifest.count()).step_by(2) {
        let off = manifest.offset(i) as usize;
        b_part
            .write(off as u64, &content[off..off + manifest.chunk_len(i)])
            .unwrap();
        b_have.set(i);
    }
    let full = Shared {
        manifest: manifest.clone(),
        key: key.clone(),
        path: original.clone(),
        have: None,
    };
    let partial = Shared {
        manifest: manifest.clone(),
        key: key.clone(),
        path: b_part.path().to_path_buf(),
        have: Some(b_have),
    };
    let net = Net(Arc::new(HashMap::from([
        (
            "sender",
            Peer {
                shared: full.clone(),
                evil: false,
                delay: Duration::from_millis(30),
            },
        ),
        (
            "member-b",
            Peer {
                shared: partial,
                evil: false,
                delay: Duration::from_millis(5),
            },
        ),
        (
            "evil",
            Peer {
                shared: full.clone(),
                evil: true,
                delay: Duration::ZERO,
            },
        ),
    ])));

    // The manifest is verified against the file id (an evil source cannot alter it).
    let m = fetch_manifest(&net, &["evil", "sender"], manifest.id())
        .await
        .unwrap();
    assert_eq!(&m, &manifest);

    // First run: interrupted after a few chunks.
    let target = dir.path().join("c").join("clip.mp4");
    let part = PartFile::open(&target, m.size()).unwrap();
    let (cancel_tx, cancel) = watch::channel(false);
    let mut saved = None;
    let outcome = download(
        &net,
        Download {
            manifest: &m,
            key: &key,
            part: &part,
            done: Bitmap::new(m.count()),
            sources: vec!["sender", "member-b", "evil"],
            new_sources: None,
            cancel,
        },
        |p, persist| {
            if let Some(b) = persist {
                saved = Some(b.clone());
            }
            if p.chunks >= 3 {
                let _ = cancel_tx.send(true);
            }
        },
    )
    .await;
    let Outcome::Cancelled(done) = outcome else {
        panic!("{outcome:?}")
    };
    assert!(done.count() >= 3 && done.count() < 10, "{done:?}");
    assert_eq!(saved.as_ref(), Some(&done), "persisted bitmap matches");
    drop(part);

    // Resume from the persisted bitmap, without the evil source (it was caught anyway).
    let part = PartFile::open(&target, m.size()).unwrap();
    let (_tx, cancel) = watch::channel(false);
    let mut sources_seen = 0;
    let outcome = download(
        &net,
        Download {
            manifest: &m,
            key: &key,
            part: &part,
            done,
            sources: vec!["sender", "member-b"],
            new_sources: None,
            cancel,
        },
        |p, _| sources_seen = sources_seen.max(p.sources),
    )
    .await;
    assert_eq!(outcome, Outcome::Complete);
    assert_eq!(sources_seen, 2, "both honest sources used");
    let fin = part.finish().unwrap();
    assert_eq!(std::fs::read(fin).unwrap(), content);
}

#[tokio::test(flavor = "multi_thread")]
async fn stalls_without_source_and_detects_changed_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("a.bin");
    std::fs::write(&original, data(3 * 1024 * 1024)).unwrap();
    let key = FileKey::generate().unwrap();
    let m = prepare(&original, &key, &DeviceKey::generate().unwrap(), |_| {}).unwrap();
    // The sender edits the file after offering it.
    std::fs::write(
        &original,
        data(3 * 1024 * 1024)
            .iter()
            .map(|b| b ^ 1)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let net = Net(Arc::new(HashMap::from([(
        "sender",
        Peer {
            shared: Shared {
                manifest: m.clone(),
                key: key.clone(),
                path: original,
                have: None,
            },
            evil: false,
            delay: Duration::ZERO,
        },
    )])));
    let part = PartFile::open(&dir.path().join("out.bin"), m.size()).unwrap();
    let (_tx, cancel) = watch::channel(false);
    let d = |sources| Download {
        manifest: &m,
        key: &key,
        part: &part,
        done: Bitmap::new(m.count()),
        sources,
        new_sources: None,
        cancel: cancel.clone(),
    };
    assert!(matches!(
        download(&net, d(vec!["sender"]), |_, _| {}).await,
        Outcome::Changed(_)
    ));

    // Nobody online: pauses ("waiting for the sender").

    assert!(matches!(
        download(&net, d(vec!["offline"]), |_, _| {}).await,
        Outcome::Stalled(_)
    ));
    let _ = Response::error(Status::Busy);
}
