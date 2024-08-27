use std::{sync::Arc, time::Duration};

use bitcoin::hashes::{sha256, Hash};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_botanix_lib::mint_validation::{try_parse_burn_event, try_parse_mint_event};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_interfaces::{
    blockchain_tree::BlockchainTreeEngine,
    p2p::{
        bodies::client::BodiesClient, full_block::FullBlockClient, headers::client::HeadersClient,
    },
};
use reth_network::{frost::manager::ToFrostManager, message::NewBlockMessage, NetworkHandle};
use reth_node_api::EngineTypes;
use reth_node_ethereum::EthEngineTypes;
use reth_primitives::{header_ext::HeaderExt, SealedBlockWithSenders, TransactionSigned};
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, CanonStateNotificationSender, Chain, ExecutorFactory,
    StateProviderFactory,
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};
use tracing::{error, info, warn};

use crate::{
    engine_util,
    excecution_utils::authority_execution_utils::execute_imported_block,
    utils::{bloom_contains_pegin, call_notify_pegin, is_active_sync_in_progress},
    utxo_sync::{UTXOSync, UTXOSyncEngine},
    AuthorityConsensus, Storage,
};
use btcserverlib::extended_client::BtcServerExtendedClient;
use client::{FinalizeSignerRequest, Output};

pub struct BlockFetcherTask<EF, BF, DB, NetworkClient, ToFrostMan> {
    /// Authority consensus
    consensus: AuthorityConsensus,
    /// Channel to recieve new blocks
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    /// Channel to send new blocks to the engine
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    /// Used to notify consumers of new blocks
    canon_state_notification: CanonStateNotificationSender,
    /// Btc Server client
    btc_server: Option<BtcServerExtendedClient>,
    /// Consensus cache
    storage: Storage<EF, BF, DB>,
    /// Recent finalize bitcoin block checkpoint.
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// Network Client, used to create [FullBlockClient]
    network_client: NetworkClient,
    /// Network Handle, used to create [FullBlockClient]
    network_handle: NetworkHandle,
    /// Utxo set sync controller
    /// Rpc servers will not have a utxo sync engine
    utxo_sync: Option<UTXOSyncEngine<EF, BF, DB, ToFrostMan>>,
}

impl<EF, BF, DB, NetworkClient, ToFrostMan> BlockFetcherTask<EF, BF, DB, NetworkClient, ToFrostMan>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    BF: BitcoindFactory + Clone + 'static,
    EF: ExecutorFactory + Clone + 'static,
    NetworkClient: HeadersClient + BodiesClient + Clone + Unpin + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        consensus: AuthorityConsensus,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: Option<BtcServerExtendedClient>,
        storage: Storage<EF, BF, DB>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        network_client: NetworkClient,
        network_handle: NetworkHandle,
        utxo_sync: Option<UTXOSyncEngine<EF, BF, DB, ToFrostMan>>,
    ) -> Self {
        Self {
            consensus,
            block_import_rx,
            to_engine,
            canon_state_notification,
            btc_server,
            storage,
            bitcoin_block_header,
            network_client,
            network_handle,
            utxo_sync,
        }
    }

    pub async fn start_task(&mut self) {
        // only a federation node has a btc_server
        let is_fed_node = self.btc_server.is_some();
        let consensus = Arc::new(self.consensus.clone());

        loop {
            // ensure the node is not syncing
            if is_active_sync_in_progress(&self.network_handle) {
                warn!(target: "consensus::authority", "Node is still syncing, block fetcher task is awaiting fully synced status ...");
                tokio::time::sleep(Duration::from_millis(500)).await;
                return;
            }

            // Rpc servers will not have a utxo sync engine
            if let Some(utxo_sync) = &self.utxo_sync {
                if let Err(utxo_sync_err) = utxo_sync.sync_utxo_set().await {
                    error!(target: "consensus::authority", ?utxo_sync_err, "Failed to sync utxo set");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                };
            }

            let new_block = match self.block_import_rx.recv().await {
                Some(b) => b,
                None => {
                    info!(target: "consensus::authority",
                        "block fetcher task shutting down (channel closed)",
                    );
                    return;
                }
            };

            let block = new_block.block.block.clone();
            info!(target: "consensus::authority", "Recieved new block from peer {:?}", block.header.hash_slow());
            let guard = self.storage.inner.read().await;
            let client = guard.client.clone();
            drop(guard);

            let best_block = client.best_block_number().expect("best block number exists");
            let best_hash = client
                .block_hash(best_block)
                .expect("best block hash exists")
                .unwrap_or_else(|| {
                    panic!("best block hash is valid");
                });
            if block.header.hash_slow() == best_hash {
                warn!(target: "consensus::authority", "Recieved block is already in the chain");
                continue;
            }
            // Seal the block
            let sealed_block = block.clone().seal_slow();

            let storage = self.storage.write().await;
            if is_fed_node && storage.aggregate_public_key.is_none() {
                warn!(target: "consensus::authority", "Do not have aggregate public key in memory, skipping block import");
                continue;
            }

            let aggregate_public_key = storage.aggregate_public_key.clone();
            if is_fed_node && storage.aggregate_public_key.is_none() {
                // note: `storage.aggregate_public_key` will get populated by the dkg state machine
                warn!(target: "consensus::authority", "Do not have aggregate public key in memory, skipping block import");
                continue;
            }

            // Notify the engine of the new block
            let _payload_status = match engine_util::send_beacon_new_payload(
                sealed_block.clone(),
                self.to_engine.clone(),
            )
            .await
            {
                Ok(payload) => payload,
                Err(err) => {
                    error!(target: "consensus::authority", ?err, "Block import failed to send new payload to engine");
                    continue;
                }
            };
            // TODO Should be handling payload status here

            match execute_imported_block(
                &self.consensus,
                sealed_block.clone(),
                &client,
                &storage.executor_factory.clone(),
                aggregate_public_key.as_ref(),
                &storage.authorities,
                &storage.genesis_authorities,
            ) {
                Ok(bundle_state) => {
                    let senders =
                        TransactionSigned::recover_signers(&block.body, block.body.len()).unwrap();
                    let sealed_block_with_senders =
                        SealedBlockWithSenders::new(sealed_block.clone(), senders)
                            .expect("senders are valid");
                    let header = sealed_block.header.clone();

                    // Consensus checks were run during PBFT so don't need to validate pegouts again
                    // unless it's an epoch block to collect pegouts for psbt.
                    // We always need to process pegins to update UTXO set
                    let should_process_receipts =
                        header.is_poa_epoch() || bloom_contains_pegin(block.header.logs_bloom);
                    if is_fed_node && should_process_receipts {
                        // process pegins
                        // must be done before getting utxo commitment
                        let btc_server = self.btc_server.as_mut().expect("have btc_server");
                        let mut pegouts = Vec::new();
                        for (idx, receipts) in bundle_state.receipts().iter().enumerate() {
                            for receipt in receipts {
                                if idx == 0 && receipt.is_none() {
                                    break; // Prunning block, skip
                                }
                                if let Some(receipt) = receipt {
                                    if !receipt.success {
                                        continue;
                                    }
                                    for log in &receipt.logs {
                                        let pegin_match =
                                            try_parse_mint_event(log).expect("passed EVM check");
                                        if let Some(pegin_data) = pegin_match {
                                            info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                                            //TODO(stevenroose) should this happen here?
                                            if let Err(e) =
                                                call_notify_pegin(btc_server, &pegin_data.meta)
                                                    .await
                                            {
                                                error!(target: "consensus::authority", ?e, "failed to notify btc_server of pegin");
                                                return;
                                            }
                                            info!(target: "consensus::authority", "notifying btc server about pegin utxo");
                                        }

                                        let pegout_match =
                                            try_parse_burn_event(log, storage.btc_network)
                                                .expect("passed EVM check");
                                        if let Some(pegout) = pegout_match {
                                            pegouts.push(pegout);
                                        }
                                    }
                                }
                            }
                        }

                        // Validate utxo commitment
                        let utxo_commitment = match btc_server
                            .get_utxo_merkle_root(client::Empty {})
                            .await
                        {
                            Ok(h) => sha256::Hash::from_slice(&h.merkle_root)
                                .expect("valid utxo commitment"),
                            Err(e) => {
                                error!(target: "consensus::authority", ?e, "Failed to get utxo commitment");
                                continue;
                            }
                        };
                        info!(target: "consensus::authority", "UTXO commitment: {:?}", utxo_commitment);
                        let edh = header.deserialize_extra_data_header().expect("valid extra data");
                        if edh.utxo_commitment != utxo_commitment {
                            error!(target: "consensus::authority", "UTXO commitment mismatch");
                            continue;
                        }

                        let best_block =
                            client.best_block_number().expect("best block number exists");

                        // get the pegouts from during the epoch
                        let epoch_pegouts = match crate::utils::epoch_pegouts(
                            best_block,
                            &client,
                            storage.btc_network,
                        )
                        .await
                        {
                            Ok(pegouts) => pegouts,
                            Err(e) => {
                                error!(target: "consensus::authority", ?e, "Failed to get epoch pegouts");
                                continue;
                            }
                        };
                        pegouts.extend(epoch_pegouts);

                        // finalizing signing if there are pegouts
                        // at this point this singer or others have provided partial signatures and
                        // completed the signing session
                        if let Some(witness) = edh.witness_data {
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

                            let bitcoin_checkpoint = self
                                .bitcoin_block_header
                                .read()
                                .await
                                .expect("should have btc checkpoint")
                                .0
                                .block_hash();
                            let res = self
                                .btc_server
                                .clone()
                                .expect("btc_server exists")
                                .signer_finalize(FinalizeSignerRequest {
                                    witness: wit,
                                    outputs,
                                    checkpoint_block_hash: bitcoin_checkpoint[..].to_vec(),
                                    utxo_merkle_root: utxo_commitment[..].to_vec(),
                                })
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

                    // Notify engine api about new FCU
                    engine_util::send_fork_choice_update_payload(
                        sealed_block.clone().hash(),
                        self.to_engine.clone(),
                    )
                    .await
                    // TODO remove unwrap
                    .unwrap();

                    // update canon chain for rpc
                    client.set_canonical_head(header);
                    client.set_safe(sealed_block.header.clone());
                    client.set_finalized(sealed_block.header.clone());
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
