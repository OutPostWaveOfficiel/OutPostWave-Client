//! Two local nodes exchange a direct message over QUIC and over TCP.

use std::time::Duration;

use ow_p2p::{Multiaddr, P2pConfig, P2pEvent, P2pHandle, device_key_of, peer_id_of, start};
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(15);

fn node(seed: u8, listen: &str) -> (P2pHandle, Receiver<P2pEvent>) {
    let mut cfg = P2pConfig::new([seed; 32]);
    cfg.listen = vec![listen.parse().unwrap()];
    cfg.mdns = false;
    start(cfg).unwrap()
}

async fn first_listen_addr(events: &mut Receiver<P2pEvent>) -> Multiaddr {
    loop {
        if let Some(P2pEvent::Listening(addr)) = timeout(T, events.recv()).await.unwrap() {
            return addr;
        }
    }
}

async fn exchange(listen: &str) {
    let (a, _a_events) = node(1, listen);
    let (b, mut b_events) = node(2, listen);
    let b_addr = first_listen_addr(&mut b_events).await;

    // B answers every message with "ack:<payload>".
    let b2 = b.clone();
    let a_id = a.peer_id();
    let responder = tokio::spawn(async move {
        loop {
            match b_events.recv().await {
                Some(P2pEvent::Message {
                    from,
                    payload,
                    token,
                }) => {
                    assert_eq!(from, a_id);
                    let mut reply = b"ack:".to_vec();
                    reply.extend_from_slice(&payload);
                    b2.reply(token, reply).await.unwrap();
                    return;
                }
                Some(_) => {}
                None => panic!("events closed"),
            }
        }
    });

    timeout(T, a.dial(b.peer_id(), vec![b_addr]))
        .await
        .unwrap()
        .unwrap();
    let reply = timeout(T, a.send(b.peer_id(), b"hello".to_vec()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, b"ack:hello");
    responder.await.unwrap();
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dm_over_quic() {
    exchange("/ip4/127.0.0.1/udp/0/quic-v1").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dm_over_tcp() {
    exchange("/ip4/127.0.0.1/tcp/0").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_payload_is_rejected() {
    let (a, _) = node(3, "/ip4/127.0.0.1/udp/0/quic-v1");
    let err = a
        .send(a.peer_id(), vec![0; ow_p2p::MAX_PAYLOAD + 1])
        .await
        .unwrap_err();
    assert_eq!(err, ow_p2p::P2pError::PayloadTooLarge);
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_id_is_the_device_key() {
    // Same derivation as ow-identity: Ed25519 secret -> public key.
    let secret = [4u8; 32];
    let device_pub = ed25519_dalek::SigningKey::from_bytes(&secret)
        .verifying_key()
        .to_bytes();
    let (node, _) = node(4, "/ip4/127.0.0.1/udp/0/quic-v1");
    assert_eq!(node.peer_id(), peer_id_of(&device_pub).unwrap());
    assert_eq!(device_key_of(&node.peer_id()), Some(device_pub));
}
