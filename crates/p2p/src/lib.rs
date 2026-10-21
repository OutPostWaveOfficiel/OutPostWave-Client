//! P2P transport for OutpostWave, built on libp2p.
//!
//! - Transports: **QUIC** first, TCP + Noise + Yamux as fallback (networks blocking UDP).
//! - Discovery: **mDNS** on the local network, **Kademlia DHT** (OutpostWave-specific protocol
//!   name, separate from IPFS) for the Internet.
//! - NAT traversal: **relay** circuits (official relays or peers) and **DCUtR** hole punching to
//!   upgrade to a direct connection.
//! - Messaging: a request/response protocol `/outpostwave/dm/1` carrying opaque, already
//!   end-to-end encrypted payloads, acknowledged by the receiver.
//!
//! The libp2p identity is the **device key** (`ow-identity`), so a peer id proves which device
//! is on the other end; see [`device_key_of`].
//!
//! The swarm runs in its own Tokio task; the app talks to it through [`P2pHandle`] and receives
//! [`P2pEvent`]s.

mod behaviour;
mod codec;
mod config;
mod error;
mod node;

pub use config::P2pConfig;
pub use error::P2pError;
pub use libp2p::{Multiaddr, PeerId, multiaddr};
pub use node::{P2pEvent, P2pHandle, ReplyToken, device_key_of, peer_id_of, start};

/// Maximum size of a direct-message payload (encrypted envelope), in bytes.
pub const MAX_PAYLOAD: usize = 64 * 1024;
/// Maximum size of a reply (acknowledgement), in bytes.
pub const MAX_REPLY: usize = 1024;
