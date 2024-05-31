use crate::{
    engine_util::{self, SendNewPayloadError},
    extended_client::BtcServerExtendedClient,
    utils::{get_recent_block_height_or_zero, is_testnet},
};

use client::{FinalizeSignerRequest, Output};
use reth_botanix_lib::extra_data_header::{ExtraDataHeader, HeaderExt};
use reth_interfaces::blockchain_tree::BlockchainTreeEngine;
use reth_primitives::{
    botanix::BotanixConsensusPackage, Block, SealedBlockWithSenders, TransactionSigned,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, CanonChainTracker, Chain, StateProviderFactory,
};

use crate::Storage;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_btc_wallet::bitcoind::BitcoindClient;
use reth_network::message::NewBlockMessage;
use reth_node_api::{ConfigureEvmEnv, EngineTypes};
use reth_primitives::ChainSpec;
use reth_provider::CanonStateNotificationSender;

use std::sync::Arc;
use tokio::sync::{
    mpsc::{error::TryRecvError, UnboundedReceiver, UnboundedSender},
    RwLock,
};

use tracing::{debug, error, info, warn};

pub struct BlockFetcherTask<Client, EvmConfig, Engine: EngineTypes> {
    chain_spec: Arc<ChainSpec>,
    block_import_rx: UnboundedReceiver<NewBlockMessage>,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    /// Used to notify consumers of new blocks
    canon_state_notification: CanonStateNotificationSender,
    /// Btc Server client
    btc_server: Option<BtcServerExtendedClient>,
    /// bitcoin block source
    bitcoind_client: BitcoindClient,
    /// Consensus cache
    storage: Storage<Client>,
    /// Recent bitcoin header
    bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
    /// The type that defines how to configure the EVM.
    evm_config: EvmConfig,
    /// Bitcoin network
    btc_network: bitcoin::Network,
}

impl<Client, EvmConfig, Engine> BlockFetcherTask<Client, EvmConfig, Engine>
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
{
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        block_import_rx: UnboundedReceiver<NewBlockMessage>,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        canon_state_notification: CanonStateNotificationSender,
        btc_server: Option<BtcServerExtendedClient>,
        bitcoind_client: BitcoindClient,
        storage: Storage<Client>,
        bitcoin_block_header: Arc<RwLock<Option<(bitcoin::block::Header, u32)>>>,
        evm_config: EvmConfig,
        btc_network: bitcoin::Network,
    ) -> Self {
        Self {
            chain_spec,
            block_import_rx,
            to_engine,
            canon_state_notification,
            btc_server,
            bitcoind_client,
            storage,
            bitcoin_block_header,
            evm_config,
            btc_network,
        }
    }

    async fn send_block_to_beacon(&self, block: &Block) -> Result<(), SendNewPayloadError> {
        // Seal the block
        let sealed_block = block.clone().seal_slow();
        // Notify the engine of the new block
        engine_util::send_beacon_new_payload(sealed_block.clone(), self.to_engine.clone()).await?;
        Ok(())
    }

    pub async fn start_task(&mut self) {
        // only a federation node has a btc_server
        let is_fed_node = self.btc_server.is_some();

        loop {
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

            let block: reth_primitives::Block = new_block.block.block.clone();
            let storage = self.storage.read().await;
            info!(target: "consensus::authority", ?block, "Recieved new block from peer");
            let (best_block, best_hash) =
                storage.get_best_block_and_hash().expect("best block exists");
            if block.header.hash_slow() == best_hash {
                warn!(target: "consensus::authority", "Recieved block is already in the chain");
                continue;
            }

            // check for missing blocks from the tip
            let are_blocks_missing = !block.parent_hash.eq(&best_hash);
            let mut blocks_to_sync: Vec<Block> = vec![];
            if are_blocks_missing {
                warn!(target: "consensus::authority", "Block fetcher is missing blocks. Catching up...");
                let mut block_num_hash = block.parent_num_hash();
                loop {
                    if let Some(block) =
                        BlockReader::block_by_hash(&storage.client, block_num_hash.hash)
                            .ok()
                            .flatten()
                    {
                        block_num_hash = block.parent_num_hash();
                        blocks_to_sync.push(block);
                        if block_num_hash.number == best_block {
                            break;
                        }
                    }
                }
            }
            blocks_to_sync.reverse();
            drop(storage);

            // send the blocks to the beacon engine
            for block_to_sync in blocks_to_sync.iter() {
                if let Err(err) = self.send_block_to_beacon(&block_to_sync).await {
                    error!(target: "consensus::authority", ?err, "Block import failed to send new payload to engine");
                    continue;
                }
            }

            let recent_bitcoin_block_header = *self.bitcoin_block_header.read().await;
            let recent_bitcoin_block_height =
                get_recent_block_height_or_zero(recent_bitcoin_block_header);
            if recent_bitcoin_block_height == 0 {
                error!(target: "consensus::authority", "Failed to get recent bitcoin block height");
                continue;
            }
            let storage = self.storage.write().await;

            if recent_bitcoin_block_header.is_none() {
                warn!(target: "consensus::authority", "Do not have recent block header in memory, skipping block import");
                continue;
            }

            let mut botanix_consensus_pkg = None;
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
            drop(storage);

            // execute the entirety of blocks
            for block_to_sync in blocks_to_sync.iter() {
                let sealed_block = block.clone().seal_slow();
                let botanix_consensus_pkg = botanix_consensus_pkg.clone();

                let mut storage = self.storage.write().await;

                match storage.execute_imported_block(
                    self.chain_spec.clone(),
                    sealed_block.clone(),
                    botanix_consensus_pkg,
                    self.evm_config.clone(),
                ) {
                    Ok(bundle_state) => {
                        let senders = TransactionSigned::recover_signers(
                            &block_to_sync.body,
                            block_to_sync.body.len(),
                        )
                        .unwrap();
                        let sealed_block_with_senders =
                            SealedBlockWithSenders::new(sealed_block.clone(), senders)
                                .expect("senders are valid");
                        // Process Botanix specific logs
                        let is_testnet = is_testnet(self.chain_spec.chain().id());
                        // get pegouts if btc_server is available
                        // only federation nodes will have btc_server
                        let mut pegouts = match self.btc_server.as_ref() {
                            Some(btc_server) => {
                                let pegouts = match crate::utils::process_receipts(
                                    &mut btc_server.clone(),
                                    &bundle_state,
                                    recent_bitcoin_block_height,
                                    is_testnet,
                                    self.btc_network,
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
                            let edh =
                                header.deserialize_extra_data_header().expect("valid extra data");
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
                            // at this point this singer or others have provided partial signatures
                            // and completed the signing session
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
                                    .signer_finalize(FinalizeSignerRequest {
                                        witness: wit,
                                        outputs,
                                    })
                                    .await;

                                if let Err(e) = res {
                                    error!(target: "consensus::authority", ?e, "Failed to finalize signer");
                                    continue;
                                }
                                info!(target: "consensus::authority", "Witness data valid and finalized");
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
                        // TODO do we need to insert the block here?
                        storage.client.set_canonical_head(header);
                        storage.client.set_safe(sealed_block.header.clone());
                        storage.client.set_finalized(sealed_block.header.clone());
                        drop(storage);

                        // TODO(armins) trie updates here are non. is that correct?
                        let chain = Arc::new(Chain::new(
                            vec![sealed_block_with_senders],
                            bundle_state,
                            None,
                        ));

                        info!(target: "consensus::authority", "sending block notification to block chain tree");
                        // send block notification
                        let _ = self
                            .canon_state_notification
                            .send(reth_provider::CanonStateNotification::Commit { new: chain });
                    }
                    Err(err) => {
                        error!(target: "consensus::authority", ?err, "Failed to exectute block recieved by peer");
                        drop(storage);
                    }
                }
            }
        }
    }
}
