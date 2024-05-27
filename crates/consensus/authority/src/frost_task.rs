use crate::{
    dkg::DKGStateMachine, epoch_manager::EpochManager, extended_client::BtcServerExtendedClient,
    signing::SigningStateMachine, Storage,
};
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_network::{
    frost::{
        manager::{FrostCommand, FrostConfig, ToFrostManager},
        DkgEventResponseType, DkgResponse, PeerMessageResponse, SigningEventResponseType,
        SigningResponse,
    },
    NetworkHandle,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
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

pub struct FrostTask<Client, ToFrostMan> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Epoch manager
    pub(crate) epoch_manager: EpochManager<Client>,
    /// dkg state machine
    pub(crate) dkg_state_machine: DKGStateMachine<Client, ToFrostMan>,
    /// signing state machine
    pub(crate) signing_state_machine: SigningStateMachine<Client, ToFrostMan>,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<Client>,
    /// Channel to receive frost notifications (from the block production task)
    /// We only wait for init signing messages
    frost_task_rx: UnboundedReceiver<FrostNotificationMessage>,
}

impl<Client, ToFrostMan> FrostTask<Client, ToFrostMan>
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
        btc_server: BtcServerExtendedClient,
        network_handle: NetworkHandle,
        frost_handle: ToFrostMan,
        epoch_manager: EpochManager<Client>,
        config: FrostConfig,
        storage: Storage<Client>,
        frost_task_rx: UnboundedReceiver<FrostNotificationMessage>,
        frost_task_tx: UnboundedSender<FrostNotificationMessage>,
        task_executor: TaskExecutor,
    ) -> Self {
        info!("Frost authority index: {}/{}", config.authority_index, config.authorities.len());

        let dkg_state_machine = DKGStateMachine::new(
            btc_server.clone(),
            storage.clone(),
            frost_handle.clone(),
            config.clone(),
        );

        let signing_state_machine = SigningStateMachine::new(
            btc_server,
            storage.clone(),
            frost_handle.clone(),
            config,
            frost_task_tx,
            task_executor,
        );

        Self {
            network_handle,
            frost_handle,
            epoch_manager,
            dkg_state_machine,
            signing_state_machine,
            storage,
            frost_task_rx,
        }
    }

    async fn start_dkg(&mut self) {
        // check if we are connected to all frost peers when in turn
        let (sender, receiver) = tokio::sync::oneshot::channel::<bool>();
        self.frost_handle.send_command(FrostCommand::CheckConnectedToAll(sender));
        match receiver.await {
            Ok(is_connected) => {
                if !is_connected {
                    info!("Not yet connected to all frost peers. Waiting ....");
                    return;
                }
                info!(">>>>>>>>>>> [FROST_TASK] Connected to all frost peers {}", is_connected);
                // start the dkg process
                info!(">>>>>>>>>>> [FROST_TASK] Starting the DKG state machine...");
                let _ = self.dkg_state_machine.start().await;
            }
            Err(e) => {
                error!("Check for connection to other peers failed {:?}", e);
            }
        }
    }

    pub async fn start_task(&mut self) {
        // before we start get a proper event receiver
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx));
        let mut peer_messages_rx = match peer_messages_rx.await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                error!("Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        // Calling get pk
        // Attempt to get the aggregate public key and store in storage
        if let Ok(public_key) = self.dkg_state_machine.get_public_key().await {
            info!(">>>>>>>>>>> [FROST_TASK] Got public key {:?}", public_key);
            if let Ok(secp_pk) = secp256k1::PublicKey::from_slice(
                hex::decode(public_key.publickey).unwrap().as_slice(),
            ) {
                let mut storage = self.storage.write().await;
                storage.aggregate_public_key = Some(secp_pk);

                drop(storage);
            } else {
                warn!(
                    ">>>>>>>>>>> [FROST_TASK] Error converting public key to secp256k1 public key"
                );
            }
        } else {
            debug!(">>>>>>>>>>> [FROST_TASK] No public key found, proceeding with DKG");
        }

        loop {
            // Check if we are in_turn and if we need to run the dkg start process
            // Note this is not enforced in consensus
            let is_inturn = self.epoch_manager.poll().await;

            // start dkg only when we are in turn + initial state + no public key
            // TODO this logic is wrong you only need dkg if there is no public key
            if is_inturn &&
                !self.dkg_state_machine.get_dkg_state().is_running() &&
                self.dkg_state_machine.get_public_key().await.is_err()
            {
                self.start_dkg().await;
            }
            while let Ok(message) = self.frost_task_rx.try_recv() {
                if let FrostNotificationMessage::InitiateSigning(frost_notification) = message {
                    match self
                        .signing_state_machine
                        .initate_signing_session(
                            frost_notification.signing_session_id,
                            frost_notification.psbt,
                        )
                        .await
                    {
                        Ok(_) => {
                            info!(">>>>>>>>>>> [FROST_TASK::SIGNING] Started new signing session successfully")
                        }
                        Err(e) => {
                            error!(">>>>>>>>>>> [FROST_TASK::SIGNING] Error starting new signing session {:?}", e);
                        }
                    }
                } else {
                    warn!(
                        ">>>>>>>>>>> [FROST_TASK] Unhandled frost notification message {:?}",
                        message
                    );
                }
            }
            // receive over a channel message from other peers and update our state machine
            if let Ok((_peerid, msg)) = peer_messages_rx.try_recv() {
                info!(">>>>>>>>>>> [FROST_TASK] Peer messaged received {:?}", msg);
                match msg {
                    PeerMessageResponse::Pbft(_) => {
                        // Nothing to do for pbft related messages. Does are handled by the pbft
                        // task
                        continue;
                    }
                    PeerMessageResponse::Dkg(dkg_response) => {
                        let DkgResponse { response_type, identifier, data } = dkg_response;
                        match response_type {
                            DkgEventResponseType::DkgRound1 => {
                                match self.dkg_state_machine.process_round1(identifier, data).await
                                {
                                    Ok(_) => {
                                        info!(">>>>>>>>>>> [FROST_TASK::DKG] Processed Round 1 dkg package successfully")
                                    }
                                    Err(e) => {
                                        error!(">>>>>>>>>>> [FROST_TASK::DKG] Error processing round 1 dkg package {:?}", e);
                                    }
                                }
                            }
                            DkgEventResponseType::DkgRound2 => {
                                match self.dkg_state_machine.process_round2(identifier, data).await
                                {
                                    Ok(_) => {
                                        info!(">>>>>>>>>>> [FROST_TASK::DKG] Processed Round 2 dkg package successfully")
                                    }
                                    Err(e) => {
                                        error!(">>>>>>>>>>> [FROST_TASK::DKG] Error processing round 2 dkg package {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                    PeerMessageResponse::Signing(signing_response) => {
                        let SigningResponse { response_type, identifier, signing_session_id, psbt } =
                            signing_response;
                        match response_type {
                            SigningEventResponseType::SignerRound1SigningPackage => {
                                match self
                                    .signing_state_machine
                                    .signer_process_round1(
                                        identifier,
                                        signing_session_id.clone(),
                                        psbt,
                                    )
                                    .await
                                {
                                    Ok(_) => {
                                        info!(">>>>>>>>>>> [FROST_TASK::SIGNING] Peer Processed Round 1 signing successfully")
                                    }
                                    Err(e) => {
                                        error!(">>>>>>>>>>> [FROST_TASK::SIGNING] Peer Error processing round 1 signing {:?}", e);
                                        let _ = self
                                            .signing_state_machine
                                            .handle_errored_signing_process(signing_session_id)
                                            .await;
                                    }
                                }
                            }
                            SigningEventResponseType::CoordinatorRound1SigningPackage => match self
                                .signing_state_machine
                                .coordinator_process_round1(
                                    identifier,
                                    signing_session_id.clone(),
                                    psbt,
                                )
                                .await
                            {
                                Ok(_) => {
                                    info!(">>>>>>>>>>> [FROST_TASK::SIGNING] Coordinator Processed Round 1 signing package successfully")
                                }
                                Err(e) => {
                                    error!(">>>>>>>>>>> [FROST_TASK::SIGNING] Coordinator Error processing round 1 signing package {:?}", e);
                                    let _ = self
                                        .signing_state_machine
                                        .handle_errored_signing_process(signing_session_id)
                                        .await;
                                }
                            },
                            SigningEventResponseType::SignerRound2SigningPackage => {
                                match self
                                    .signing_state_machine
                                    .signer_process_round2(
                                        identifier,
                                        signing_session_id.clone(),
                                        psbt,
                                    )
                                    .await
                                {
                                    Ok(_) => {
                                        info!(">>>>>>>>>>> [FROST_TASK::SIGNING] Peer Processed Round 2 signing package successfully")
                                    }
                                    Err(e) => {
                                        error!(">>>>>>>>>>> [FROST_TASK::SIGNING] Peer Error processing round 2 signing package {:?}", e);
                                        let _ = self
                                            .signing_state_machine
                                            .handle_errored_signing_process(signing_session_id)
                                            .await;
                                    }
                                }
                            }
                            SigningEventResponseType::CoordinatorRound2SigningPackage => match self
                                .signing_state_machine
                                .coordinator_process_round2(
                                    identifier,
                                    signing_session_id.clone(),
                                    psbt,
                                )
                                .await
                            {
                                Ok(_) => {
                                    info!(">>>>>>>>>>> [FROST_TASK::SIGNING] Coordinator Processed Round 2 signing package successfully")
                                }
                                Err(e) => {
                                    error!(">>>>>>>>>>> [FROST_TASK::SIGNING] Coordinator Error processing round 2 signing package {:?}", e);
                                    let _ = self
                                        .signing_state_machine
                                        .handle_errored_signing_process(signing_session_id)
                                        .await;
                                }
                            },
                        }
                    }
                }
            }

            // short sleep
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

impl<Client, ToFrostMan> std::fmt::Debug for FrostTask<Client, ToFrostMan>
where
    ToFrostMan: ToFrostManager + Clone,
    Client: Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrostTask").finish_non_exhaustive()
    }
}
