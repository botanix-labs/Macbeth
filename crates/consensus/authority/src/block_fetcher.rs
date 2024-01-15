use crate::{task::BlockProductionTask, AuthorityConsensus};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_consensus_common::utils;
use reth_primitives::{SealedBlockWithSenders, TransactionSigned};
use reth_provider::{CanonChainTracker, StateProviderFactory};
use reth_revm::{database::StateProviderDatabase, processor::EVMProcessor, State};
use reth_rpc_types::engine::PayloadStatusEnum;
use reth_transaction_pool::TransactionPool;

use tokio::sync::{mpsc::error::TryRecvError, oneshot};
use tracing::{debug, error, info};

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    pub(crate) async fn try_fetch_block(&mut self) {
        match self.block_import_rx.try_recv() {
            Ok(new_block) => {
                // Recieved a new block from a peer. Block import has ran consensus validation
                // against this block Update internal cache and notify the
                // engine
                loop {
                    let block = new_block.block.block.clone();
                    info!(target: "consensus::authority", ?block, "Recieved new block from peer");

                    // extract signer pub key
                    let signer = utils::recovery_authority(&block.header).expect("valid signer");

                    let storage_inner = self.epoch_manager.storage.inner.read().await;
                    let authorities = storage_inner.authorities.clone();
                    drop(storage_inner);
                    let signer_index =
                        authorities.iter().position(|pk| *pk == signer).expect("valid signer");
                    match AuthorityConsensus::validate_inturn(
                        block.header.timestamp,
                        authorities.len() as u64,
                        signer_index as u64,
                    ) {
                        Ok(_) => {}
                        Err(err) => {
                            error!(target: "consensus::authority", ?err, "Block import failed in turn check");
                            continue
                        }
                    }

                    // send the new update to the engine, this will trigger the engine
                    // to download and execute the block we just inserted
                    let (tx, rx) = oneshot::channel();
                    let sealed_block = block.clone().seal_slow();
                    let _ = self.to_engine.send(BeaconEngineMessage::NewPayload {
                        payload: sealed_block.clone().into(),
                        cancun_fields: None,
                        tx,
                    });

                    match rx.await.unwrap() {
                        Ok(payload_status) => {
                            match payload_status.status {
                                PayloadStatusEnum::Accepted | PayloadStatusEnum::Valid => {
                                    // remove the tx which are now confirmed
                                    info!("Removing txs from the pool upon recevied block");
                                    let tx_hashes = block
                                        .body
                                        .iter()
                                        .map(|tx| tx.hash().to_owned())
                                        .collect::<Vec<_>>();
                                    self.pool.remove_transactions(tx_hashes);

                                    let senders = TransactionSigned::recover_signers(
                                        &block.body,
                                        block.body.len(),
                                    )
                                    .unwrap();

                                    let db = State::builder()
                                        .with_database_boxed(Box::new(StateProviderDatabase::new(
                                            self.client.latest().unwrap(),
                                        )))
                                        .with_bundle_update()
                                        .build();
                                    let mut executor =
                                        EVMProcessor::new_with_state(self.chain_spec.clone(), db);
                                    let mut storage = self.storage.write().await;
                                    let recent_bitcoin_block_header =
                                        self.bitcoin_block_header.read().await.clone();

                                    match storage.execute(
                                        &block,
                                        &mut executor,
                                        senders.clone(),
                                        recent_bitcoin_block_header,
                                    ) {
                                        Ok((bundle_state, _gas_used)) => {
                                            drop(storage);
                                            let sealed_block_with_senders =
                                                SealedBlockWithSenders::new(sealed_block, senders)
                                                    .expect("senders are valid");
                                            self.persist_new_block(
                                                sealed_block_with_senders.clone(),
                                                bundle_state,
                                            )
                                            .await;
                                        }
                                        Err(err) => {
                                            error!(target: "consensus::authority", ?err, "Failed to exectute block recieved by peer");
                                        }
                                    }
                                    break
                                }
                                PayloadStatusEnum::Invalid { validation_error } => {
                                    error!(target: "consensus::authority", ?validation_error, "Authority fork new payload returned invalid response");
                                    break
                                }
                                PayloadStatusEnum::Syncing => {
                                    debug!(target: "consensus::authority", ?payload_status, "Authority fork new payload returned SYNCING, waiting for VALID");
                                    // wait for the next fork choice update
                                    continue
                                }
                            }
                        }
                        Err(status_err) => {
                            error!(target: "consensus::authority", ?status_err, "Authority fork new payload failed");
                            return ()
                        }
                    }
                }
            }
            Err(error) => match error {
                TryRecvError::Empty => {
                    debug!(target: "consensus::authority", "No new blocks from peers");
                }
                TryRecvError::Disconnected => {
                    error!(target: "consensus::authority", "Block import channel disconnected");
                    return ()
                }
            },
        }
    }
}
