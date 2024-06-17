use std::{collections::HashMap, sync::Arc, time::Instant};

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
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const NONRESPONDING_PEERS_TIMEOUT_SECS: u64 = 5 * 60;

pub struct HealthcheckTask<Client, ToFrostMan> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<Client>,
    /// Task Executor
    pub(crate) task_executor: TaskExecutor,
    /// Tracker list for peers healthcheck
    pub(crate) peers_healthcheck_tracker: Arc<RwLock<HashMap<PeerId, Instant>>>,
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
        Self {
            network_handle,
            frost_handle,
            storage,
            task_executor,
            peers_healthcheck_tracker: Default::default(),
        }
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
        let connected_peers = match connected_peers_rx.await {
            Ok(connected_peers) => connected_peers,
            Err(e) => {
                error!(target: "Healthcheck Task", "Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        // update the tracker for each peer_id and mark its state as healthy at the moment of check
        let mut guard = self.peers_healthcheck_tracker.write().await;
        for (peer_id, _) in connected_peers.into_iter() {
            let _ = guard.insert(peer_id, Instant::now());
        }
        drop(guard);

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

        // spawn a background task to do periodical healthchecks
        let frost_handle = self.frost_handle.clone();
        let peers_healthcheck_tracker = Arc::clone(&self.peers_healthcheck_tracker);
        self.task_executor.spawn(async move {
            // start looping and sending healthchecks to all connected peers
            loop {
                frost_handle.send_command(FrostCommand::SendHealtcheckToPeers);

                // sleep for some time before the next check
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;

                // check for any peers whose health checks havent been recently received
                let none_responding_peers = peers_healthcheck_tracker
                .read()
                .await
                .iter()
                .filter_map(
                    |(peer_id, &last_check)| {
                        if last_check.elapsed().as_secs().gt(&NONRESPONDING_PEERS_TIMEOUT_SECS) {
                            Some(*peer_id)
                        } else {
                            None
                        }
                    }
                )
                .collect::<Vec<PeerId>>();

                // force reconnection to those peers
                let (sender, receiver) = tokio::sync::oneshot::channel::<bool>();
                frost_handle.send_command(FrostCommand::ReconnectPeers(none_responding_peers, sender));
                match receiver.await {
                    Ok(peers_reconnected) => peers_reconnected,
                    Err(e) => {
                        error!(target: "Healthcheck Task", "Error reconnecting peers = {:?}", e);
                        panic!("Error reconnecting peers = {:?}", e);
                    }
                };
            }
        });

        let authority_peers = self.storage
        .read()
        .await
        .authorities
        .iter()
        .map(|pk| PeerId::from_slice(&pk.serialize_uncompressed()[1..]))
        .collect::<Vec<PeerId>>();
        let peers_healthcheck_tracker = Arc::clone(&self.peers_healthcheck_tracker);
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
                    PeerMessageResponse::Healtcheck(healthcheck_response) => {
                        if !authority_peers.contains(&healthcheck_response.sender) || !authority_peers.contains(&healthcheck_response.receiver) {
                            warn!(target: "Healthcheck Task", "Received healthcheck response from a peer without having requested it {:?}", &healthcheck_response.sender);
                            continue;
                        }
                        let mut peers_healthcheck_tracker = peers_healthcheck_tracker.write().await;
                        if healthcheck_response.sender.eq(self.network_handle.peer_id()) {
                            peers_healthcheck_tracker.insert(healthcheck_response.receiver, Instant::now());
                        } else {
                            warn!(target: "Healthcheck Task", "Received healthcheck response from a peer without having requested it {:?}", &healthcheck_response.sender);
                        }
                        drop(peers_healthcheck_tracker);
                    }
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
