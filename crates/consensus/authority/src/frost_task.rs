use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};

use crate::{
    metrics::AuthorityMetrics,
    prost_parser::{ProstError, ProstMessageSerdelizer},
    random_source_provider::RandomSource,
    signing::SigningStateMachine,
    utils::{
        get_pending_pegouts_from_pegout_data, get_utxos_from_pegin_meta, retry_exec,
        validate_psbt_by_ids,
    },
    Storage,
};
use bitcoin::consensus::Encodable;
use btcserverlib::{
    extended_client::{BtcServerExtendedApi, GrpcClientError},
    wallet::psbt::frost_id_from_bytes,
};
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
        DkgResponse, FrostPeerCommand, PeerMessageResponse, SigningEventResponseType,
        SigningResponse, WalletStateResponse,
    },
    NetworkHandle,
};
use reth_primitives::header_ext::HeaderExt;
use reth_provider::{
    BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions, StateProviderFactory,
};
use reth_revm::primitives::FixedBytes;
use tendermint_rpc::client::HttpClient;
use tokio::sync::mpsc::{self, error::SendError};
use tracing::{error, info, warn};

// TODO: @rwlock Combine with FrostTaskError?
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

// TODO: @rwlock Combine with FrostTaskError?
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
    /// signing state machine
    pub(crate) signing_state_machine: SigningStateMachine<ToFrostMan, Source, BtcServerClient>,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<EF, BF, DB>,
    /// A handle to the `DkgRunnerTask` task. This is only `Some` if no
    /// aggregate public key is available, and the `start_task` method has hence
    /// started the DKG process.
    dkg_task: Option<mpsc::Sender<DkgResponse>>,
    /// Pre-configured data-parser
    compressor: DataParser,
    /// btc server client
    btc_server: BtcServerClient,
    /// Authority Metrics
    metrics: Arc<AuthorityMetrics>,
    /// cometbft light client provider
    cbft_rpc_provider: HttpClient,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum FrostTaskError {
    #[error("Unable to get all connected peers {0}")]
    UnableToGetAllConnectedPeers(#[from] SendError<FrostCommand>),
}

impl<EF, BF, DB, ToFrostMan, Source, BtcServerClient>
    FrostTask<EF, BF, DB, ToFrostMan, Source, BtcServerClient>
where
    ToFrostMan: 'static + Send + Sync + ToFrostManager + Clone,
    BF: Clone + 'static + Send + Sync,
    DB: BlockReaderIdExt + StateProviderFactory + CanonStateSubscriptions + Clone + 'static,
    EF: Clone + 'static + Send + Sync,
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
            signing_state_machine,
            storage,
            btc_server,
            dkg_task: None,
            compressor,
            metrics,
            cbft_rpc_provider,
        }
    }

    async fn is_syncing(&self) -> Result<bool, SyncError> {
        let status = self.cbft_rpc_provider.status().await?;
        Ok(status.sync_info.catching_up)
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

        let mut received_healthy_chunks = 0;
        let mut total_expected_chunks;
        let mut is_final_chunk_received = false;

        // get the stream from the response
        while let Some(item) = response.next().await {
            match item {
                Ok(prost_serialized_pegout_ids) => {
                    total_expected_chunks = prost_serialized_pegout_ids.total_chunks;
                    if prost_serialized_pegout_ids.is_final {
                        is_final_chunk_received = true;
                    }
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
                    received_healthy_chunks += 1;

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
                Err(e) => {
                    error!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Got grpc error {:?}", e);
                    continue;
                }
            }

            if (received_healthy_chunks == total_expected_chunks) && is_final_chunk_received {
                info!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Received all chunks");
            } else {
                warn!(target: "consensus::authority::forst_task::send_serialized_compressed_finalized_pegout_ids", "Received {} out of {} chunks", received_healthy_chunks, total_expected_chunks);
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

        let mut peer_messages_rx = match self
            .frost_handle
            .send_command(FrostCommand::GetPeerMessagesStream(peer_messages_tx))
        {
            Ok(_) => {
                // only await on the receiver if the send was successful
                match peer_messages_rx.await {
                    Ok(rx) => rx,
                    Err(e) => {
                        error!(target: "consensus::authority::frost_task::start_task", "Error getting receiver handle = {:?}", e);
                        panic!("Error getting receiver handle. Error - {e:?}");
                    }
                }
            }
            Err(e) => {
                error!(target: "consensus::authority::frost_task::start_task", "Failed to send GetPeerMessagesStream frost command {}", e);
                panic!("Failed to send GetPeerMessagesStream frost command - {e:?}");
            }
        };

        // Calling get pk
        // Attempt to get the aggregate public key and store in storage
        if let Ok(public_key) = self.btc_server.get_public_key(client::Empty {}).await {
            info!(target: "consensus::authority::frost_task::start_task", " received aggregate public key from dkg state machine {:?}", public_key);
            if let Ok(secp_pk) = secp256k1::PublicKey::from_slice(
                hex::decode(public_key.publickey)
                    .expect("invalid aggregated public key")
                    .as_slice(),
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
            warn!(target: "consensus::authority::frost_task::start_task", "No public key found, proceeding with DKG");

            // Start the dkg state machine task runner.
            let tx = DkgRunnerTask::new(
                self.frost_handle.clone(),
                self.frost_config.authorities.as_ref(),
                self.storage.clone(),
                self.btc_server.clone(),
                Arc::clone(&self.metrics),
            );
            self.dkg_task = Some(tx);

            info!(target: "consensus::authority::frost_task::start_task", "DKG runner task started...");
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
                                        // We need to panic bc we have no way to recover any missed pegouts and they won't be sent to the btc server.
                                        // We could rollback and sync to reprocess the pegouts but we would intentionally be replaying old pegouts.
                                        panic!("Error sending checkpoint to btc server: {}", err);
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
                let frost_identifier = match message_context.frost_identifier {
                    Some(frost_identifier) => frost_identifier,
                    None => {
                        error!(target: "consensus::authority::frost_task::start_task", "Frost identifier not found for peer id {:?}", peer_id);
                        continue;
                    }
                };

                match peer_message {
                    PeerMessageResponse::Error(err) => {
                        error!(target: "consensus::authority::frost_task::start_task", "Received error from peer {:?}: {:?}", peer_id, err);
                        continue;
                    }
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

                        // get all frost peers connections
                        let all_peers_handle = {
                            let (tx, rx) = tokio::sync::oneshot::channel();

                            let cmd = FrostCommand::GetAllConnectedPeers(tx);
                            if let Err(e) = self.frost_handle.send_command(cmd) {
                                error!(target: "consensus::authority::frost_task::start_task", "Error getting all peers handle {:?}", e);
                                continue;
                            }

                            rx.await.expect("expect all peers handle to exist")
                        };

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
                    PeerMessageResponse::Dkg(dkg_response) => {
                        let Some(task) = self.dkg_task.as_ref() else {
                            warn!(target: "consensus::authority::frost_task::start_task", "Dkg task is not running, dropping request...");
                            continue;
                        };

                        if let Err(err) = task.send(dkg_response).await {
                            warn!(target: "consensus::authority::frost_task::start_task", "Failed to send dkg response to task: {:?}", err);
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

struct DkgRunnerTask<EF, BF, DB, ToFrostMan, BtcServerClient> {
    rx: mpsc::Receiver<DkgResponse>,
    // Frost network Handler
    frost_handle: ToFrostMan,
    // Frost Id lookup table
    frost_ids: HashMap<frost_secp256k1_tr::Identifier, secp256k1::PublicKey>,
    // Shared storage to insert aggregate public key
    storage: Storage<EF, BF, DB>,
    // btc-server client
    btc_server: BtcServerClient,
    // Authority Metrics
    metrics: Arc<AuthorityMetrics>,
}

impl<EF, BF, DB, ToFrostMan, BtcServerClient> DkgRunnerTask<EF, BF, DB, ToFrostMan, BtcServerClient>
where
    EF: 'static + Send + Sync,
    BF: 'static + Send + Sync,
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonStateSubscriptions
        + Clone
        + 'static
        + Send
        + Sync,
    ToFrostMan: 'static + Send + Sync + ToFrostManager,
    BtcServerClient: BtcServerExtendedApi,
{
    #[allow(clippy::new_ret_no_self)]
    fn new(
        frost_handle: ToFrostMan,
        authorities: &[secp256k1::PublicKey],
        storage: Storage<EF, BF, DB>,
        btc_server: BtcServerClient,
        metrics: Arc<AuthorityMetrics>,
    ) -> mpsc::Sender<DkgResponse> {
        let (tx, rx) = mpsc::channel(100);

        let frost_ids = authorities
            .iter()
            .enumerate()
            .map(|(index, pk)| {
                let frost_id = authority_index_to_frost_identifier(index as u16);
                (frost_id, *pk)
            })
            .collect();

        let this = DkgRunnerTask { rx, frost_handle, frost_ids, storage, btc_server, metrics };

        // Spawn-off the task, which will keep interacting with the btc-server.
        tokio::spawn(this.run());

        tx
    }
    async fn run(mut self) {
        const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

        // On startup, we call the btc-server immediately to get the initial
        // payloads. Only the coordinator will have something to send at this
        // point, while non-coordinators wait for the coordinators first message
        // before any messages get sent.
        let mut timeout = Duration::from_millis(0);

        loop {
            match tokio::time::timeout(timeout, self.rx.recv()).await {
                // Received a DKG payload from the frost task, forwarding to btc-server.
                Ok(Some(dkg)) => {
                    let req = client::DkgPayload {
                        sender: dkg.sender,
                        recipient: dkg.recipient,
                        payload: dkg.data,
                    };

                    let resp = match self.btc_server.new_dkg_payload(req).await {
                        Ok(r) => r,
                        Err(err) => {
                            timeout = DEFAULT_TIMEOUT;
                            error!(target: "consensus::authority::frost_task::DkgRunnerTask", "Error sending dkg payload to btc server {:?}", err);
                            continue;
                        }
                    };

                    if let Ok(resp) = self.btc_server.get_public_key(client::Empty {}).await {
                        self.metrics.created_agg_pub_keys.increment(1);

                        // decode the public key and assign it to the self variable
                        let public_key_package = secp256k1::PublicKey::from_str(&resp.publickey)
                            .expect("invalid aggregated public key");

                        let mut storage = self.storage.write().await;
                        storage.aggregate_public_key = Some(public_key_package);
                    }

                    // Update timeout at which point the btc-server should be called again.
                    timeout = Duration::from_millis(resp.timeout);

                    // Gossip the payloads to all frost peers.
                    if self.gossip_payloads(resp.payloads).await.is_err() {
                        error!(target: "consensus::authority::frost_task::DkgRunnerTask", "Failed to gossip payloads. Wait for the next message");
                        continue;
                    }
                }
                // Frost task dropped the handle, exiting...
                Ok(None) => {
                    info!(target: "consensus::authority::frost_task::DkgRunnerTask", "Received shutdown signal");
                    break;
                }
                // Timeout triggered, calling the btc-server to generate new payloads.
                Err(_) => {
                    warn!(target: "consensus::authority::frost_task::DkgRunnerTask", "DKG timeout triggered");

                    let resp = match self.btc_server.get_dkg_payloads(client::Empty {}).await {
                        Ok(r) => r,
                        Err(err) => {
                            timeout = DEFAULT_TIMEOUT;
                            error!(target: "consensus::authority::frost_task::DkgRunnerTask", "Error getting dkg payloads from btc server {:?}", err);
                            continue;
                        }
                    };

                    // Update timeout at which point the btc-server should be called again.
                    timeout = Duration::from_millis(resp.timeout);

                    if self.gossip_payloads(resp.payloads).await.is_err() {
                        error!(target: "consensus::authority::frost_task::DkgRunnerTask", "Failed to gossip payloads. Wait for the next message");
                        continue;
                    }
                }
            }
        }
    }
    async fn gossip_payloads(
        &self,
        payloads: Vec<client::DkgPayload>,
    ) -> Result<(), FrostTaskError> {
        if payloads.is_empty() {
            return Ok(());
        }

        info!(target: "consensus::authority::frost_task::DkgRunnerTask", "Ready to gossip {} generated DKG payload(s)", payloads.len());

        // get all frost peers connections
        let all_peers_handles = {
            let (tx, rx) = tokio::sync::oneshot::channel();

            let cmd = FrostCommand::GetAllConnectedPeers(tx);
            if let Err(e) = self.frost_handle.send_command(cmd) {
                error!(target: "consensus::authority::frost_task::DkgRunnerTask", "Failed to send GetAllConnectedPeers frost command {}", e);
                return Err(FrostTaskError::UnableToGetAllConnectedPeers(e));
            }

            rx.await.expect("expect all peers handle to exist")
        };

        for payload in payloads {
            let recipient = frost_id_from_bytes(&payload.recipient).expect("valid frost id");

            // Lookup the public key of the recipient.
            let Some(pk) = self.frost_ids.get(&recipient) else {
                error!(target: "consensus::authority::frost_task::DkgRunnerTask", "No Frost Id lookup available for recipient {:?}, dropping DKG payload...", recipient);
                continue;
            };

            let pk_string = pk.to_string();

            // TODO (lamafab): This could be improved, by using a hashmap or so.
            let Some(peer_data) = all_peers_handles
                .iter()
                .find(|(_, peer_data)| peer_data.frost_identifier == recipient)
                .map(|(_, peer_data)| peer_data)
            else {
                warn!(target: "consensus::authority::frost_task::DkgRunnerTask", "Peer handle not found for recipient {}, dropping DKG payload...", pk_string);
                continue;
            };

            let resp = PeerMessageResponse::Dkg(DkgResponse {
                data: payload.payload,
                sender: payload.sender,
                recipient: payload.recipient,
            });

            match peer_data.peer_commands_tx.send(FrostPeerCommand::PeerMessage(resp)) {
                Ok(_) => {
                    info!(target: "consensus::authority::frost_task::DkgRunnerTask", "Gossiping DKG payload to peer {}", pk_string);
                }
                Err(err) => {
                    error!(target: "consensus::authority::frost_task::DkgRunnerTask", "Error sending DKG payload to recipient {}: {:?}", pk_string, err);
                }
            }
        }

        Ok(())
    }
}
