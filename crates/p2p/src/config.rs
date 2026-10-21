//! Node configuration.

use core::fmt;

use libp2p::Multiaddr;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Configuration of a P2P node.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct P2pConfig {
    /// Device Ed25519 secret key (32 bytes). Zeroized on drop.
    pub device_secret: [u8; 32],
    /// Addresses to listen on.
    #[zeroize(skip)]
    pub listen: Vec<Multiaddr>,
    /// Discover peers on the local network with mDNS.
    pub mdns: bool,
    /// DHT bootstrap nodes (must include `/p2p/<peer id>`).
    #[zeroize(skip)]
    pub bootstrap: Vec<Multiaddr>,
    /// Relay nodes to reserve a circuit on (must include `/p2p/<peer id>`), used when this
    /// device is behind a NAT.
    #[zeroize(skip)]
    pub relays: Vec<Multiaddr>,
}

impl P2pConfig {
    /// Default configuration: QUIC and TCP on all interfaces (IPv4 + IPv6), random ports,
    /// mDNS enabled, no bootstrap node and no relay yet.
    #[must_use]
    pub fn new(device_secret: [u8; 32]) -> Self {
        let listen = [
            "/ip4/0.0.0.0/udp/0/quic-v1",
            "/ip6/::/udp/0/quic-v1",
            "/ip4/0.0.0.0/tcp/0",
            "/ip6/::/tcp/0",
        ]
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
        Self {
            device_secret,
            listen,
            mdns: true,
            bootstrap: Vec::new(),
            relays: Vec::new(),
        }
    }
}

impl fmt::Debug for P2pConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("P2pConfig")
            .field("listen", &self.listen)
            .field("mdns", &self.mdns)
            .field("bootstrap", &self.bootstrap)
            .field("relays", &self.relays)
            .finish_non_exhaustive()
    }
}
