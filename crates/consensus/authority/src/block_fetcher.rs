use crate::{engine_util, task::BlockProductionTask, AuthorityConsensus};

use reth_consensus_common::utils;
use reth_primitives::{SealedBlockWithSenders, TransactionSigned};
use reth_provider::{CanonChainTracker, StateProviderFactory, BlockReaderIdExt};
use reth_revm::{database::StateProviderDatabase, processor::EVMProcessor, State};

use reth_transaction_pool::TransactionPool;

use tokio::sync::{mpsc::error::TryRecvError};
use tracing::{debug, error, info};

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    pub(crate) async fn try_fetch_block(&mut self) {
        let new_block = match self.block_import_rx.try_recv() {
            Ok(b) => b,
            Err(error) => match error {
                TryRecvError::Empty => {
                    debug!(target: "consensus::authority", "No new blocks from peers");
                    return;
                }
                TryRecvError::Disconnected => {
                    error!(target: "consensus::authority", "Block import channel disconnected");
                    return;
                }
            },
        };

        let block = new_block.block.block.clone();
        info!(target: "consensus::authority", ?block, "Recieved new block from peer");

        // extract signer pub key
        let signer = utils::recovery_authority(&block.header).expect("valid signer");

        let authorities = self.epoch_manager.storage.inner.read().await.authorities.clone();
        let signer_index = authorities.iter().position(|pk| *pk == signer).expect("valid signer");
        // TODO(armins) this should be a consensus check not standalone in the block fetcher
        match AuthorityConsensus::validate_inturn(
            block.header.timestamp,
            authorities.len() as u64,
            signer_index as u64,
        ) {
            Ok(_) => {}
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Block import failed in turn check");
                return
            }
        }

        // TODO(armins) this should be a consensus check not standalone in the block fetcher
        // validate beneficiary is within the authorities list
        match utils::validate_poa_block_beneficiary(&block.header, &authorities) {
            Ok(_) => {}
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Block beneficiary not found in authorities list");
                return
            }
        }
        // Seal the block
        let sealed_block = block.clone().seal_slow();
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
                return
            }
        };

        let senders = TransactionSigned::recover_signers(&block.body, block.body.len()).unwrap();

        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(
                self.client.latest().unwrap(),
            )))
            .with_bundle_update()
            .build();
        let mut executor = EVMProcessor::new_with_state(self.chain_spec.clone(), db);
        let mut storage = self.storage.write().await;
        let recent_bitcoin_block_header = self.bitcoin_block_header.read().await.clone();

        match storage.execute(&block, &mut executor, senders.clone(), recent_bitcoin_block_header) {
            Ok((bundle_state, _gas_used)) => {
                drop(storage);
                let sealed_block_with_senders =
                    SealedBlockWithSenders::new(sealed_block, senders).expect("senders are valid");
                // Process Botanix specific logs
                match self.process_reciepts(&bundle_state, false).await {
                    Ok(_) => {}
                    Err(e) => {
                        error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                        return
                    }
                }
                // Persist new block to storage
                match self.persist_new_block(sealed_block_with_senders.clone(), bundle_state).await
                {
                    Ok(_) => {}
                    Err(err) => {
                        error!(target: "consensus::authority", ?err, "Failed to persist new block");
                    }
                }

                // lastly prune mempool
                info!(target: "consensus::authority", "Removing txs from the pool upon recevied block");
                let tx_hashes =
                    block.body.iter().map(|tx| tx.hash().to_owned()).collect::<Vec<_>>();
                self.pool.remove_transactions(tx_hashes);
            }
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Failed to exectute block recieved by peer");
            }
        }
    }
}
