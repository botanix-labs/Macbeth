use super::{FrostPeerCommand, HealthcheckResponse, NetworkFrostEvent, PeerMessageResponse};
use crate::{session::Direction, NetworkHandle};
use frost_secp256k1_tr as frost;
use futures::{Future, StreamExt};
use reth_network_api::Peers;
use reth_rpc_types::PeerId;
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

/// Implementations for the [`FrostHandle`]`
impl ToFrostManager for FrostHandle {
    /// Sends a command to the Protocol
    fn send_command(&self, cmd: FrostCommand) -> Result<(), SendError<FrostCommand>> {
        self.manager_tx.send(cmd)
    }
}

/// Structure that stores all informattion about a connected peer
#[derive(Debug, Clone)]
pub struct PeerData {
    /// peer id
    pub peer_id: PeerId,
    /// chanel use for sending commands to the peer
    pub peer_commands_tx: Option<UnboundedSender<FrostPeerCommand>>,
    /// socket where the peer is connected
    pub socket_addr: Option<SocketAddr>,
    /// in or outgoing connection
    pub direction: Option<Direction>,
    /// the frost identifier of the peer
    pub frost_identifier: Option<frost::Identifier>,
}

/// Frost Manager implementation
#[derive(Debug)]
pub struct FrostManager {
    /// Network access.
    network: NetworkHandle,
    /// Subscriptions to all network related events.
    ///
    /// From which we get all new incoming transaction related messages.
    from_network: UnboundedReceiverStream<NetworkFrostEvent>,
    /// Copy of the sender half, so new [`FrostManager`] can be created on demand.
    command_tx: mpsc::UnboundedSender<FrostCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<FrostCommand>,
    /// All the connected peers.
    peers_connections: HashMap<PeerId, PeerData>,
    /// total authorities to connect to
    authority_peerid: Vec<PeerId>,
    /// Forwards for message to the frost task
    task_forwarder_txs: Vec<mpsc::UnboundedSender<(PeerId, PeerMessageResponse)>>,
}

impl FrostManager {
    /// Create a new [`FrostManager`] instance with the given config
    pub fn new(
        config: FrostConfig,
        network: NetworkHandle,
        from_network: mpsc::UnboundedReceiver<NetworkFrostEvent>,
    ) -> Self {
        let FrostConfig {
            authority_index: _,
            authorities,
            min_signers: _,
            max_signers: _,
            authority_pk: _,
        } = config;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let authority_peerid = authorities
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
        }
    }

    fn all_authority_peers_connected(&self) -> bool {
        let peers_count = self.peers_connections.keys().cloned().count();
        let mut all_peer_connection_peer_ids =
            self.peers_connections.keys().cloned().collect::<Vec<_>>();

        // retain all peer ids that are not in the autorities list
        all_peer_connection_peer_ids.retain(|peer_id| !self.authority_peerid.contains(peer_id));

        // check that the connected peers are indeed the authority peers
        peers_count == self.authority_peerid.len() - 1 && all_peer_connection_peer_ids.is_empty()
    }

    /// Returns a new [`FrostHandle`] that can send commands to this type.
    pub fn handle(&self) -> FrostHandle {
        FrostHandle { manager_tx: self.command_tx.clone() }
    }

    fn is_authority_peer(&self, peer_id: &PeerId) -> bool {
        self.authority_peerid.contains(peer_id)
    }

    fn send_healthcheck_to_peers(&self) {
        for (peer_id, peer_data) in self.peers_connections.iter() {
            let resp = HealthcheckResponse { sender: *self.network.peer_id(), receiver: *peer_id };
            if let Some(peer_commands_tx) = peer_data.peer_commands_tx.as_ref() {
                match peer_commands_tx
                    .send(FrostPeerCommand::PeerMessage(PeerMessageResponse::Healthcheck(resp)))
                {
                    Ok(_) => {
                        debug!("Healthcheck sent to peer {:?}", peer_id,);
                    }
                    Err(e) => {
                        error!("Failed to send healthcheck to peer {:?}, error: {:?}", peer_id, e);
                    }
                }
            }
        }
    }

    fn on_network_event(&mut self, protocol_event: NetworkFrostEvent) {
        match protocol_event {
            NetworkFrostEvent::ConnectionEstablished { direction, peer_id, to_connection } => {
                if !self.is_authority_peer(&peer_id) {
                    warn!(target: "network::frost::on_network_event", "Received message from non-authority peer {:?}, protocol_event", peer_id);
                    return;
                }

                // make sure we ignore our own connection
                let my_peer_id = self.network.peer_id();
                if *my_peer_id != peer_id {
                    if let Some(peer_data) = self.peers_connections.get_mut(&peer_id) {
                        peer_data.direction = Some(direction);
                        peer_data.peer_commands_tx = Some(to_connection);
                    } else {
                        self.peers_connections.insert(
                            peer_id,
                            PeerData {
                                peer_id,
                                direction: Some(direction),
                                peer_commands_tx: Some(to_connection),
                                socket_addr: None,
                                frost_identifier: None,
                            },
                        );
                    }
                }
            }
            NetworkFrostEvent::PeerMessage { peer_id, response } => {
                if !self.is_authority_peer(&peer_id) {
                    warn!(target: "network::frost::on_network_event", "Received message from non-authority peer {:?}, protocol_event", peer_id);
                    return;
                }
                for task_forwarder in self.task_forwarder_txs.iter() {
                    // TODO:  handle error?
                    let _send_res = task_forwarder.send((peer_id, response.clone()));
                }
            }
            NetworkFrostEvent::PeerConfirmed(peer_id, authority_index, peer_socket_addr) => {
                if !self.is_authority_peer(&peer_id) {
                    return;
                }

                if self.peers_connections.contains_key(&peer_id) {
                    // only if we have an already connection established
                    if let Some(peer_data) = self.peers_connections.get_mut(&peer_id) {
                        // add the peer conn mapped to a frost id based on authority index
                        let frost_identifier = peer_id_to_identifier(authority_index);
                        peer_data.socket_addr = Some(peer_socket_addr);
                        peer_data.frost_identifier = Some(frost_identifier);
                    }
                }
            }
        }
    }

    /// Handles a command received from a detached [`FrostHandle`]
    fn on_command(&mut self, cmd: FrostCommand) {
        match cmd {
            FrostCommand::SendHealtcheckToPeers => {
                self.send_healthcheck_to_peers();
            }
            FrostCommand::ReconnectPeers(peers) => {
                let peers_to_reconnect = peers.len();
                let mut reconnected_peers = 0;
                for peer in peers.into_iter() {
                    if let Some(peer_data) = self.peers_connections.get(&peer) {
                        if let Some(conn_socket) = peer_data.socket_addr {
                            info!(target: "network::frost::on_command", "Readding peer {:?} with remote address {:?} ...", peer, conn_socket.to_string());
                            self.network.add_trusted_peer(peer, conn_socket);
                            reconnected_peers += 1;
                        } else {
                            warn!(target: "network::frost::on_command", "Could not find remote socket address for peer {:?}", peer);
                        }
                    }
                }
                if reconnected_peers > 0 {
                    info!(target: "network::frost::on_command", "Readded/Reconnected {}/{} peers", reconnected_peers, peers_to_reconnect);
                }
            }
            FrostCommand::CheckConnectedToAll(tx) => {
                // reply to caller
                if let Err(e) = tx.send(self.all_authority_peers_connected()) {
                    error!(target: "network::frost::on_command", "Error replying to call on CheckConnectedToAll {:?}", e);
                }
            }
            FrostCommand::GetAllConnectedPeers(tx) => {
                // reply to caller
                if let Err(e) = tx.send(self.peers_connections.clone()) {
                    error!(target: "network::frost::on_command", "Error replying to call on GetAllConnectedPeers {:?}", e);
                }
            }
            FrostCommand::GetPeerMessagesStream(tx) => {
                // create channel whereby keeping the sender half and sending to the caller the
                // receiver
                let (task_forwarder_txs, frost_task_forwarder_rx) =
                    mpsc::unbounded_channel::<(PeerId, PeerMessageResponse)>();
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
                    return Poll::Ready(());
                }
                Poll::Ready(Some(event)) => this.on_network_event(event),
            };
        }

        loop {
            match this.command_rx.poll_next_unpin(cx) {
                Poll::Pending => break,
                Poll::Ready(None) => {
                    // This is only possible if the channel was deliberately closed since we always
                    // have an instance of `NetworkHandle`
                    error!(target: "network::frost::poll", "Network message channel closed.");
                    return Poll::Ready(());
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
    /// sends healthcheck messages to all peers
    SendHealtcheckToPeers,
    /// Reconect peers in case their connection got dropped
    ReconnectPeers(Vec<PeerId>),
    /// Check if connection to all federated peers is established
    CheckConnectedToAll(oneshot::Sender<bool>),
    /// Get the readily connected peers
    GetAllConnectedPeers(oneshot::Sender<HashMap<PeerId, PeerData>>),
    /// Get a receiver for streaming peer messages
    GetPeerMessagesStream(oneshot::Sender<mpsc::UnboundedReceiver<(PeerId, PeerMessageResponse)>>),
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
    pub fn new(
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

/// Maps an authority index to a frost specific identifier
// TODO rename this to authority_index to frost id
pub fn peer_id_to_identifier(authority_index: u16) -> frost::Identifier {
    frost::Identifier::derive(authority_index.to_le_bytes().as_slice())
        .expect("can derive identifier")
}
