//! Composed libp2p behaviour.

use std::time::Duration;

use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::{StreamProtocol, dcutr, identify, identity, kad, mdns, ping, relay, request_response};

use crate::codec::{BytesCodec, DM_PROTOCOL, FILE_PROTOCOL};

/// OutpostWave DHT protocol (kept separate from the public IPFS DHT).
const KAD_PROTOCOL: StreamProtocol = StreamProtocol::new("/outpostwave/kad/1");
/// Identify protocol version string.
const IDENTIFY_PROTOCOL: &str = "/outpostwave/id/1";

/// All protocols spoken by an OutpostWave node.
#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    pub relay: relay::client::Behaviour,
    pub relay_server: Toggle<relay::Behaviour>,
    pub dcutr: dcutr::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub dm: request_response::Behaviour<BytesCodec>,
    pub file: request_response::Behaviour<BytesCodec>,
}

/// Relay server settings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RelayServer {
    pub enabled: bool,
    pub max_circuit_bytes: u64,
    pub max_circuit_duration: Duration,
}

impl Behaviour {
    pub(crate) fn new(
        key: &identity::Keypair,
        relay: relay::client::Behaviour,
        enable_mdns: bool,
        relay_server: RelayServer,
    ) -> std::io::Result<Self> {
        let peer_id = key.public().to_peer_id();
        let identify = identify::Behaviour::new(
            identify::Config::new(IDENTIFY_PROTOCOL.to_owned(), key.public())
                .with_agent_version(format!("outpostwave/{}", env!("CARGO_PKG_VERSION"))),
        );
        let kad = kad::Behaviour::with_config(
            peer_id,
            kad::store::MemoryStore::new(peer_id),
            kad::Config::new(KAD_PROTOCOL),
        );
        let mdns = if enable_mdns {
            Some(mdns::tokio::Behaviour::new(
                mdns::Config::default(),
                peer_id,
            )?)
        } else {
            None
        };
        let dm = request_response::Behaviour::with_codec(
            BytesCodec::DM,
            [(DM_PROTOCOL, request_response::ProtocolSupport::Full)],
            request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
        );
        // Many chunk requests run in parallel; a chunk may take a while on a slow uplink.
        let file = request_response::Behaviour::with_codec(
            BytesCodec::FILE,
            [(FILE_PROTOCOL, request_response::ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(90))
                .with_max_concurrent_streams(256),
        );
        // DCUtR upgrades relayed connections to direct ones when hole punching works.
        let relay_server = relay_server.enabled.then(|| {
            relay::Behaviour::new(
                peer_id,
                relay::Config {
                    max_reservations: 4096,
                    max_circuits: 4096,
                    max_circuits_per_peer: 16,
                    max_circuit_duration: relay_server.max_circuit_duration,
                    max_circuit_bytes: relay_server.max_circuit_bytes,
                    ..relay::Config::default()
                },
            )
        });
        Ok(Self {
            relay,
            relay_server: relay_server.into(),
            dcutr: dcutr::Behaviour::new(peer_id),
            identify,
            ping: ping::Behaviour::new(ping::Config::new()),
            kad,
            mdns: mdns.into(),
            dm,
            file,
        })
    }
}
