use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::Instant,
};

use crate::Storage;
use reth_network::{
    frost::{
        manager::{FrostCommand, ToFrostManager},
        PeerMessageResponse,
    },
    NetworkHandle,
};
use reth_network_api::Peers;
use reth_network_types::pk2id;
use reth_rpc_types::PeerId;
use reth_tasks::TaskExecutor;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const NONRESPONDING_PEERS_TIMEOUT_SECS: u64 = 45;

pub struct HealthcheckTask<EF, BF, DB, ToFrostMan> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Shared authority storage
    pub(crate) storage: Storage<EF, BF, DB>,
    /// Task Executor
    pub(crate) task_executor: TaskExecutor,
    /// Tracker list for peers healthcheck
    pub(crate) peers_healthcheck_tracker: Arc<RwLock<HashMap<PeerId, Instant>>>,
}

impl<EF, BF, DB, ToFrostMan> HealthcheckTask<EF, BF, DB, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone + Send + 'static,
    DB: Clone,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        network_handle: NetworkHandle,
        frost_handle: ToFrostMan,
        storage: Storage<EF, BF, DB>,
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

    pub async fn start_task(&mut self) {
        info!(target: "HealthcheckTask::start_task", "Starting HealthcheckTask");

        // get all authority peers in the federation (at this point we must be connected to all of
        // them)

        let storage_lock = self.storage.read().await;

        let authority_peers_sockets = storage_lock
            .authorities
            .iter()
            .enumerate()
            .filter_map(|(index, authority_pk)| {
                let authority_peer_id = pk2id(authority_pk);
                if authority_peer_id != *self.network_handle.peer_id() {
                    // excluse our own peer_id
                    let authority_socket_addr =
                        storage_lock.authority_socket_addresses.get(index).cloned();
                    Some(authority_peer_id).zip(authority_socket_addr)
                } else {
                    None
                }
            })
            .collect::<Vec<(PeerId, SocketAddr)>>();

        drop(storage_lock);

        // update the tracker for each authority peer_id and mark its state as healthy initially
        let mut guard = self.peers_healthcheck_tracker.write().await;
        for (peer_id, _) in authority_peers_sockets.iter() {
            let _ = guard.insert(*peer_id, Instant::now());
        }
        drop(guard);

        // get all authority peers rx channels
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        if let Err(e) =
            self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))
        {
            error!(target: "HealthcheckTask::start_task", "Failed to send GetPeerMessagesStream frost command {:?}", e);
        }
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
        let network_handle = self.network_handle.clone();
        self.task_executor.spawn(async move {
            // start looping and sending healthchecks to all connected authority peers
            loop {
                // send healthcheck to all connected authority peers
                if let Err(e) = frost_handle.send_command(FrostCommand::SendHealtcheckToPeers) {
                    error!(target: "HealthcheckTask::start_task", "Failed to send SendHealtcheckToPeers frost command {:?}", e);
                }

                // sleep for some time before the next check
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;

                // check for any authority peers whose health checks havent been recently received
                let mut none_responding_authority_peers = peers_healthcheck_tracker
                    .read()
                    .await
                    .iter()
                    .filter_map(|(peer_id, &last_check)| {
                        if last_check.elapsed().as_secs().gt(&NONRESPONDING_PEERS_TIMEOUT_SECS) {
                            Some(*peer_id)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<PeerId>>();

                // get from the network handler all trusted connected peers (could be authority but also none-authority ones)
                let all_trusted_connected_peers_ids = network_handle
                .get_trusted_peers()
                .await
                .ok()
                .unwrap_or_default()
                .into_iter()
                .map(|peer| {
                    peer.remote_id
                })
                .collect::<Vec<_>>();

                // check for any physically disconnected authority peers
                let disconnected_authority_peers = authority_peers_sockets
                .iter()
                .filter(|(peer_id, _)| !all_trusted_connected_peers_ids.contains(peer_id))
                .cloned()
                .collect::<Vec<_>>();

                // merge physically disconnected and frost non-responsive peers
                none_responding_authority_peers.extend(disconnected_authority_peers.iter().map(|(k, _)| k.clone()));
                let peers_to_reconnect: HashSet<PeerId> = HashSet::from_iter(none_responding_authority_peers.into_iter());

                // if no peers to reconnect to, skip
                if peers_to_reconnect.is_empty() {
                    continue;
                }

                // print the peeers we need to reconnect with
                let peers_to_reconnect_stringified = peers_to_reconnect
                .clone()
                .iter()
                .map(|peer| peer.to_string())
                .collect::<Vec<String>>()
                .join(",");
                warn!(target: "HealthcheckTask::start_task", "Trying to reconnect to peers {} ...", peers_to_reconnect_stringified);

                // now for all peers we want to reconnect with, get their socket addresses
                let peers_to_reconnect = peers_to_reconnect
                .iter()
                .filter_map(|peer_id| {
                    let peer_remote_addr = authority_peers_sockets
                    .iter()
                    .find(|(p, _)| *p == *peer_id)
                    .map(|(_, socket_addr)| *socket_addr);
                    Some(*peer_id).zip(peer_remote_addr)
                })
                .collect::<Vec<(PeerId, SocketAddr)>>();

                // try to reconnect to the peers whose frost subprotocol connection or physical connection has been lost to
                if let Err(e) = frost_handle
                    .send_command(FrostCommand::ReconnectPeers(peers_to_reconnect.into_iter().collect())) {
                        error!(target: "HealthcheckTask", "Failed to send ReconnectPeers frost command {:?}", e);
                    }
            }
        });

        let peers_healthcheck_tracker = Arc::clone(&self.peers_healthcheck_tracker);

        // receive over a channel message from other peers
        while let Some((_peerid, msg)) = peer_messages_rx.recv().await {
            match msg {
                PeerMessageResponse::Pbft(_) => {
                    // Nothing to do for pbft related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Dkg(_) => {
                    // Nothing to do for dkg related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Signing(_) => {
                    // Nothing to do for signing related messages. Does are handled by the frost
                    // task
                }
                PeerMessageResponse::Healthcheck(healthcheck_response) => {
                    let mut peers_healthcheck_tracker = peers_healthcheck_tracker.write().await;
                    peers_healthcheck_tracker.insert(healthcheck_response.sender, Instant::now());
                    drop(peers_healthcheck_tracker);
                }
                PeerMessageResponse::Utxo(_) => {
                    // Nothing to do for utxo sync messages. Does are handled by the frost
                    // task
                }
            }
        }
    }
}

impl<EF, BF, DB, ToFrostMan> std::fmt::Debug for HealthcheckTask<EF, BF, DB, ToFrostMan> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthcheckTask").finish_non_exhaustive()
    }
}
