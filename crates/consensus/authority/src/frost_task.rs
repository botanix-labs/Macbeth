use std::{sync::Arc, time::Duration};

use crate::{
    dkg::DKGStateMachine,
    metrics::AuthorityMetrics,
    prost_parser::{ProstError, ProstMessageSerdelizer},
    random_source_provider::RandomSource,
    signing::SigningStateMachine,
    utils::{
        deserialize_frost_peer_id, get_pending_pegouts_from_pegout_data, get_utxos_from_pegin_meta,
        retry_exec, validate_psbt_by_ids,
    },
    Storage,
};
use bitcoin::consensus::Encodable;
use btcserverlib::extended_client::{BtcServerExtendedApi, GrpcClientError};
use client::ConsensusCheckpointRequest;
use comet_bft_rpc::{Client, CometBftRpcFactory, HttpCometBFTRpcClientFactory};
use futures::{pin_mut, StreamExt};
use reth_chainspec::ChainSpec;
use reth_data_parser::{DataParser, Error as DataParserError};
use reth_network::{
    frost::{
        manager::{
            authority_index_to_frost_identifier, FrostCommand, FrostConfig, PeerData,
            ToFrostManager,
        },
        DkgEventResponseType, DkgResponse, FrostPeerCommand, PeerMessageResponse,
        SigningEventResponseType, SigningResponse, WalletStateResponse,
    },
    NetworkHandle,
};
use reth_primitives::header_ext::HeaderExt;
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions, StateProviderFactory,
};
use reth_revm::primitives::FixedBytes;
use tendermint_rpc::client::HttpClient;
use tracing::{debug, error, info, warn};

#[derive(Debug, thiserror::Error)]
/// Errors that can occur during synchronization.
pub(crate) enum SyncError {
    #[error("tendermint error")]
    /// Error related to Tendermint.
    Tendermint(#[from] tendermint::Error),
    /// Error related to Tendermint RPC.
    #[error("tendermint rpc error")]
    TendermintRpc(#[from] tendermint_rpc::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FinalizedPegoutIdsSyncSerializationError {
    #[error("Received a grpc client error {0}")]
    Grpc(#[from] GrpcClientError),
    #[error("prost error {0}")]
    Prost(#[from] ProstError),
    #[error("data parser error {0}")]
    DataParser(#[from] DataParserError),
}

#[allow(dead_code)]
pub struct FrostTask<EF, BF, DB, ToFrostMan, Source, BtcServerClient> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Frost configuration
    pub(crate) frost_config: FrostConfig,
    /// dkg state machine
    pub(crate) dkg_state_machine: DKGStateMachine<EF, BF, DB, ToFrostMan, BtcServerClient>,
    /// signing state machine
    pub(crate) signing_state_machine: SigningStateMachine<ToFrostMan, Source, BtcServerClient>,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<EF, BF, DB>,
    /// Pre-configured data-parser
    compressor: DataParser,
    /// btc server client
    btc_server: BtcServerClient,
    /// Authority Metrics
    metrics: Arc<AuthorityMetrics>,
    /// cometbft light client provider
    cbft_rpc_provider: HttpClient,
}

impl<EF, BF, DB, ToFrostMan, Source, BtcServerClient>
    FrostTask<EF, BF, DB, ToFrostMan, Source, BtcServerClient>
where
    ToFrostMan: ToFrostManager + Clone,
    BF: Clone,
    DB: BlockReaderIdExt + StateProviderFactory + CanonStateSubscriptions + Clone + 'static,
    EF: Clone,
    Source: RandomSource,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        btc_server: BtcServerClient,
        network_handle: NetworkHandle,
        frost_handle: ToFrostMan,
        config: FrostConfig,
        storage: Storage<EF, BF, DB>,
        compressor: DataParser,
        random_source_provider: Source,
        metrics: Arc<AuthorityMetrics>,
        cometbft_rpc_factory: HttpCometBFTRpcClientFactory,
    ) -> Self {
        info!(target: "consensus::authority::frost_task::new", "Frost authority index: {}/{}", config.authority_index, config.authorities.len() - 1);

        let dkg_state_machine = DKGStateMachine::new(
            btc_server.clone(),
            storage.clone(),
            frost_handle.clone(),
            config.clone(),
            metrics.clone(),
        );

        let signing_state_machine = SigningStateMachine::new(
            chain_spec,
            btc_server.clone(),
            frost_handle.clone(),
            config.clone(),
            random_source_provider,
            metrics.clone(),
        );

        let cbft_rpc_provider =
            cometbft_rpc_factory.build_and_connect().expect("light client to connect");

        Self {
            network_handle,
            frost_handle,
            frost_config: config,
            dkg_state_machine,
            signing_state_machine,
            storage,
            btc_server,
            compressor,
            metrics,
            cbft_rpc_provider,
        }
    }

    async fn is_syncing(&self) -> Result<bool, SyncError> {
        let status = self.cbft_rpc_provider.status().await?;
        Ok(status.sync_info.catching_up)
    }

    async fn start_dkg(&mut self) {
        // check if we are connected to all frost peers when in turn
        let (sender, receiver) = tokio::sync::oneshot::channel::<bool>();
        if let Err(e) = self.frost_handle.send_command(FrostCommand::CheckConnectedToAll(sender)) {
            error!(target: "consensus::authority::frost_task::start_dkg", "Failed to send CheckConnectedToaAll frost command {}", e);
        }
        match receiver.await {
            Ok(is_connected) => {
                if !is_connected {
                    info!(target: "consensus::authority::frost_task::start_dkg", "Not yet connected to all frost peers. Waiting to start DKG ....");
                    return;
                }
                info!(target: "consensus::authority::frost_task::start_dkg", "Connected to all frost peers {}", is_connected);
                // start the dkg process
                info!(target: "consensus::authority::frost_task::start_dkg", "Starting the DKG state machine...");
                let _ = self.dkg_state_machine.start_coordinator().await;
            }
            Err(e) => {
                error!("Check for connection to other peers failed {:?}", e);
            }
        }
    }

    async fn send_serialized_compressed_finalized_pegout_ids(
        &mut self,
        chunk_size: u64,
        peer_data: &PeerData,
        wallet_state_response: &WalletStateResponse,
    ) -> Result<(), FinalizedPegoutIdsSyncSerializationError> {
        // create the request
        let request = client::GetFinalizedPegoutIdsRequest { chunk_size };

        // call the streaming RPC method
        let response = self.btc_server.get_finalized_pegout_ids(request).await?;
        pin_mut!(response);

        // get the stream from the response
        while let Some(item) = response.next().await {
            let prost_serialized_pegout_ids = item.map_err(|e| {
                error!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Got grpc error {:?}", e);
                FinalizedPegoutIdsSyncSerializationError::Grpc(GrpcClientError::Call(e))
            })?;

            if prost_serialized_pegout_ids.data.is_empty() {
                warn!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Received empty finalized pegout ids from btc server");
                continue;
            }

            // serialize the prost message
            let prost_message_wrapper = ProstMessageSerdelizer(prost_serialized_pegout_ids);
            let prost_serialized = prost_message_wrapper.serialize().map_err(|e| {
                error!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Got serializer error {:?}", e);
                FinalizedPegoutIdsSyncSerializationError::Prost(e)
            })?;

            // now compress the prost message
            let prost_serialized_compressed = self.compressor.compress(&prost_serialized).await.map_err(|e| {
                error!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Got compressor error {:?}", e);
                FinalizedPegoutIdsSyncSerializationError::DataParser(e)
            })?;

            let mut wallet_state_response = wallet_state_response.clone();
            wallet_state_response.finalized_pegout_ids = prost_serialized_compressed;

            info!(target: "consensus::authority::frost_task::start_task", "Sending wallet state to peer {:?}", peer_data.peer_id);
            if let Err(e) = peer_data.peer_commands_tx.send(FrostPeerCommand::PeerMessage(
                PeerMessageResponse::WalletState(wallet_state_response),
            )) {
                error!(target: "consensus::authority::frost_task::start_task", "Error sending wallet state message to peer {:?}: {:?}",  peer_data.peer_id, e);
                continue;
            }
        }
        Ok(())
    }

    fn has_wallet_state(response: &WalletStateResponse) -> bool {
        !response.finalized_pegout_ids.is_empty()
    }

    pub async fn start_task(&mut self, mut abci_started_rx: tokio::sync::oneshot::Receiver<()>) {
        // before we start get a proper event receiver
        let (peer_messages_tx, peer_messages_rx) = tokio::sync::oneshot::channel();
        if let Err(e) =
            self.frost_handle.send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))
        {
            error!(target: "consensus::authority::frost_task::start_task", "Failed to send GetPeerMessagesStream frost command {}", e);
        }
        let mut peer_messages_rx = match peer_messages_rx.await {
            Ok(peer_messages_rx) => peer_messages_rx,
            Err(e) => {
                error!(target: "consensus::authority::frost_task::start_task", "Error getting receiver handle = {:?}", e);
                panic!("Error getting receiver handle");
            }
        };

        // Calling get pk
        // Attempt to get the aggregate public key and store in storage
        if let Ok(public_key) = self.dkg_state_machine.get_public_key().await {
            info!(target: "consensus::authority::frost_task::start_task", " received aggregate public key from dkg state machine {:?}", public_key);
            if let Ok(secp_pk) = secp256k1::PublicKey::from_slice(
                hex::decode(public_key.publickey).unwrap().as_slice(),
            ) {
                let mut storage = self.storage.inner.write().await;
                storage.aggregate_public_key = Some(secp_pk);

                drop(storage);
            } else {
                warn!(
                    target: "consensus::authority::frost_task::start_task", "converting public key to secp256k1 public key"
                );
            }
        } else {
            debug!(target: "consensus::authority::frost_task::start_task", "No public key found, proceeding with DKG");
        }
        let mut canon_state_notifs = self.storage.client.subscribe_to_canonical_state();

        let mut abci_started = false;
        loop {
            // check if abci has started
            if abci_started_rx.try_recv().is_ok() {
                abci_started = true;
            }
            if abci_started {
                // get sync status
                match self.is_syncing().await {
                    Ok(is_syncing) => {
                        self.storage.inner.write().await.is_block_syncing = is_syncing;
                        if is_syncing {
                            info!(target: "consensus::authority::frost_task::start_task", "Node is syncing, pausing frost task...");
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            continue;
                        }
                    }
                    Err(e) => {
                        info!(target: "consensus::authority::frost_task::start_task", "Error getting block sync status {:?}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }

            let my_frost_id =
                authority_index_to_frost_identifier(self.frost_config.authority_index as u16);
            let is_coordinator = self.dkg_state_machine.coordinator_identifier() == my_frost_id;
            // start dkg only when we are the coordinator+ initial state + no public key
            if is_coordinator &&
                !self.dkg_state_machine.get_dkg_state().is_running() &&
                self.dkg_state_machine.get_public_key().await.is_err()
            {
                self.start_dkg().await;
            }

            // Receive canon state notifications
            while let Ok(ref notification) = canon_state_notifs.try_recv() {
                info!(target: "consensus::authority::frost_task::start_task", "canon state notification received for block number {:?}", notification.tip().number);
                match notification {
                    CanonStateNotification::Commit { new, pegins, pegouts } => {
                        let tip = new.tip();
                        // TODO(armins) make this block of code more readable by removing all the
                        // matches
                        let edh = match tip.header().deserialize_extra_data_header() {
                            Ok(edh) => edh,
                            Err(e) => {
                                error!(target: "consensus::authority::frost_task::start_task", "Error deserializing extra data header: {}", e);
                                continue;
                            }
                        };

                        let pegins = pegins
                            .as_ref()
                            .map_or_else(Vec::new, |pegins| get_utxos_from_pegin_meta(pegins));

                        // convert pegouts into correct format
                        let pending_pegouts = pegouts.as_ref().map_or_else(Vec::new, |pegouts| {
                            get_pending_pegouts_from_pegout_data(pegouts, tip.number)
                        });

                        let cp_block_hash = edh.bitcoin_block_hash;
                        let mut block_hash_writer = vec![];
                        match cp_block_hash.consensus_encode(&mut block_hash_writer) {
                            Ok(_) => {
                                let btc_server_capture = self.btc_server.clone();
                                let block_hash_writer = block_hash_writer.clone();
                                let pegins = pegins.clone();
                                let pending_pegouts = pending_pegouts.clone();

                                let fut = move || {
                                    let mut btc_server = btc_server_capture.clone();
                                    let block_hash = block_hash_writer.clone();
                                    let pegins_data = pegins.clone();
                                    let pending_data = pending_pegouts.clone();

                                    async move {
                                        btc_server
                                            .new_consensus_checkpoint(ConsensusCheckpointRequest {
                                                checkpoint_block_hash: block_hash,
                                                pegins: pegins_data,
                                                pending_pegouts: pending_data,
                                            })
                                            .await
                                    }
                                };
                                match retry_exec(
                                    "new_consensus_checkpoint",
                                    fut,
                                    3,
                                    Duration::from_secs(2),
                                )
                                .await
                                {
                                    Ok(_) => {
                                        info!(target: "consensus::authority::frost_task::start_task", "Sent checkpoint to btc server");
                                    }
                                    Err(err) => {
                                        error!(target: "consensus::authority::frost_task::start_task", "Error sending checkpoint to btc server: {}", err);
                                    }
                                }
                            }
                            Err(e) => {
                                error!(target: "consensus::authority::frost_task::start_task", "Error encoding checkpoint block hash: {}", e);
                            }
                        }

                        // check if epoch block and if we are the coordinator
                        // if so, initiate signing session

                        if tip.is_poa_epoch() {
                            if !self.signing_state_machine.is_coordinator() {
                                info!("Received canon state notification during epoch block but we're not the coordinator");
                                continue;
                            } else {
                                // create psbt and send init signing message
                                match crate::utils::get_psbt(
                                    &mut self.btc_server,
                                    &tip.hash(),
                                    cp_block_hash,
                                )
                                .await
                                {
                                    Ok(psbt_payload) => {
                                        // validate psbt
                                        let psbt = match bitcoin::Psbt::deserialize(
                                            psbt_payload.psbt.as_slice(),
                                        ) {
                                            Ok(psbt) => psbt,
                                            Err(e) => {
                                                error!(target: "consensus::authority::frost_task::start_task", "Error deserializing psbt {:?}", e);
                                                continue;
                                            }
                                        };
                                        match validate_psbt_by_ids(
                                            self.storage.client.clone(),
                                            self.storage.btc_network,
                                            &psbt,
                                        )
                                        .await
                                        {
                                            Ok(_) => {
                                                info!(target: "consensus::authority::frost_task::start_task", "Validated psbt successfully")
                                            }
                                            Err(e) => {
                                                error!(target: "consensus::authority::frost_task::start_task", "Error validating psbt {:?}", e);
                                                continue;
                                            }
                                        }

                                        match self
                                            .signing_state_machine
                                            .initate_signing_session(tip.hash(), psbt_payload.psbt)
                                            .await
                                        {
                                            Ok(_) => {
                                                info!(target: "consensus::authority::frost_task::start_task", "Started new signing session successfully")
                                            }
                                            Err(e) => {
                                                error!(target: "consensus::authority::frost_task::start_task", "Error starting new signing session {:?}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!(target: "consensus::authority", ?e, "Failed to get psbt");
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // Ignore other notifications
                    }
                }
            }
            // receive over a channel message from other peers and update our state machine
            while let Ok(message_context) = peer_messages_rx.try_recv() {
                let peer_message = message_context.message;
                let peer_id = message_context.peer_id;
                let frost_identifier = message_context.frost_identifier;
                match peer_message {
                    PeerMessageResponse::WalletState(response) => {
                        // Only handle response if it has no state: responses with state are also
                        // sent to WalletStateSyncEngine::sync_wallet_state
                        // which updates the wallet state. This code block
                        // handles sending our wallet state to a peer
                        //
                        if Self::has_wallet_state(&response) {
                            info!(target: "consensus::authority::wallet_syncer::start_task", "Received wallet state in frost task from peer {:?}", peer_id);
                            continue;
                        }

                        match self.dkg_state_machine.get_all_peers_handle().await {
                            Ok(all_peers_handle) => {
                                info!(target: "consensus::authority::frost_task::start_task", "Got all peers handle");
                                if !all_peers_handle.contains_key(&peer_id) {
                                    error!(target: "consensus::authority::frost_task::start_task", "Peer handle not found for peer id {:?}", peer_id);
                                    continue;
                                }
                                let peer_handle =
                                    all_peers_handle.get(&peer_id).expect("peer handle to exist");

                                if let Err(e) = self
                                    .send_serialized_compressed_finalized_pegout_ids(
                                        self.frost_config.wallet_state_sync_chunk_size,
                                        peer_handle,
                                        &response,
                                    )
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::start_task", "Error getting serialized compressed finalized pegout ids: {:?}", e);
                                    continue;
                                }
                            }
                            Err(e) => {
                                error!(target: "consensus::authority::frost_task::start_task", "Error getting all peers handle {:?}", e);
                                continue;
                            }
                        }
                    }
                    PeerMessageResponse::Dkg(dkg_response) => {
                        let DkgResponse { response_type, identifier, data } = dkg_response;
                        let frost_identifier = match deserialize_frost_peer_id(identifier) {
                            Ok(frost_identifier) => frost_identifier,
                            Err(e) => {
                                error!(target: "consensus::authority::frost_task::start_task", "Error deserializing frost identifier in DKG payload {:?}", e);
                                continue;
                            }
                        };
                        match response_type {
                            DkgEventResponseType::DkgRound1Request => {
                                match self.dkg_state_machine.process_round1_request().await {
                                    Ok(_) => {
                                        info!(target: "consensus::authority::frost_task::start_task", "Processed Round 1 request dkg package successfully")
                                    }
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::start_task", "Error processing round 1 request dkg package {:?}", e.to_string());
                                    }
                                }
                            }
                            DkgEventResponseType::DkgRound1 => {
                                match self
                                    .dkg_state_machine
                                    .process_round1(&frost_identifier, data)
                                    .await
                                {
                                    Ok(_) => {
                                        info!(target: "consensus::authority::frost_task::start_task", "Processed Round 1 dkg package successfully")
                                    }
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::start_task", "Error processing round 1 dkg package {:?}", e.to_string());
                                    }
                                }
                            }
                            DkgEventResponseType::DkgRound2 => {
                                match self
                                    .dkg_state_machine
                                    .process_round2(&frost_identifier, data)
                                    .await
                                {
                                    Ok(_) => {
                                        info!(target: "consensus::authority::frost_task::start_task", "Processed Round 2 dkg package successfully")
                                    }
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::start_task", "Error processing round 2 dkg package {:?}", e.to_string());
                                    }
                                }
                            }
                        }
                    }
                    PeerMessageResponse::Signing(signing_response) => {
                        let SigningResponse { response_type, signing_session_id, psbt } =
                            signing_response;
                        let signing_session_id = match FixedBytes::try_from(
                            signing_session_id.as_slice(),
                        ) {
                            Ok(signing_session_id) => signing_session_id,
                            Err(e) => {
                                error!(target: "consensus::authority::frost_task::start_task", "Error deserializing signing session id {:?}", e);
                                continue;
                            }
                        };
                        match response_type {
                            SigningEventResponseType::SignerRound1SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(psbt.as_slice()) {
                                    Ok(psbt) => psbt,
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::SignerRound1SigningPackage", "Error deserializing psbt {:?}", e);
                                        continue;
                                    }
                                };

                                if let Err(e) = validate_psbt_by_ids(
                                    self.storage.client.clone(),
                                    self.storage.btc_network,
                                    &psbt_res,
                                )
                                .await
                                {
                                    error!(target: "consensus::authority::frost_task::SignerRound1SigningPackage", "Error validating psbt {:?}", e);
                                    continue;
                                }

                                if let Err(e) = self
                                    .signing_state_machine
                                    .signer_process_round1(
                                        &frost_identifier,
                                        signing_session_id,
                                        psbt,
                                    )
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::SignerRound1SigningPackage", "Peer Error processing round 1 signing {:?}", e);
                                }
                            }
                            SigningEventResponseType::CoordinatorRound1SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(psbt.as_slice()) {
                                    Ok(psbt) => psbt,
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::CoordinatorRound1SigningPackage", "Error deserializing psbt {:?}", e);
                                        continue;
                                    }
                                };

                                if let Err(e) = validate_psbt_by_ids(
                                    self.storage.client.clone(),
                                    self.storage.btc_network,
                                    &psbt_res,
                                )
                                .await
                                {
                                    error!(target: "consensus::authority::frost_task::CoordinatorRound1SigningPackage", "Error validating psbt {:?}", e);
                                    continue;
                                }

                                if let Err(e) = self
                                    .signing_state_machine
                                    .coordinator_process_round1(
                                        &frost_identifier,
                                        signing_session_id,
                                        psbt,
                                    )
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::CoordinatorRound1SigningPackage", "Coordinator Error processing round 1 signing package {:?}", e);
                                }
                            }
                            SigningEventResponseType::SignerRound2SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(psbt.as_slice()) {
                                    Ok(psbt) => psbt,
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::SignerRound2SigningPackage", "Error deserializing psbt {:?}", e);
                                        continue;
                                    }
                                };

                                if let Err(e) = validate_psbt_by_ids(
                                    self.storage.client.clone(),
                                    self.storage.btc_network,
                                    &psbt_res,
                                )
                                .await
                                {
                                    error!(target: "consensus::authority::frost_task::SignerRound2SigningPackage", "Error validating psbt {:?}", e);
                                    continue;
                                }

                                if let Err(e) = self
                                    .signing_state_machine
                                    .signer_process_round2(
                                        &frost_identifier,
                                        signing_session_id,
                                        psbt,
                                    )
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::SignerRound2SigningPackage", "Peer Error processing round 2 signing package {:?}", e);
                                }
                            }
                            SigningEventResponseType::CoordinatorRound2SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(psbt.as_slice()) {
                                    Ok(psbt) => psbt,
                                    Err(e) => {
                                        error!(target: "consensus::authority::frost_task::CoordinatorRound1SigningPackage", "Error deserializing psbt {:?}", e);
                                        continue;
                                    }
                                };

                                if let Err(e) = validate_psbt_by_ids(
                                    self.storage.client.clone(),
                                    self.storage.btc_network,
                                    &psbt_res,
                                )
                                .await
                                {
                                    error!(target: "consensus::authority::frost_task::CoordinatorRound1SigningPackage", "Error validating psbt {:?}", e);
                                    continue;
                                }

                                if let Err(e) = self
                                    .signing_state_machine
                                    .coordinator_process_round2(
                                        &frost_identifier,
                                        signing_session_id,
                                        psbt,
                                    )
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::CoordinatorRound2SigningPackage", "Coordinator Error processing round 2 signing package {:?}", e);
                                }
                            }
                        }
                    }
                }
            }

            // short sleep
            tokio::time::sleep(std::time::Duration::from_millis(1250)).await;
        }
    }
}

impl<EF, BF, DB, ToFrostMan, Source, BtcServerClient> std::fmt::Debug
    for FrostTask<EF, BF, DB, ToFrostMan, Source, BtcServerClient>
where
    ToFrostMan: ToFrostManager + Clone,
    Source: RandomSource,
    BtcServerClient: BtcServerExtendedApi + Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrostTask").finish_non_exhaustive()
    }
}
