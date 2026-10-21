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

#[tokio::test(flavor = "multi_thread")]
async fn dm_through_relay() {
    // Relay node.
    let mut cfg = P2pConfig::new([10; 32]);
    cfg.listen = vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()];
    cfg.mdns = false;
    cfg.relay_server = true;
    let (relay, mut r_events) = start(cfg).unwrap();
    let relay_listen = first_listen_addr(&mut r_events).await;
    let relay_addr = relay_listen.with(libp2p::multiaddr::Protocol::P2p(relay.peer_id()));
    tokio::spawn(async move { while r_events.recv().await.is_some() {} });

    // Bob only reachable through the relay (no direct address shared).
    let mut cfg = P2pConfig::new([11; 32]);
    cfg.listen = vec![];
    cfg.mdns = false;
    cfg.relays = vec![relay_addr.clone()];
    let (bob, mut b_events) = start(cfg).unwrap();
    // Wait for the relayed listen address (reservation accepted).
    timeout(T, async {
        loop {
            if let Some(P2pEvent::Listening(a)) = b_events.recv().await {
                if a.to_string().contains("p2p-circuit") {
                    return;
                }
            }
        }
    })
    .await
    .expect("no relay reservation");
    let b2 = bob.clone();
    tokio::spawn(async move {
        while let Some(e) = b_events.recv().await {
            if let P2pEvent::Message { payload, token, .. } = e {
                b2.reply(token, payload).await.unwrap();
            }
        }
    });

    let mut cfg = P2pConfig::new([12; 32]);
    cfg.listen = vec![];
    cfg.mdns = false;
    let (alice, _a) = start(cfg).unwrap();
    let via = ow_p2p::circuit_addr(&relay_addr, bob.peer_id());
    timeout(T, alice.dial(bob.peer_id(), vec![via]))
        .await
        .unwrap()
        .unwrap();
    let reply = timeout(T, alice.send(bob.peer_id(), b"via relay".to_vec()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, b"via relay");
}

#[tokio::test(flavor = "multi_thread")]
async fn file_protocol_large_responses() {
    let (a, mut a_events) = node(21, "/ip4/127.0.0.1/udp/0/quic-v1");
    let (b, mut b_events) = node(22, "/ip4/127.0.0.1/udp/0/quic-v1");
    let b_addr = first_listen_addr(&mut b_events).await;
    tokio::spawn(async move { while a_events.recv().await.is_some() {} });
    let b2 = b.clone();
    tokio::spawn(async move {
        while let Some(e) = b_events.recv().await {
            if let P2pEvent::FileRequest { payload, token, .. } = e {
                // Answers with a full 1 MiB "chunk".
                let b3 = b2.clone();
                tokio::spawn(async move {
                    b3.file_reply(token, vec![payload[0]; 1024 * 1024])
                        .await
                        .unwrap();
                });
            }
        }
    });
    timeout(T, a.dial(b.peer_id(), vec![b_addr]))
        .await
        .unwrap()
        .unwrap();
    let reqs = (0..16u8).map(|i| a.file_request(b.peer_id(), vec![i]));
    let replies = timeout(T, futures::future::join_all(reqs)).await.unwrap();
    for (i, r) in replies.into_iter().enumerate() {
        let r = r.unwrap();
        assert_eq!(r.len(), 1024 * 1024);
        assert_eq!(usize::from(r[0]), i);
    }
}
