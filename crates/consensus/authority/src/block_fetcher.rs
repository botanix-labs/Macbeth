
use std::{sync::Arc, time::Duration};

use client::{FinalizeSignerRequest, Output};
use reth_consensus::Consensus;
use reth_interfaces::{
    blockchain_tree::{BlockValidationKind, BlockchainTreeEngine},
    p2p::{
        bodies::client::BodiesClient, full_block::FullBlockClient, headers::client::HeadersClient,
    },
};
use reth_primitives::{
    botanix::BotanixConsensusPackage, extra_data_header::ExtraDataHeader, header_ext::HeaderExt,
    SealedBlockWithSenders, TransactionSigned,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, Chain, StateProviderFactory};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_network::{message::NewBlockMessage, NetworkHandle};
use reth_node_api::{ConfigureEvmEnv, EngineTypes};
use reth_provider::CanonStateNotificationSender;
use tokio::sync::{
    mpsc::{error::TryRecvError, UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::{debug, error, info, warn};

use crate::{
    engine_util,
    extended_client::BtcServerExtendedClient,
    utils::{get_recent_block_height_or_zero, get_witness_data_from_psbt, is_active_sync_in_progress},
    AuthorityConsensus, Storage,
};

pub struct BlockFetcherTask<Client, EvmConfig, Engine: EngineTypes, NetworkClient> {
    /// Authority consensus
    consensus: AuthorityConsensus,
    /// Channel to recieve new blocks
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    /// Channel to send new blocks to the engine
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    /// Used to notify consumers of new blocks
    canon_state_notification: CanonStateNotificationSender,
    /// Btc Server client
    btc_server: Option<BtcServerExtendedClient>,
    /// Consensus cache
    storage: Storage<Client>,
    /// Recent bitcoin header
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// The type that defines how to configure the EVM.
    evm_config: EvmConfig,
    /// Bitcoin network
    btc_network: bitcoin::Network,
    /// Network Client, used to create [FullBlockClient]
    network_client: NetworkClient,
    /// Network Handle, used to create [FullBlockClient]
    network_handle: NetworkHandle,
}

impl<Client, EvmConfig, Engine, NetworkClient>
    BlockFetcherTask<Client, EvmConfig, Engine, NetworkClient>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    Engine: EngineTypes + 'static,
    EvmConfig:
        ConfigureEvmEnv + Clone + Unpin + Send + Sync + 'static + reth_node_api::ConfigureEvm,
    NetworkClient: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    pub(crate) fn new(
        consensus: AuthorityConsensus,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: Option<BtcServerExtendedClient>,
        storage: Storage<Client>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        evm_config: EvmConfig,
        btc_network: bitcoin::Network,
        network_client: NetworkClient,
        network_handle: NetworkHandle,
    ) -> Self {
        Self {
            consensus,
            block_import_rx,
            to_engine,
            canon_state_notification,
            btc_server,
            storage,
            bitcoin_block_header,
            evm_config,
            btc_network,
            network_client,
            network_handle,
        }
    }

    pub async fn start_task(&mut self) {
        // only a federation node has a btc_server
        let is_fed_node = self.btc_server.is_some();
        let consensus: Arc<dyn Consensus> = Arc::new(self.consensus.clone());
        let full_block_client = FullBlockClient::new(self.network_client.clone(), consensus);

        loop {
            // ensure the node is not syncing
            if is_active_sync_in_progress(&self.network_handle) {
                warn!(target: "consensus::authority", "Node is still syncing, block fetcher task is awaiting fully synced status ...");
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }

            let new_block = match self.block_import_rx.try_recv() {
                Ok(b) => b,
                Err(error) => match error {
                    TryRecvError::Empty => {
                        debug!(target: "consensus::authority", "No new blocks from peers");
                        continue;
                    }
                    TryRecvError::Disconnected => {
                        debug!(target: "consensus::authority", "Block import channel disconnected");
                        continue;
                    }
                },
            };

            let block = new_block.block.block.clone();
            let storage = self.storage.read().await;
            info!(target: "consensus::authority", "Recieved new block from peer {:?}", block.header.hash_slow());
            let best_hash = storage.get_best_block_and_hash().expect("best block exists").1;
            if block.header.hash_slow() == best_hash {
                warn!(target: "consensus::authority", "Recieved block is already in the chain");
                continue;
            }
            drop(storage);
            // Seal the block
            let sealed_block = block.clone().seal_slow();

            let recent_bitcoin_block_header = *self.bitcoin_block_header.read().await;
            let mut botanix_consensus_pkg = None;
            let mut storage = self.storage.write().await;
            if is_fed_node {
                if storage.aggregate_public_key.is_none() {
                    warn!(target: "consensus::authority", "Do not have aggregate public key in memory, skipping block import");
                    continue;
                } else {
                    botanix_consensus_pkg = Some(BotanixConsensusPackage {
                        recent_header: recent_bitcoin_block_header.expect("recent header is some"),
                        aggregate_public_key: storage
                            .aggregate_public_key
                            .clone()
                            .expect("aggregate pk is some"),
                        btc_network: self.btc_network,
                    });
                }
            }

            // Notify the engine of the new block
            let _payload_status = match engine_util::send_beacon_new_payload(
                sealed_block.clone(),
                self.to_engine.clone(),
                botanix_consensus_pkg.clone(),
            )
            .await
            {
                Ok(payload) => payload,
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Block import failed to send new payload to engine");
                    continue;
                }
            };

            let recent_bitcoin_block_height =
                get_recent_block_height_or_zero(recent_bitcoin_block_header);
            if recent_bitcoin_block_height == 0 {
                error!(target: "consensus::authority", "Failed to get recent bitcoin block height");
                continue;
            }

            if recent_bitcoin_block_header.is_none() {
                warn!(target: "consensus::authority", "Do not have recent block header in memory, skipping block import");
                continue;
            }

            match storage.execute_imported_block(
                &self.consensus,
                sealed_block.clone(),
                botanix_consensus_pkg.clone(),
                self.evm_config.clone(),
            ) {
                Ok(bundle_state) => {
                    let senders =
                        TransactionSigned::recover_signers(&block.body, block.body.len()).unwrap();
                    let sealed_block_with_senders =
                        SealedBlockWithSenders::new(sealed_block.clone(), senders)
                            .expect("senders are valid");
                    // Process Botanix specific logs
                    // get pegouts if btc_server is available
                    // only federation nodes will have btc_server
                    let mut pegouts = match self.btc_server.as_ref() {
                        Some(btc_server) => {
                            let pegouts = match crate::utils::process_receipts(
                                &mut btc_server.clone(),
                                &bundle_state,
                                recent_bitcoin_block_height,
                                self.btc_network,
                                self.consensus.chain_spec.parent_confirmation_depth,
                            )
                            .await
                            {
                                Ok(pegouts) => pegouts,
                                Err(e) => {
                                    error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                                    continue;
                                }
                            };

                            pegouts
                        }
                        None => vec![],
                    };

                    // Validate utxo commitment
                    let header = sealed_block.header.clone();
                    if is_fed_node {
                        let utxo_commitment: [u8; 32] =
                        match self.btc_server.clone().expect("btc_server exists").get_utxo_merkle_root(client::Empty {}).await {
                            Ok(utxo_commitment) => utxo_commitment,
                            Err(e) => {
                                error!(target: "consensus::authority", ?e, "Failed to get utxo commitment");
                                continue;
                            }
                        }
                        .merkle_root
                        .try_into()
                        .expect("valid UTXO commitment");
                        info!(target: "consensus::authority", "UTXO commitment: {:?}", utxo_commitment);
                        let edh = header.deserialize_extra_data_header().expect("valid extra data");
                        if edh.utxo_commitment != utxo_commitment {
                            error!(target: "consensus::authority", "UTXO commitment mismatch");
                            continue;
                        }
                    }

                    let (best_block, _best_hash) =
                        storage.get_best_block_and_hash().expect("best block exists");
                    if header.is_poa_epoch() && is_fed_node {
                        // get the pegouts from during the epoch
                        let past_pegouts = crate::utils::epoch_pegouts(best_block, &storage.client, self.btc_network,).await.map_err(|e| {
                            error!(target: "consensus::authority", ?e, "Failed to get epoch pegouts");
                            e
                        }).unwrap();
                        pegouts.extend(past_pegouts);
                        let extra_data = ExtraDataHeader::deserialize(
                            &mut header.extra_data.clone().to_vec().as_slice(),
                        )
                        .expect("extra data is valid");
                        // finalizing signing if there are pegouts
                        // at this point this singer or others have provided partial signatures and
                        // completed the signing session
                        if let Some(witness) = extra_data.witness_data {
                            let wit = witness
                                .iter()
                                .map(|witness| witness.to_vec()[0].clone())
                                .collect::<Vec<Vec<u8>>>();
                            let outputs = pegouts
                                .iter()
                                .map(|pegout| Output {
                                    address: pegout.destination.to_string(),
                                    value: pegout.amount.to_sat(),
                                })
                                .collect();
                            let res = self
                                .btc_server
                                .clone()
                                .expect("btc_server exists")
                                .signer_finalize(FinalizeSignerRequest { witness: wit, outputs })
                                .await;

                            if let Err(e) = res {
                                error!(target: "consensus::authority", ?e, "Failed to finalize signer");
                                continue;
                            }
                            info!(target: "consensus::authority", "Witness data valid and finalized");
                        } else {
                            // if there are pegouts but no witness data in the EDH, fail consensus
                            if !pegouts.is_empty() {
                                error!(target: "consensus::authority", "Pegouts exist but no witness data in the EDH");
                                continue;
                            }
                        }
                    }

                    // Need to decide if we accepting a forked block or not
                    // There is a garuntee a quorum of signers will not sign an invalid fork
                    let tip = storage.client.best_block_number().expect("best block exists");
                    let best_block = storage
                        .client
                        .block_by_number(tip)
                        .expect("best block exists")
                        .expect("best block exists");
                    if best_block.header.hash_slow() != header.parent_hash {
                        warn!(target: "consensus::authority", "Recieved block is not a direct child of the best block");
                        // need to retrieve this missing block from a peer
                        let missing_block =
                            full_block_client.get_full_block(header.parent_hash.clone()).await;
                        if let Err(e) = storage.client.insert_block_without_senders(
                            missing_block.clone(),
                            BlockValidationKind::Exhaustive,
                            botanix_consensus_pkg,
                        ) {
                            error!(target: "consensus::authority", ?e, "Failed to insert forked block");
                            continue;
                        }
                        storage.client.set_canonical_head(missing_block.header.clone());
                        storage.client.set_safe(missing_block.header.clone());
                        storage.client.set_finalized(missing_block.header.clone());
                        if let Err(e) = engine_util::send_fork_choice_update_payload(
                            sealed_block.clone().hash(),
                            self.to_engine.clone(),
                        )
                        .await
                        {
                            error!(target: "consensus::authority", ?e, "Failed to send fork choice update on forked block");
                            continue;
                        }
                    }

                    // Notify engine api about new FCU
                    engine_util::send_fork_choice_update_payload(
                        sealed_block.clone().hash(),
                        self.to_engine.clone(),
                    )
                    .await
                    // TODO remove unwrap
                    .unwrap();

                    // update canon chain for rpc
                    storage.client.set_canonical_head(header);
                    storage.client.set_safe(sealed_block.header.clone());
                    storage.client.set_finalized(sealed_block.header.clone());
                    drop(storage);

                    // TODO(armins) trie updates here are non. is that correct?
                    let chain =
                        Arc::new(Chain::new(vec![sealed_block_with_senders], bundle_state, None));

                    info!(target: "consensus::authority", "sending block notification to block chain tree");
                    // send block notification
                    let _ = self
                        .canon_state_notification
                        .send(reth_provider::CanonStateNotification::Commit { new: chain });
                }
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Failed to exectute block recieved by peer");
                }
            }
        }
    }
}
