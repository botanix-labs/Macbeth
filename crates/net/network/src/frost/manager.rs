use super::{
    FrostPeerCommand, FrostProtocolEvent, HealthcheckResponse, PeerMessageResponse,
    WalletStateResponse,
};
use crate::{session::Direction, NetworkHandle};
use frost_secp256k1_tr as frost;
use futures::{Future, StreamExt};
use rand::Rng;
use reth_network_api::Peers;
use reth_network_peers::PeerId;
use std::{
    collections::HashMap,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{
    mpsc::{self, error::SendError, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info, warn};

/// Trait for sending commands to the [`FrostManager`]
/// Trait was created mainly for the convenience of mocking during testing
pub trait ToFrostManager {
    /// Sends a command to the Protocol
    fn send_command(&self, cmd: FrostCommand) -> Result<(), SendError<FrostCommand>>;
}

/// Frost Handle for communication with the protocol
#[derive(Clone, Debug)]
pub struct FrostHandle {
    manager_tx: mpsc::UnboundedSender<FrostCommand>,
}

/// Implementations for the [`FrostHandle`]
impl ToFrostManager for FrostHandle {
    /// Sends a command to the Protocol
    fn send_command(&self, cmd: FrostCommand) -> Result<(), SendError<FrostCommand>> {
        self.manager_tx.send(cmd)
    }
}

/// Structure that stores all information about a connected peer
#[derive(Debug, Clone)]
pub struct PeerData {
    /// peer id
    pub peer_id: PeerId,
    /// channel use for sending commands to the peer
    pub peer_commands_tx: UnboundedSender<FrostPeerCommand>,
    /// in or outgoing connection
    pub direction: Direction,
    /// the frost identifier of the peer
    pub frost_identifier: frost::Identifier,
}

/// Context for forwarding peer messages to the frost task
#[derive(Debug, Clone)]
pub struct PeerMessageContext {
    /// the peer id
    pub peer_id: PeerId,
    /// frost identifier
    pub frost_identifier: frost::Identifier,
    /// The message itself
    pub message: PeerMessageResponse,
}

/// Frost Manager implementation
#[derive(Debug)]
pub struct FrostManager {
    /// Network access.
    network: NetworkHandle,
    /// Subscriptions to all network related events.
    ///
    /// From which we get all new incoming transaction related messages.
    from_network: UnboundedReceiverStream<FrostProtocolEvent>,
    /// Copy of the sender half, so new [`FrostManager`] can be created on demand.
    command_tx: mpsc::UnboundedSender<FrostCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<FrostCommand>,
    /// All the connected peers.
    peers_connections: HashMap<PeerId, Vec<PeerData>>,
    /// total authorities to connect to, including ourselves
    authority_peerid: Vec<PeerId>,
    /// Forwards for message to the frost task
    task_forwarder_txs: Vec<mpsc::UnboundedSender<PeerMessageContext>>,
    /// Frost configuration
    config: FrostConfig,
}

impl FrostManager {
    /// Create a new [`FrostManager`] instance with the given config
    pub fn new(
        config: FrostConfig,
        network: NetworkHandle,
        from_network: mpsc::UnboundedReceiver<FrostProtocolEvent>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let authority_peerid = config
            .authorities
            .iter()
            .map(|pk| PeerId::from_slice(&pk.serialize_uncompressed()[1..]))
            .collect();

        Self {
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            network,
            from_network: UnboundedReceiverStream::new(from_network),
            peers_connections: HashMap::default(),
            authority_peerid,
            task_forwarder_txs: Vec::new(),
            config,
        }
    }

    fn all_authority_peers_connected(&mut self) -> bool {
        // lets first prune all closed connections
        self.prune_closed_connections();

        // Filter out all peers that are not confirmed and have a closed channels
        info!(target: "network::frost::all_authority_peers_connected", "Peers connections len: {:?}", self.peers_connections.values());
        for peer_data in self.peers_connections.values() {
            for data in peer_data {
                info!(target: "network::frost::all_authority_peers_connected", "channel closed: {:?}", data.peer_commands_tx.is_closed());
            }
        }

        self.peers_connections.len() == self.authority_peerid.len() - 1
    }

    /// Returns a new [`FrostHandle`] that can send commands to this type.
    pub fn handle(&self) -> FrostHandle {
        FrostHandle { manager_tx: self.command_tx.clone() }
    }

    fn is_authority_peer(&self, peer_id: &PeerId) -> bool {
        self.authority_peerid.contains(peer_id)
    }

    fn prune_closed_connections(&mut self) {
        let mut pruned_peers_connections = self.peers_connections.clone();
        for (peer_id, peer_data) in &mut self.peers_connections {
            // Prune all connections that have a closed channels
            peer_data.retain(|data| !data.peer_commands_tx.is_closed());

            if peer_data.is_empty() {
                warn!(target: "network::frost::prune_closed_connections", "Peer {:?} has no open channels, removing from peer connections", peer_id);
                pruned_peers_connections.remove(peer_id);
                // perhaps here we can try to reconnect to the peer?
            }

            if peer_data.len() > 1 {
                // This should not happen, worth logging
                warn!(target: "network::frost::prune_closed_connections", "Peer {:?} has multiple open channels, this should not happen", peer_id);
            }
        }

        self.peers_connections = pruned_peers_connections;
    }

    fn send_healthcheck_to_peers(&mut self) {
        // lets first prune all closed connections
        // For each peer there should only be one valid connection
        self.prune_closed_connections();
        for (peer_id, peer_data) in &self.peers_connections {
            if let Some(peer_data) = peer_data.first() {
                let resp =
                    HealthcheckResponse { sender: *self.network.peer_id(), receiver: *peer_id };
                match peer_data
                    .peer_commands_tx
                    .send(FrostPeerCommand::PeerMessage(PeerMessageResponse::Healthcheck(resp)))
                {
                    Ok(_) => {
                        debug!("Healthcheck sent to peer {:?}", peer_id,);
                    }
                    Err(e) => {
                        error!("Failed to send healthcheck to peer {:?}, error: {:?}", peer_id, e);
                    }
                }
            } else {
                // This should not happen, worth logging
                warn!(target: "network::frost::send_healthcheck_to_peers", "Peer {:?} has no open channels, skipping healthcheck", peer_id);
            }
        }
    }

    fn on_network_event(&mut self, protocol_event: FrostProtocolEvent) {
        match protocol_event {
            FrostProtocolEvent::ConnectionEstablished { direction, peer_id, peer_commands_tx } => {
                info!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::ConnectionEstablished event from peer with id = {:?}, direction = {:?}, connection channel = {:?}", peer_id, direction, peer_commands_tx);
                if !self.is_authority_peer(&peer_id) {
                    info!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::ConnectionEstablished event from non-authority peer {:?}, protocol_event", peer_id);
                    return;
                }

                // make sure we ignore our own connection
                if *self.network.peer_id() == peer_id {
                    info!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::ConnectionEstablished event from our own peer {:?}", peer_id);
                    return;
                }

                if peer_commands_tx.is_closed() {
                    warn!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::ConnectionEstablished event from peer with id = {:?}, but the connection channel is already closed", peer_id);
                    return;
                }

                let (index, _) = self
                    .config
                    .authorities
                    .iter()
                    .enumerate()
                    .find(|(_, pk)| {
                        peer_id == PeerId::from_slice(&pk.serialize_uncompressed()[1..])
                    })
                    .unzip();

                let mut peers_connections = self.peers_connections.clone();
                let peer_data = peers_connections.entry(peer_id).or_default();
                peer_data.push(PeerData {
                    peer_id,
                    direction,
                    peer_commands_tx,
                    frost_identifier: authority_index_to_frost_identifier(
                        index.expect("index must exist, checked by is_authority_peer") as u16,
                    ),
                });

                self.peers_connections.insert(peer_id, peer_data.clone());
                self.prune_closed_connections();
            }
            FrostProtocolEvent::PeerMessage { peer_id, response } => {
                info!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::PeerMessage message from peer with id = {:?}, response = {:?}", peer_id, response);
                if !self.is_authority_peer(&peer_id) {
                    warn!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::PeerMessage message from non-authority peer {:?}", peer_id);
                    return;
                }

                // Check if the peer is in the peer connections. i.e. we have a valid connection to
                // the peer. it may be find to address this message on the
                // application level, but then we cannot respond
                if !self.peers_connections.contains_key(&peer_id) {
                    warn!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::PeerMessage message from peer with id = {:?}, but the peer is not in the peer connections", peer_id);
                    return;
                }

                let peer_data =
                    self.peers_connections.get(&peer_id).unwrap().first().expect("checked above");

                for task_forwarder in &self.task_forwarder_txs {
                    if let Err(send_res) = task_forwarder.send(PeerMessageContext {
                        peer_id,
                        frost_identifier: peer_data.frost_identifier,
                        message: response.clone(),
                    }) {
                        error!(target: "network::frost::on_network_event", "Received FrostProtocolEvent::PeerMessage event from peer with id {}, but could not forward it to task. Error: {:?}", peer_id, send_res);
                    }
                }
            }
        }
    }

    /// Handles a command received from a detached [`FrostHandle`]
    fn on_command(&mut self, cmd: FrostCommand) {
        // lets first prune all closed connections
        self.prune_closed_connections();

        match cmd {
            FrostCommand::CheckConnectedToAll(tx) => {
                // reply to caller
                if let Err(e) = tx.send(self.all_authority_peers_connected()) {
                    error!(target: "network::frost::on_command", "Error replying to call on CheckConnectedToAll {:?}", e);
                }
            }
            FrostCommand::GetAllConnectedPeers(tx) => {
                // Filter out all peers that are not confirmed and have a closed channels
                let peer_connections = self
                    .peers_connections
                    .iter()
                    .map(|(peer_id, peer_data)| {
                        (*peer_id, peer_data.first().expect("will always have one element").clone())
                    })
                    .collect::<HashMap<_, _>>();
                // reply to caller
                if let Err(e) = tx.send(peer_connections) {
                    error!(target: "network::frost::on_command", "Error replying to call on GetAllConnectedPeers {:?}", e);
                }
            }
            FrostCommand::GetPeerMessagesStream(tx) => {
                // create channel whereby keeping the sender half and sending to the caller the
                // receiver
                let (task_forwarder_txs, frost_task_forwarder_rx) =
                    mpsc::unbounded_channel::<PeerMessageContext>();
                self.task_forwarder_txs.push(task_forwarder_txs);
                // reply to caller
                if let Err(e) = tx.send(frost_task_forwarder_rx) {
                    error!(target: "network::frost::on_command", "Error replying to call on GetPeerMessagesStream {:?}", e);
                }
            }
        }
    }
}

/// An endless future. Preemption ensure that future is non-blocking, nonetheless. See
/// [`crate::NetworkManager`] for more context on the design pattern.
///
/// This should be spawned or used as part of `tokio::select!`.
impl Future for FrostManager {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match this.from_network.poll_next_unpin(cx) {
                Poll::Pending => break,
                Poll::Ready(None) => {
                    // This is only possible if the channel was deliberately closed since we always
                    // have an instance of `NetworkHandle`
                    error!(target: "network::frost::poll", "Network message channel closed.");
                    panic!("Network message channel closed.");
                }
                Poll::Ready(Some(event)) => {
                    this.on_network_event(event);
                }
            };
        }

        loop {
            match this.command_rx.poll_next_unpin(cx) {
                Poll::Pending => break,
                Poll::Ready(None) => {
                    // This is only possible if the channel was deliberately closed since we always
                    // have an instance of `NetworkHandle`
                    error!(target: "network::frost::poll", "Network message channel closed.");
                    panic!("Network command rx message channel closed.");
                }
                Poll::Ready(Some(cmd)) => this.on_command(cmd),
            };
        }
        Poll::Pending
    }
}

/// Commands the [`FrostManager`] listens for.
#[derive(Debug)]
pub enum FrostCommand {
    /// Check if connection to all federated peers is established
    CheckConnectedToAll(oneshot::Sender<bool>),
    /// Get the readily connected peers
    GetAllConnectedPeers(oneshot::Sender<HashMap<PeerId, PeerData>>),
    /// Get a receiver for streaming peer messages
    GetPeerMessagesStream(oneshot::Sender<mpsc::UnboundedReceiver<PeerMessageContext>>),
}

/// Config type for initiating a [`FrostManager`] instance.
#[derive(Clone, Debug)]
pub struct FrostConfig {
    /// Authority public key of the current peer participating in frost
    pub authority_pk: secp256k1::PublicKey,
    /// Authority index of the current peer participating in frost
    pub authority_index: usize,
    /// Total number of authorities participating in frost
    pub authorities: Vec<secp256k1::PublicKey>,
    /// Minimum number of signers required to participate in frost
    pub min_signers: u16,
    /// Maximum number of signers required to participate in frost
    pub max_signers: u16,
}

impl FrostConfig {
    /// Create a new [`FrostConfig`] with default values
    pub const fn new(
        authority_pk: secp256k1::PublicKey,
        authority_index: usize,
        authorities: Vec<secp256k1::PublicKey>,
        min_signers: u16,
        max_signers: u16,
    ) -> Self {
        Self { authority_pk, authority_index, authorities, min_signers, max_signers }
    }

    /// Sets the authority public key
    pub fn set_authority_pk(&mut self, authority_pk: secp256k1::PublicKey) {
        self.authority_pk = authority_pk;
    }

    /// Sets the authority index
    pub fn set_authority_index(&mut self, authority_index: usize) {
        self.authority_index = authority_index;
    }

    /// Sets total authorities
    pub fn set_authorities(&mut self, authorities: Vec<secp256k1::PublicKey>) {
        self.authorities = authorities;
    }

    /// Sets minimum signers
    pub fn set_min_signers(&mut self, min_signers: u16) {
        self.min_signers = min_signers;
    }

    /// Sets maximum signers
    pub fn set_max_signers(&mut self, max_signers: u16) {
        self.max_signers = max_signers;
    }
}

// TODO(armins): import btcserverlib::frost_id and use on the callers
/// Maps an authority index to a frost specific identifier
/// Indices start at 0, so we add 1 to the index to get the correct identifier
/// As 0 is not a valid identifier
pub fn authority_index_to_frost_identifier(authority_index: u16) -> frost::Identifier {
    frost::Identifier::derive(authority_index.to_le_bytes().as_slice())
        .expect("can derive identifier")
}
