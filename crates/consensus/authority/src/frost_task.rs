use std::sync::Arc;

use crate::{
    compressor::{Compressor, Error as CompressorError, ProstMessageSerdelizer},
    dkg::DKGStateMachine,
    random_source_provider::RandomSource,
    signing::SigningStateMachine,
    utils::validate_psbt_by_ids,
    Storage,
};

use bitcoin_hashes::Hash;
use btcserverlib::extended_client::{BtcServerExtendedClient, GrpcClientError};
use client::SyncTxIndexRequest;
use reth_blockchain_tree::BlockchainTreeEngine;
use reth_chainspec::ChainSpec;
use reth_network::{
    frost::{
        manager::{authority_index_to_frost_identifier, FrostCommand, FrostConfig, ToFrostManager},
        DkgEventResponseType, DkgResponse, FrostPeerCommand, PeerMessageResponse,
        SigningEventResponseType, SigningResponse,
    },
    NetworkHandle,
};
use reth_primitives::header_ext::HeaderExt;
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotification, StateProviderFactory,
};
use reth_revm::primitives::FixedBytes;
use tokio::sync::{broadcast::Receiver as BroadcastReceiver, oneshot::error::RecvError};
use tracing::{debug, error, info, warn};

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum UtxoSetSyncSerializationError {
    #[error("Failed to receive a frost message from a peer {0}")]
    FrostRecv(RecvError),
    #[error("Received a grpc client error {0}")]
    Grpc(GrpcClientError),
    #[error("compressor error {0}")]
    Compressor(CompressorError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TrackedTxSyncSerializationError {
    #[error("Received a grpc client error {0}")]
    Grpc(GrpcClientError),
    #[error("compressor error {0}")]
    Compressor(CompressorError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PendingPegoutsSyncSerializationError {
    #[error("Received a grpc client error {0}")]
    Grpc(GrpcClientError),
    #[error("compressor error {0}")]
    Compressor(CompressorError),
}

#[allow(dead_code)]
pub struct FrostTask<EF, BF, DB, ToFrostMan, Source> {
    /// Network Handler
    pub(crate) network_handle: NetworkHandle,
    /// Frost network Handler
    pub(crate) frost_handle: ToFrostMan,
    /// Frost configuration
    pub(crate) frost_config: FrostConfig,
    /// dkg state machine
    pub(crate) dkg_state_machine: DKGStateMachine<EF, BF, DB, ToFrostMan>,
    /// signing state machine
    pub(crate) signing_state_machine: SigningStateMachine<ToFrostMan, Source>,
    /// Shared storage to insert aggregate public key
    pub(crate) storage: Storage<EF, BF, DB>,
    /// Pre-configured compressor
    compressor: Compressor,
    /// btc server client
    btc_server: BtcServerExtendedClient,
    /// Channel to receive canon state notifications
    canon_state_notification_receiver: BroadcastReceiver<CanonStateNotification>,
}

impl<EF, BF, DB, ToFrostMan, Source> FrostTask<EF, BF, DB, ToFrostMan, Source>
where
    ToFrostMan: ToFrostManager + Clone,
    BF: Clone,
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: Clone,
    Source: RandomSource,
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        btc_server: BtcServerExtendedClient,
        network_handle: NetworkHandle,
        frost_handle: ToFrostMan,
        config: FrostConfig,
        storage: Storage<EF, BF, DB>,
        compressor: Compressor,
        random_source_provider: Source,
        canon_state_notification_receiver: BroadcastReceiver<CanonStateNotification>,
    ) -> Self {
        info!(target: "consensus::authority::frost_task::new", "Frost authority index: {}/{}", config.authority_index, config.authorities.len() - 1);

        let dkg_state_machine = DKGStateMachine::new(
            btc_server.clone(),
            storage.clone(),
            frost_handle.clone(),
            config.clone(),
        );

        let signing_state_machine = SigningStateMachine::new(
            chain_spec,
            btc_server.clone(),
            frost_handle.clone(),
            config.clone(),
            random_source_provider,
        );

        Self {
            network_handle,
            frost_handle,
            frost_config: config,
            dkg_state_machine,
            signing_state_machine,
            storage,
            btc_server,
            compressor,
            canon_state_notification_receiver,
        }
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

    async fn get_serialized_compressed_utxo_set(
        &mut self,
    ) -> Result<Vec<u8>, UtxoSetSyncSerializationError> {
        let prost_utxos = self.btc_server.get_all_utxos(client::Empty {}).await.map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_utxo_set", "Got grpc error {:?}", e);
            UtxoSetSyncSerializationError::Grpc(e)
        })?;

        // serialize the prost message
        let prost_message_wrapper = ProstMessageSerdelizer(prost_utxos);
        let prost_serialized = prost_message_wrapper.serialize().map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_utxo_set", "Got compressor error {:?}", e);
            UtxoSetSyncSerializationError::Compressor(e)
        })?;

        // now compress the prost message
        let prost_serialized_compressed = self.compressor.compress(&prost_serialized).await.map_err(|e| {
            error!(target: "consensus::authority::utxo_syncer::get_utxo_set", "Got compressor error {:?}", e);
            UtxoSetSyncSerializationError::Compressor(e)
        })?;
        Ok(prost_serialized_compressed)
    }

    async fn get_serialized_compressed_tracked_txs(
        &mut self,
    ) -> Result<Vec<u8>, TrackedTxSyncSerializationError> {
        let prost_tracked_txs = self.btc_server.get_tracked_txs(client::Empty {}).await.map_err(|e| {
            error!(target: "consensus::authority::tracked_tx_syncer::get_tracked_txs", "Got grpc error {:?}", e);
            TrackedTxSyncSerializationError::Grpc(e)
        })?;

        // serialize the prost message
        let prost_message_wrapper = ProstMessageSerdelizer(prost_tracked_txs);
        let prost_serialized = prost_message_wrapper.serialize().map_err(|e| {
            error!(target: "consensus::authority::tracked_tx_syncer::get_tracked_txs", "Got compressor error {:?}", e);
            TrackedTxSyncSerializationError::Compressor(e)
        })?;

        // now compress the prost message
        let prost_serialized_compressed = self.compressor.compress(&prost_serialized).await.map_err(|e| {
            error!(target: "consensus::authority::tracked_tx_syncer::get_tracked_txs", "Got compressor error {:?}", e);
            TrackedTxSyncSerializationError::Compressor(e)
        })?;
        Ok(prost_serialized_compressed)
    }

    async fn get_serialized_compressed_pending_pegouts(
        &mut self,
    ) -> Result<Vec<u8>, PendingPegoutsSyncSerializationError> {
        let prost_pending_pegouts = self.btc_server.get_pending_pegouts(client::Empty {}).await.map_err(|e| {
            error!(target: "consensus::authority::pending_pegouts_syncer::get_pending_pegouts", "Got grpc error {:?}", e);
            PendingPegoutsSyncSerializationError::Grpc(e)
        })?;

        // serialize the prost message
        let prost_message_wrapper = ProstMessageSerdelizer(prost_pending_pegouts);
        let prost_serialized = prost_message_wrapper.serialize().map_err(|e| {
            error!(target: "consensus::authority::pending_pegouts_syncer::get_pending_pegouts", "Got compressor error {:?}", e);
            PendingPegoutsSyncSerializationError::Compressor(e)
        })?;

        // now compress the prost message
        let prost_serialized_compressed = self.compressor.compress(&prost_serialized).await.map_err(|e| {
            error!(target: "consensus::authority::pending_pegouts_syncer::get_pending_pegouts", "Got compressor error {:?}", e);
            PendingPegoutsSyncSerializationError::Compressor(e)
        })?;
        Ok(prost_serialized_compressed)
    }

    pub async fn start_task(&mut self) {
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

        loop {
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
            while let Ok(notification) = self.canon_state_notification_receiver.try_recv() {
                match notification {
                    CanonStateNotification::Commit { new } => {
                        let tip = new.tip();
                        let cp_block_hash = tip
                            .header()
                            .deserialize_extra_data_header()
                            .unwrap()
                            .bitcoin_block_hash;

                        match self
                            .btc_server
                            .tx_index_new_checkpoint(SyncTxIndexRequest {
                                checkpoint_block_hash: cp_block_hash.to_byte_array().to_vec(),
                            })
                            .await
                        {
                            Ok(_) => {
                                info!(target: "consensus::authority::frost_task::start_task", "Sent checkpoint to btc server");
                            }
                            Err(e) => {
                                error!(target: "consensus::authority::frost_task::start_task", "Error sending checkpoint to btc server: {}", e);
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
                                let bitcoin_checkpoint = match tip
                                    .header
                                    .header()
                                    .deserialize_extra_data_header()
                                {
                                    Ok(edh) => edh.bitcoin_block_hash,
                                    Err(e) => {
                                        error!(target: "consensus::authority", ?e, "Failed to deserialize extra data header in frost task");
                                        continue;
                                    }
                                };
                                match crate::utils::get_psbt(
                                    &mut self.btc_server,
                                    &tip.hash(),
                                    bitcoin_checkpoint,
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
            while let Ok((peerid, msg)) = peer_messages_rx.try_recv() {
                match msg {
                    PeerMessageResponse::WalletState(mut response) => {
                        let all_peers_handle = self
                            .dkg_state_machine
                            .get_all_peers_handle()
                            .await
                            .expect("remove this later");
                        let peer_handle = all_peers_handle.get(&peerid).expect("remove this later");

                        // Note its important we do not respond to this message if we are syncing
                        // ourselves This should be checked above
                        let serialized_compressed_utxo_set = match self
                            .get_serialized_compressed_utxo_set()
                            .await
                        {
                            Ok(serialized_compressed_utxo_set) => serialized_compressed_utxo_set,
                            Err(e) => {
                                error!(target: "consensus::authority::utxo_syncer::start_task", "Error getting serialized compressed utxo set: {:?}", e);
                                continue;
                            }
                        };

                        if serialized_compressed_utxo_set.is_empty() {
                            warn!(target: "consensus::authority::utxo_syncer::start_task", "Received empty utxo set from database");
                            continue;
                        }

                        let serialized_compressed_tracked_txs = match self
                            .get_serialized_compressed_tracked_txs()
                            .await
                        {
                            Ok(serialized_compressed_tracked_txs) => {
                                serialized_compressed_tracked_txs
                            }
                            Err(e) => {
                                error!(target: "consensus::authority::tracked_tx_syncer::start_task", "Error getting serialized compressed tracked txs: {:?}", e);
                                continue;
                            }
                        };

                        let serialized_compressed_pending_pegouts = match self
                            .get_serialized_compressed_pending_pegouts()
                            .await
                        {
                            Ok(serialized_compressed_pending_pegouts) => {
                                serialized_compressed_pending_pegouts
                            }
                            Err(e) => {
                                error!(target: "consensus::authority::pending_pegouts_syncer::start_task", "Error getting serialized compressed pending pegouts: {:?}", e);
                                continue;
                            }
                        };

                        // update response with data
                        response.utxos = serialized_compressed_utxo_set;
                        response.tracked_txs = serialized_compressed_tracked_txs;
                        response.pending_pegouts = serialized_compressed_pending_pegouts;

                        if let Err(e) =
                            peer_handle.peer_commands_tx.send(FrostPeerCommand::PeerMessage(
                                PeerMessageResponse::WalletState(response),
                            ))
                        {
                            error!(target: "consensus::authority::wallet_syncer::start_task", "Error sending wallet state message to a peer: {:?}", e);
                            continue;
                        }

                        continue;
                    }
                    PeerMessageResponse::Healthcheck(_) => {
                        // Nothing to do for healthcheck related messages.
                        continue;
                    }
                    PeerMessageResponse::Dkg(dkg_response) => {
                        let DkgResponse { response_type, identifier, data } = dkg_response;
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
                                match self.dkg_state_machine.process_round1(identifier, data).await
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
                                match self.dkg_state_machine.process_round2(identifier, data).await
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
                        let SigningResponse { response_type, identifier, signing_session_id, psbt } =
                            signing_response;
                        let signing_session_id = FixedBytes::from_slice(&signing_session_id);
                        match response_type {
                            SigningEventResponseType::SignerRound1SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(&psbt.as_slice()) {
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
                                    .signer_process_round1(identifier, signing_session_id, psbt)
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::SignerRound1SigningPackage", "Peer Error processing round 1 signing {:?}", e);
                                }
                            }
                            SigningEventResponseType::CoordinatorRound1SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(&psbt.as_slice()) {
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
                                        identifier,
                                        signing_session_id,
                                        psbt,
                                    )
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::CoordinatorRound1SigningPackage", "Coordinator Error processing round 1 signing package {:?}", e);
                                }
                            }
                            SigningEventResponseType::SignerRound2SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(&psbt.as_slice()) {
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
                                    .signer_process_round2(identifier, signing_session_id, psbt)
                                    .await
                                {
                                    error!(target: "consensus::authority::frost_task::SignerRound2SigningPackage", "Peer Error processing round 2 signing package {:?}", e);
                                }
                            }
                            SigningEventResponseType::CoordinatorRound2SigningPackage => {
                                let psbt_res = match bitcoin::Psbt::deserialize(&psbt.as_slice()) {
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
                                        identifier,
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

impl<EF, BF, DB, ToFrostMan, Source> std::fmt::Debug for FrostTask<EF, BF, DB, ToFrostMan, Source>
where
    ToFrostMan: ToFrostManager + Clone,
    Source: RandomSource,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrostTask").finish_non_exhaustive()
    }
}
