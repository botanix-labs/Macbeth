use crate::Storage;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::{
    frost::{
        manager::{FrostCommand, FrostConfig, ToFrostManager},
        PeerMessageResponse,
    },
    NetworkHandle,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::PeerId;
use reth_tasks::TaskExecutor;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

pub struct HealthcheckTask<Client, ToFrostMan> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<Client>,
    /// Task Executor
    pub(crate) task_executor: TaskExecutor,
}

impl<Client, ToFrostMan> HealthcheckTask<Client, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone + Send + 'static,
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        network_handle: NetworkHandle,
        frost_handle: ToFrostMan,
        config: FrostConfig,
        storage: Storage<Client>,
        task_executor: TaskExecutor,
    ) -> Self {
        info!("Frost authority index: {}/{}", config.authority_index, config.authorities.len());

        Self { network_handle, frost_handle, storage, task_executor }
    }

    async fn check_all_peers_initially_connected(&mut self) -> bool {
        // check if we are connected to all frost peers when in turn
        let (sender, receiver) = tokio::sync::oneshot::channel::<bool>();
        self.frost_handle.send_command(FrostCommand::CheckConnectedToAll(sender));
        match receiver.await {
            Ok(is_connected) => {
                if !is_connected {
                    info!(target: "Healthcheck Task", "Not yet connected to all frost peers. Waiting ...");
                    return false;
                }
                info!(target: "Healthcheck Task", "Connected to all frost peer {:?}", is_connected);
                return true;
            }
            Err(e) => {
                error!(target: "Healthcheck Task", "Check for connection to other peers failed {:?}", e);
                return false;
            }
        }
    }

    pub async fn start_task(&mut self) {
        // get all authority peers
        let authority_peers: Vec<PeerId> = self
            .storage
            .read()
            .await
            .authorities
            .iter()
            .map(|pk| PeerId::from_slice(&pk.serialize_uncompressed()[1..]))
            .collect();

        info!(target: "Healthcheck Task", "Starting Healthcheck Task");
        loop {
            // await all peers to be connected
            if self.check_all_peers_initially_connected().await {
                break;
            }

            // short sleep
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        // get all connected peers
        let (connected_peers_tx, connected_peers_rx) = tokio::sync::oneshot::channel();
        self.frost_handle.send_command(FrostCommand::GetAllConnectedPeers(connected_peers_tx));
        let mut connected_peers = match connected_peers_rx.await {
            Ok(connected_peers) => connected_peers,
            Err(e) => {
                error!(target: "Healthcheck Task", "Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        // get all peers rx channels
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx));
        let mut peer_messages_rx = match peer_messages_rx.await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                error!("Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        let frost_handle = self.frost_handle.clone();
        self.task_executor.spawn(async move {
            // start looping and sending healthchecks to all connected peers
            loop {
                frost_handle.send_command(FrostCommand::SendHealtcheckToPeers);

                // sleep for some time before the next check
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });

        // now
        loop {
            // receive over a channel message from other peers
            if let Ok((_peerid, msg)) = peer_messages_rx.try_recv() {
                match msg {
                    PeerMessageResponse::Pbft(_) => {
                        // Nothing to do for pbft related messages. Does are handled by the frost
                        // task
                        continue;
                    }
                    PeerMessageResponse::Dkg(_) => {
                        // Nothing to do for dkg related messages. Does are handled by the frost
                        // task
                        continue;
                    }
                    PeerMessageResponse::Signing(_) => {
                        // Nothing to do for signing related messages. Does are handled by the frost
                        // task
                        continue;
                    }
                    PeerMessageResponse::Healtcheck => {}
                }
            }

            // short sleep
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

impl<Client, ToFrostMan> std::fmt::Debug for HealthcheckTask<Client, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
    Client: Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthcheckTask").finish_non_exhaustive()
    }
}
