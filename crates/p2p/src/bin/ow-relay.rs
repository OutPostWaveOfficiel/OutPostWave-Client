//! `ow-relay`: a standalone OutpostWave relay node (circuit relay v2 + DHT + hole punching help).
//!
//! Peers behind NAT reserve a slot on it; other peers reach them through it without learning their
//! IP address, then try to upgrade to a direct connection (DCUtR).
//!
//! Usage: `ow-relay [--dev] [--port <udp/tcp port, default 4001>] [--key-file <path>] [--external <multiaddr>]...`
//!
//! `--dev` uses a fixed, public key so clients find it without configuration (development only).
//!
//! The key file keeps the peer id stable across restarts (created on first run).

use std::path::PathBuf;

use ow_p2p::{P2pConfig, P2pEvent, start};

fn arg_values(name: &str) -> Vec<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .filter(|w| w[0] == name)
        .map(|w| w[1].clone())
        .collect()
}

fn load_or_create_key(path: &PathBuf) -> std::io::Result<[u8; 32]> {
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, key)?;
    Ok(key)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = arg_values("--port")
        .first()
        .map_or(Ok(ow_p2p::DEV_RELAY_PORT), |p| p.parse())?;
    let key_file = PathBuf::from(
        arg_values("--key-file")
            .first()
            .map_or("relay.key", String::as_str),
    );
    let dev = std::env::args().any(|a| a == "--dev");
    let key = if dev {
        ow_p2p::DEV_RELAY_SEED
    } else {
        load_or_create_key(&key_file)?
    };
    let mut cfg = P2pConfig::new(key);
    cfg.listen = [
        format!("/ip4/0.0.0.0/udp/{port}/quic-v1"),
        format!("/ip6/::/udp/{port}/quic-v1"),
        format!("/ip4/0.0.0.0/tcp/{port}"),
        format!("/ip6/::/tcp/{port}"),
    ]
    .iter()
    .filter_map(|a| a.parse().ok())
    .collect();
    cfg.mdns = false;
    cfg.relay_server = true;
    cfg.external_addrs = arg_values("--external")
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();

    let (node, mut events) = start(cfg)?;
    println!("OutpostWave relay — peer id {}", node.peer_id());
    while let Some(event) = events.recv().await {
        match event {
            P2pEvent::Listening(addr) => println!("listening on {addr}/p2p/{}", node.peer_id()),
            P2pEvent::PeerConnected { peer, .. } => println!("+ {peer}"),
            P2pEvent::PeerDisconnected(peer) => println!("- {peer}"),
            // A relay does not handle direct messages.
            P2pEvent::Message { token, .. } => {
                let _ = node.reply(token, b"err".to_vec()).await;
            }
            P2pEvent::Discovered { .. } => {}
        }
    }
    Ok(())
}
