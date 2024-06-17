use crate::Storage;
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::{
    frost::manager::{FrostCommand, FrostConfig, ToFrostManager},
    NetworkHandle,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::PeerId;
use reth_tasks::TaskExecutor;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

/// Enum defining possible frost message notifications
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrostNotificationMessage {
    /// Finalized frost signing signature
    FinalizedSignature(FrostNotification),
    /// Initiate signing session
    InitiateSigning(FrostNotification),
}

/// Finalised frost signature message
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrostNotification {
    /// The signing session id
    pub(crate) signing_session_id: Vec<u8>,
    /// The agglomerated psbts
    pub(crate) psbt: Vec<u8>,
}

pub struct HealthcheckTask<Client, ToFrostMan> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<Client>,
}

impl<Client, ToFrostMan> HealthcheckTask<Client, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
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
    ) -> Self {
        info!("Frost authority index: {}/{}", config.authority_index, config.authorities.len());

        Self { network_handle, frost_handle, storage }
    }

    async fn check_all_peers_connected(&mut self) -> bool {
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
            if self.check_all_peers_connected().await {
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

        // start looping and checking for disconnected peers
        loop {
            if !self.check_all_peers_connected().await {}
        }

        // start listening for dropped peers
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
