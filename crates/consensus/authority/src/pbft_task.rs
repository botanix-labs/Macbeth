use std::time::Duration;

use crate::{pbft::PbftStateMachine, utils::is_active_sync_in_progress};
use reth_interfaces::{blockchain_tree::BlockchainTreeEngine, p2p::headers::client::HeadersClient};
use reth_network::{
    frost::{
        manager::{FrostCommand, FrostConfig, ToFrostManager},
        PbftEventResponseType, PbftResponse, PeerMessageResponse,
    },
    NetworkHandle,
};
use reth_network_types::pk2id;
use reth_primitives::{header_ext::BlockWitness, SealedBlock};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_tasks::TaskExecutor;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};

/// Enum defining possible frost message notifications
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PbftNotificationMessage {
    /// Block builder task propose a block to get gossip'd to peers
    ProposeBlock(PbftNotification),
    /// A notification to the block builder task that we have received a with a quorum of
    /// commitments
    CommitmentsReceived(PbftFinalizationNotification),
    /// A notification to the block builder task we have timed out or are no longer in turn so we
    /// can reset
    Reset,
}

/// Notification for proposing a block
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PbftNotification {
    /// The signing session id
    pub(crate) block: SealedBlock,
}

/// Notification for finalizing a pbft round
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PbftFinalizationNotification {
    /// The signing session id
    pub(crate) block_witness: BlockWitness,
}

pub struct PbftTask<Client, ToFrostMan: ToFrostManager, NetworkClient> {
    /// Frost Handler
    pub(crate) frost_handle: ToFrostMan,
    /// pbft state machine
    pub(crate) pbft_state_machine: PbftStateMachine<ToFrostMan, Client, NetworkClient>,
    /// Shared storage to insert aggregate public key
    pub(crate) client: Client,
    /// Channel to receive pbft notifications (from the block production task)
    pbft_task_rx: UnboundedReceiver<PbftNotificationMessage>,
    /// Channel to send pbft notifications (to the block production task)
    pbft_task_tx: UnboundedSender<PbftNotificationMessage>,
    /// authority / network secret key
    secret_key: secp256k1::SecretKey,
    /// config
    config: FrostConfig,
    /// network client
    network_client: NetworkClient,
    /// network handle
    network_handle: NetworkHandle,
}

impl<Client, ToFrostMan, NetworkClient> PbftTask<Client, ToFrostMan, NetworkClient>
where
    ToFrostMan: ToFrostManager + Clone + 'static,
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    NetworkClient: HeadersClient + Clone + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        client: Client,
        frost_handle: ToFrostMan,
        config: FrostConfig,
        secret_key: secp256k1::SecretKey,
        pbft_task_rx: UnboundedReceiver<PbftNotificationMessage>,
        pbft_task_tx: UnboundedSender<PbftNotificationMessage>,
        task_executor: TaskExecutor,
        network_client: NetworkClient,
        network_handle: NetworkHandle,
    ) -> Self {
        let my_peerid = pk2id(&config.authority_pk);
        let mut pbft_state_machine = PbftStateMachine::new(
            client.clone(),
            frost_handle.clone(),
            config.clone(),
            my_peerid,
            secret_key,
            Some(task_executor),
            network_client.clone(),
        );
        pbft_state_machine.spawn_cleanup_task();
        Self {
            client,
            frost_handle,
            pbft_state_machine,
            secret_key,
            pbft_task_rx,
            pbft_task_tx,
            config,
            network_client,
            network_handle,
        }
    }

    pub async fn start_task(&mut self) -> () {
        info!(target: "PBFT Task", "Starting PBFT Task");
        // before we start get a proper event receiver
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx));
        let mut peer_messages_rx = match peer_messages_rx.await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                error!(target: "PBFT Task", "Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        loop {
            // ensure the node is not syncing
            if is_active_sync_in_progress(&self.network_handle) {
                warn!(target: "consensus::authority", "Node is still syncing, pbft task is awaiting fully synced status ...");
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }

            // First handle any pbft notifications from the block builder task
            while let Ok(message) = self.pbft_task_rx.try_recv() {
                match message {
                    PbftNotificationMessage::Reset => {
                        info!(target: "PBFT Task", "Resetting PBFT State Machine");
                        self.pbft_state_machine = self.pbft_state_machine.clone().reset();
                    }
                    PbftNotificationMessage::ProposeBlock(pbft_notification) => {
                        info!(target: "PBFT Task", "Received block proposal notification");
                        // we are the in turn block producer proposing a block
                        match self
                            .pbft_state_machine
                            .init_block_proposal(pbft_notification.block)
                            .await
                        {
                            Ok(()) => {
                                info!(target: "PBFT Task", "Block proposal Init processed successfully");
                            }
                            Err(e) => {
                                error!(target: "PBFT Task", "Error processing block proposal Init {:?}", e);
                            }
                        }
                    }
                    msg => {
                        warn!(
                            target: "PBFT Task",
                            "uncovered pbft notification message {:?}",
                            msg
                        );
                    }
                }
            }
            // receive over a channel message from other peers and update our state machine
            if let Ok((peer_id, msg)) = peer_messages_rx.try_recv() {
                match msg {
                    PeerMessageResponse::Pbft(pbft_response) => {
                        let PbftResponse { response_type, data } = pbft_response;
                        match response_type {
                            PbftEventResponseType::CoordinatorBlockProposal => {
                                match self
                                    .pbft_state_machine
                                    .process_block_proposal(data, peer_id)
                                    .await
                                {
                                    Ok(()) => {
                                        info!(target: "PBFT Task", "Block proposal processed successfully");
                                    }
                                    Err(e) => {
                                        error!(target: "PBFT Task", "Error processing block proposal {:?}", e);
                                    }
                                }
                            }
                            PbftEventResponseType::PeerPreCommitment => {
                                match self
                                    .pbft_state_machine
                                    .process_precommitment(data, peer_id)
                                    .await
                                {
                                    Ok(()) => {
                                        info!(target: "PBFT Task", "Peer pre-commitment processed successfully");
                                    }
                                    Err(e) => {
                                        error!(target: "PBFT Task", "Error processing peer pre-commitment {:?}", e);
                                    }
                                }
                            }
                            PbftEventResponseType::PeerCommitment => {
                                match self
                                    .pbft_state_machine
                                    .process_commitment(data, peer_id)
                                    .await
                                {
                                    Ok(None) => {
                                        info!(target: "PBFT Task", "Peer commitment processed successfully, still waiting for other commits");
                                    }
                                    Ok(Some(block_witness)) => {
                                        info!(target: "PBFT Task", "Peer commitment processed successfully, quorum reached");
                                        self.pbft_task_tx
                                            .send(PbftNotificationMessage::CommitmentsReceived(
                                                PbftFinalizationNotification { block_witness },
                                            ))
                                            // TODO remove unwrap()
                                            .unwrap();
                                    }
                                    Err(e) => {
                                        error!(target: "PBFT Task", "Error processing peer commitment {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                    PeerMessageResponse::Dkg(_) => {
                        // Nothing to do for dkg related messages. Does are handled by the frost
                        // task
                        continue;
                    }
                    PeerMessageResponse::Signing(_) => {
                        // Nothing to do for dkg related messages. Does are handled by the frost
                        // task
                        continue;
                    }
                    PeerMessageResponse::Healtcheck(_) => {
                        // Nothing to do for health related messages.
                        continue;
                    }
                }
            }

            // short sleep
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

impl<Client, F, NetworkClient> std::fmt::Debug for PbftTask<Client, F, NetworkClient>
where
    F: ToFrostManager + Clone,
    Client: Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PbftTask").finish_non_exhaustive()
    }
}
