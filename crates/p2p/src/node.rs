//! Node task, command handle and events.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use libp2p::core::ConnectedPoint;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, OutboundRequestId, ResponseChannel};
use libp2p::swarm::{SwarmEvent, dial_opts::DialOpts};
use libp2p::{Multiaddr, PeerId, Swarm, identify, identity, mdns, noise, tcp, yamux};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroize;

use crate::behaviour::{Behaviour, BehaviourEvent};
use crate::{MAX_PAYLOAD, P2pConfig, P2pError};

/// Identifies an inbound message to reply to with [`P2pHandle::reply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplyToken(u64);

/// Events emitted by the node.
#[derive(Debug)]
pub enum P2pEvent {
    /// The node listens on a new address.
    Listening(Multiaddr),
    /// A connection to a peer was established.
    PeerConnected {
        /// Remote peer.
        peer: PeerId,
        /// `true` if the connection goes through a relay.
        relayed: bool,
    },
    /// The last connection to a peer was closed.
    PeerDisconnected(PeerId),
    /// A peer was found on the local network (mDNS).
    Discovered {
        /// Peer found.
        peer: PeerId,
        /// Its addresses.
        addrs: Vec<Multiaddr>,
    },
    /// A direct message arrived. Answer it with [`P2pHandle::reply`].
    Message {
        /// Sender.
        from: PeerId,
        /// Opaque (encrypted) payload.
        payload: Vec<u8>,
        /// Token to reply with.
        token: ReplyToken,
    },
}

enum Command {
    Dial {
        peer: PeerId,
        addrs: Vec<Multiaddr>,
        done: oneshot::Sender<Result<(), P2pError>>,
    },
    Send {
        peer: PeerId,
        payload: Vec<u8>,
        done: oneshot::Sender<Result<Vec<u8>, P2pError>>,
    },
    Reply {
        token: ReplyToken,
        payload: Vec<u8>,
    },
    ListenAddrs {
        done: oneshot::Sender<Vec<Multiaddr>>,
    },
    Shutdown,
}

/// Cheap, cloneable handle to a running node.
#[derive(Clone, Debug)]
pub struct P2pHandle {
    peer_id: PeerId,
    cmd: mpsc::Sender<Command>,
}

impl P2pHandle {
    /// Local peer id (derived from the device key).
    #[must_use]
    pub const fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    async fn call<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Result<T, P2pError> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(make(tx))
            .await
            .map_err(|_| P2pError::Stopped)?;
        rx.await.map_err(|_| P2pError::Stopped)
    }

    /// Current listen addresses.
    ///
    /// # Errors
    ///
    /// [`P2pError::Stopped`] if the node is not running.
    pub async fn listen_addrs(&self) -> Result<Vec<Multiaddr>, P2pError> {
        self.call(|done| Command::ListenAddrs { done }).await
    }

    /// Connects to `peer`, trying `addrs` (may be empty if the peer is already known).
    ///
    /// # Errors
    ///
    /// [`P2pError::Dial`] if no connection could be established.
    pub async fn dial(&self, peer: PeerId, addrs: Vec<Multiaddr>) -> Result<(), P2pError> {
        self.call(|done| Command::Dial { peer, addrs, done })
            .await?
    }

    /// Sends a payload to `peer` and waits for its reply (acknowledgement).
    ///
    /// # Errors
    ///
    /// [`P2pError::PayloadTooLarge`], or [`P2pError::Send`] if the peer is unreachable, does not
    /// answer in time or rejects the message.
    pub async fn send(&self, peer: PeerId, payload: Vec<u8>) -> Result<Vec<u8>, P2pError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(P2pError::PayloadTooLarge);
        }
        self.call(|done| Command::Send {
            peer,
            payload,
            done,
        })
        .await?
    }

    /// Replies to an inbound message. Unanswered messages time out on the sender side.
    ///
    /// # Errors
    ///
    /// [`P2pError::Stopped`] if the node is not running.
    pub async fn reply(&self, token: ReplyToken, payload: Vec<u8>) -> Result<(), P2pError> {
        self.cmd
            .send(Command::Reply { token, payload })
            .await
            .map_err(|_| P2pError::Stopped)
    }

    /// Stops the node.
    pub async fn shutdown(&self) {
        let _ = self.cmd.send(Command::Shutdown).await;
    }
}

/// Peer id of a device public key (Ed25519, 32 bytes).
///
/// # Errors
///
/// [`P2pError::InvalidKey`] if the bytes are not a valid Ed25519 public key.
pub fn peer_id_of(device_pub: &[u8; 32]) -> Result<PeerId, P2pError> {
    let pk = identity::ed25519::PublicKey::try_from_bytes(device_pub)
        .map_err(|_| P2pError::InvalidKey)?;
    Ok(identity::PublicKey::from(pk).to_peer_id())
}

/// Extracts the device public key (Ed25519) from a peer id.
///
/// Returns `None` for peer ids that do not embed an Ed25519 key (not an OutpostWave device).
#[must_use]
pub fn device_key_of(peer: &PeerId) -> Option<[u8; 32]> {
    let mh: &libp2p::multihash::Multihash<64> = peer.as_ref();
    // Identity multihash (code 0x00) embeds the protobuf-encoded public key.
    if mh.code() != 0 {
        return None;
    }
    identity::PublicKey::try_decode_protobuf(mh.digest())
        .ok()?
        .try_into_ed25519()
        .ok()
        .map(|k| k.to_bytes())
}

/// Starts a node in a new Tokio task.
///
/// Must be called from within a Tokio runtime.
///
/// # Errors
///
/// [`P2pError::InvalidKey`] or [`P2pError::Transport`].
pub fn start(config: P2pConfig) -> Result<(P2pHandle, mpsc::Receiver<P2pEvent>), P2pError> {
    let mut secret = config.device_secret;
    let key = identity::Keypair::ed25519_from_bytes(&mut secret).map_err(|_| P2pError::InvalidKey);
    secret.zeroize();
    let key = key?;
    let peer_id = key.public().to_peer_id();
    let mdns = config.mdns;
    let relay_server = config.relay_server;
    // A relay must announce addresses in reservations: without explicit external addresses,
    // announce its listen addresses (fine on a LAN or a server with a public IP).
    let auto_external = relay_server && config.external_addrs.is_empty();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|e| P2pError::Transport(e.to_string()))?
        .with_quic()
        .with_dns()
        .map_err(|e| P2pError::Transport(e.to_string()))?
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .map_err(|e| P2pError::Transport(e.to_string()))?
        .with_behaviour(|key, relay| {
            Behaviour::new(key, relay, mdns, relay_server)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        })
        .map_err(|e| P2pError::Transport(e.to_string()))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    for addr in &config.listen {
        if let Err(e) = swarm.listen_on(addr.clone()) {
            tracing::warn!(%addr, %e, "cannot listen");
        }
    }
    for addr in &config.external_addrs {
        swarm.add_external_address(addr.clone());
    }
    for addr in &config.bootstrap {
        if let Some(peer) = peer_of_addr(addr) {
            swarm.behaviour_mut().kad.add_address(&peer, addr.clone());
        }
    }
    if !config.bootstrap.is_empty() {
        let _ = swarm.behaviour_mut().kad.bootstrap();
    }
    for addr in &config.relays {
        // Reserve a slot on the relay so peers can reach us through it.
        if let Some(peer) = peer_of_addr(addr) {
            swarm.add_peer_address(peer, addr.clone());
        }
        if let Err(e) = swarm.listen_on(addr.clone().with(Protocol::P2pCircuit)) {
            tracing::warn!(%addr, %e, "cannot listen through relay");
        }
    }

    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (ev_tx, ev_rx) = mpsc::channel(256);
    let mut node = Node::new(swarm, cmd_rx, ev_tx);
    node.auto_external = auto_external;
    tokio::spawn(node.run());
    Ok((
        P2pHandle {
            peer_id,
            cmd: cmd_tx,
        },
        ev_rx,
    ))
}

/// Address to reach `peer` through `relay` (a relay address including `/p2p/<relay id>`).
#[must_use]
pub fn circuit_addr(relay: &Multiaddr, peer: PeerId) -> Multiaddr {
    relay
        .clone()
        .with(Protocol::P2pCircuit)
        .with(Protocol::P2p(peer))
}

fn peer_of_addr(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| {
        if let Protocol::P2p(id) = p {
            Some(id)
        } else {
            None
        }
    })
}

struct Node {
    swarm: Swarm<Behaviour>,
    cmd: mpsc::Receiver<Command>,
    events: mpsc::Sender<P2pEvent>,
    pending_dials: HashMap<PeerId, Vec<oneshot::Sender<Result<(), P2pError>>>>,
    pending_sends: HashMap<OutboundRequestId, oneshot::Sender<Result<Vec<u8>, P2pError>>>,
    inbound: HashMap<ReplyToken, ResponseChannel<Vec<u8>>>,
    next_token: u64,
    auto_external: bool,
}

impl Node {
    fn new(
        swarm: Swarm<Behaviour>,
        cmd: mpsc::Receiver<Command>,
        events: mpsc::Sender<P2pEvent>,
    ) -> Self {
        Self {
            swarm,
            cmd,
            events,
            pending_dials: HashMap::new(),
            pending_sends: HashMap::new(),
            inbound: HashMap::new(),
            next_token: 0,
            auto_external: false,
        }
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                cmd = self.cmd.recv() => match cmd {
                    Some(Command::Shutdown) | None => break,
                    Some(cmd) => self.on_command(cmd),
                },
                event = self.swarm.select_next_some() => self.on_swarm_event(event).await,
            }
        }
    }

    fn on_command(&mut self, cmd: Command) {
        match cmd {
            Command::Dial { peer, addrs, done } => {
                if self.swarm.is_connected(&peer) {
                    let _ = done.send(Ok(()));
                    return;
                }
                let opts = DialOpts::peer_id(peer).addresses(addrs).build();
                match self.swarm.dial(opts) {
                    Ok(()) => self.pending_dials.entry(peer).or_default().push(done),
                    Err(e) => {
                        let _ = done.send(Err(P2pError::Dial(e.to_string())));
                    }
                }
            }
            Command::Send {
                peer,
                payload,
                done,
            } => {
                let id = self.swarm.behaviour_mut().dm.send_request(&peer, payload);
                self.pending_sends.insert(id, done);
            }
            Command::Reply { token, payload } => {
                if let Some(channel) = self.inbound.remove(&token) {
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .dm
                        .send_response(channel, payload);
                }
            }
            Command::ListenAddrs { done } => {
                let _ = done.send(self.swarm.listeners().cloned().collect());
            }
            Command::Shutdown => {}
        }
    }

    // Event sends ignore errors: if the app stopped listening, events are dropped.
    async fn on_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                if self.auto_external {
                    self.swarm.add_external_address(address.clone());
                }
                let _ = self.events.send(P2pEvent::Listening(address)).await;
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                endpoint,
                num_established,
                ..
            } => {
                for done in self.pending_dials.remove(&peer_id).unwrap_or_default() {
                    let _ = done.send(Ok(()));
                }
                if num_established.get() == 1 {
                    let relayed = match &endpoint {
                        ConnectedPoint::Dialer { address, .. } => address,
                        ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
                    }
                    .iter()
                    .any(|p| matches!(p, Protocol::P2pCircuit));
                    let _ = self
                        .events
                        .send(P2pEvent::PeerConnected {
                            peer: peer_id,
                            relayed,
                        })
                        .await;
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
                let _ = self.events.send(P2pEvent::PeerDisconnected(peer_id)).await;
            }
            SwarmEvent::OutgoingConnectionError {
                peer_id: Some(peer),
                error,
                ..
            } => {
                for done in self.pending_dials.remove(&peer).unwrap_or_default() {
                    let _ = done.send(Err(P2pError::Dial(error.to_string())));
                }
            }
            SwarmEvent::Behaviour(event) => self.on_behaviour_event(event).await,
            _ => {}
        }
    }

    async fn on_behaviour_event(&mut self, event: BehaviourEvent) {
        match event {
            BehaviourEvent::Dm(request_response::Event::Message { peer, message, .. }) => {
                match message {
                    request_response::Message::Request {
                        request, channel, ..
                    } => {
                        let token = ReplyToken(self.next_token);
                        self.next_token = self.next_token.wrapping_add(1);
                        self.inbound.insert(token, channel);
                        let _ = self
                            .events
                            .send(P2pEvent::Message {
                                from: peer,
                                payload: request,
                                token,
                            })
                            .await;
                    }
                    request_response::Message::Response {
                        request_id,
                        response,
                    } => {
                        if let Some(done) = self.pending_sends.remove(&request_id) {
                            let _ = done.send(Ok(response));
                        }
                    }
                }
            }
            BehaviourEvent::Dm(request_response::Event::OutboundFailure {
                request_id,
                error,
                ..
            }) => {
                if let Some(done) = self.pending_sends.remove(&request_id) {
                    let _ = done.send(Err(P2pError::Send(error.to_string())));
                }
            }
            BehaviourEvent::Dm(request_response::Event::InboundFailure { .. }) => {
                // The response channel was dropped or timed out: forget stale tokens.
                self.inbound.retain(|_, ch| ch.is_open());
            }
            BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
                // Feed the DHT with the addresses the peer says it listens on.
                for addr in info.listen_addrs {
                    self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                }
            }
            BehaviourEvent::Mdns(mdns::Event::Discovered(list)) => {
                let mut by_peer: HashMap<PeerId, Vec<Multiaddr>> = HashMap::new();
                for (peer, addr) in list {
                    self.swarm.add_peer_address(peer, addr.clone());
                    by_peer.entry(peer).or_default().push(addr);
                }
                for (peer, addrs) in by_peer {
                    let _ = self.events.send(P2pEvent::Discovered { peer, addrs }).await;
                }
            }
            _ => {}
        }
    }
}
