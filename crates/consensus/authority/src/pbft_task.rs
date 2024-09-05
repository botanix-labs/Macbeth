use std::{sync::Arc, time::Duration};
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_evm::execute::BlockExecutorProvider;
use reth_network_p2p::HeadersClient;
use tracing::{info, error, warn};
use crate::{
    pbft::PbftStateMachine, utils::is_active_sync_in_progress, AuthorityConsensus, Storage,
};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_network::{
    frost::{
        manager::{FrostCommand, FrostConfig, ToFrostManager},
        PbftEventResponseType, PbftResponse, PeerMessageResponse,
    },
    NetworkHandle,
};
use reth_primitives::{header_ext::BlockWitness, SealedBlock};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::PeerId;
use reth_tasks::TaskExecutor;
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use reth_network_peers::pk2id;
/// Enum defining possible frost message notifications
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PbftNotificationMessage {
    /// Block builder task propose a block to get gossip'd to peers
    ProposeBlock(PbftNotification),
    /// A notification to the block builder task that we have received a with a quorum of
    /// commitments
    CommitmentsReceived(PbftFinalizationNotification),
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

pub struct PbftTask<EF, BF, DB, ToFrostMan: ToFrostManager, NetworkClient> {
    /// Frost Handler
    pub(crate) frost_handle: ToFrostMan,
    /// pbft state machine
    pub(crate) pbft_state_machine: PbftStateMachine<EF, BF, DB, ToFrostMan, NetworkClient>,
    /// Channel to receive pbft notifications (from the block production task)
    pbft_task_rx: UnboundedReceiver<PbftNotificationMessage>,
    /// Channel to send pbft notifications (to the block production task)
    pbft_task_tx: UnboundedSender<PbftNotificationMessage>,
    /// authority / network secret key
    #[allow(dead_code)]
    secret_key: secp256k1::SecretKey,
    /// config
    #[allow(dead_code)]
    config: FrostConfig,
    /// network client
    #[allow(dead_code)]
    network_client: NetworkClient,
    /// network handle
    network_handle: NetworkHandle,
}

impl<EF, BF, DB, ToFrostMan, NetworkClient> PbftTask<EF, BF, DB, ToFrostMan, NetworkClient>
where
    ToFrostMan: ToFrostManager + Clone + 'static,
    NetworkClient: HeadersClient + Clone + 'static,
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: BlockExecutorProvider + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        storage: Storage<EF, BF, DB>,
        frost_handle: ToFrostMan,
        config: FrostConfig,
        secret_key: secp256k1::SecretKey,
        pbft_task_rx: UnboundedReceiver<PbftNotificationMessage>,
        pbft_task_tx: UnboundedSender<PbftNotificationMessage>,
        task_executor: TaskExecutor,
        network_client: NetworkClient,
        network_handle: NetworkHandle,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        consensus: AuthorityConsensus,
        _executor_factory: EF,
    ) -> Self {
        let my_peerid = pk2id(&config.authority_pk);
        let pbft_state_machine = PbftStateMachine::new(
            storage,
            frost_handle.clone(),
            config.clone(),
            my_peerid,
            secret_key,
            Some(task_executor),
            network_client.clone(),
            bitcoin_block_header,
            consensus,
        );

        Self {
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

    /// handle any pbft notifications from the block builder task
    async fn handle_notification(&mut self, message: PbftNotificationMessage) {
        match message {
            PbftNotificationMessage::ProposeBlock(pbft_notification) => {
                info!(target: "PBFT Task", "Received block proposal notification");
                // we are the in turn block producer proposing a block
                match self.pbft_state_machine.init_block_proposal(pbft_notification.block).await {
                    Ok(None) => {
                        info!(target: "PBFT Task", "Block proposal Init processed successfully");
                    }
                    Ok(Some(block_witness)) => {
                        info!(target: "PBFT Task", "Block proposal Init processed successfully -- already ready to produce a block");
                        self.pbft_task_tx
                            .send(PbftNotificationMessage::CommitmentsReceived(
                                PbftFinalizationNotification { block_witness },
                            ))
                            // TODO remove unwrap()
                            .unwrap();
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

    /// receive over a channel message from other peers and update our state machine
    async fn handle_peer_msg(&mut self, (peer_id, msg): (PeerId, PeerMessageResponse)) {
        match msg {
            PeerMessageResponse::Pbft(pbft_response) => {
                let PbftResponse { response_type, data } = pbft_response;
                match response_type {
                    PbftEventResponseType::CoordinatorBlockProposal => {
                        match self.pbft_state_machine.process_block_proposal(data, peer_id).await {
                            Ok(()) => {
                                info!(target: "PBFT Task", "Block proposal processed successfully");
                            }
                            Err(e) => {
                                error!(target: "PBFT Task", "Error processing block proposal {:?}", e);
                            }
                        }
                    }
                    PbftEventResponseType::PeerPreCommitment => {
                        match self.pbft_state_machine.process_precommitment(data, peer_id).await {
                            Ok(()) => {
                                info!(target: "PBFT Task", "Peer pre-commitment processed successfully");
                            }
                            Err(e) => {
                                error!(target: "PBFT Task", "Error processing peer pre-commitment {:?}", e);
                            }
                        }
                    }
                    PbftEventResponseType::PeerCommitment => {
                        match self.pbft_state_machine.process_commitment(data, peer_id).await {
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
            }
            PeerMessageResponse::Signing(_) => {
                // Nothing to do for dkg related messages. Does are handled by the frost
                // task
            }
            PeerMessageResponse::Healthcheck(_) => {
                // Nothing to do for health related messages.
            }
            PeerMessageResponse::Utxo(_) => {
                // Nothing to do for UTXO sync related messages.
            }
        }
    }

    pub async fn start_task(&mut self) {
        info!(target: "PBFT Task", "Starting PBFT Task");
        // before we start get a proper event receiver
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        if let Err(e) =
            self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))
        {
            error!(target: "PBFT Task", "Failed to send GetPeerMessagesStream frost command {:?}", e);
        }
        let mut peer_messages_rx = match peer_messages_rx.await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                tracing::error!(target: "PBFT Task", "Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        self.pbft_state_machine.spawn_cleanup_task().await;

        loop {
            // ensure the node is not syncing
            if is_active_sync_in_progress(&self.network_handle) {
                tracing::warn!(target: "PBFT Task", "Node is still syncing, pbft task is awaiting fully synced status ...");
                tokio::time::sleep(Duration::from_millis(500)).await;
                break;
            }

            tokio::select! {
                Some(msg) = self.pbft_task_rx.recv() => self.handle_notification(msg).await,
                Some(msg) = peer_messages_rx.recv() => self.handle_peer_msg(msg).await,
            };

            if self.pbft_task_rx.is_closed() && peer_messages_rx.is_closed() {
                tracing::info!(target: "consensus::authority", "pbft task shutting down");
                break;
            }
        }
    }
}

impl<EF, BF, DB, ToFrostMan, NetworkClient> std::fmt::Debug
    for PbftTask<EF, BF, DB, ToFrostMan, NetworkClient>
where
    ToFrostMan: ToFrostManager + Clone,
    DB: Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PbftTask").finish_non_exhaustive()
    }
}
