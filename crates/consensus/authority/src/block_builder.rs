use crate::task::BlockProductionTask;
use reth_eth_wire::NewBlock;
use reth_primitives::{Block, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::{CanonChainTracker, StateProviderFactory};
use reth_transaction_pool::TransactionPool;
use ruint::Uint;
use std::task::Poll;
use tracing::{error, info, warn};

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    pub(crate) async fn try_build_block(&mut self) {
        let is_inturn = match self.epoch_manager.poll(&self.pool).await {
            (Poll::Pending, is_inturn) => is_inturn,
            (Poll::Ready(transactions), is_inturn) => {
                info!("Adding to the list of transctions, {:?}, {:?}", transactions, self.queued);
                self.queued.push_back(transactions.clone());
                let mining_pool = self.pool.clone();
                mining_pool.remove_transactions(
                    transactions.iter().map(|tx| tx.hash().to_owned()).collect(),
                );
                is_inturn
            }
        };

        // If insert task is not none executinon of async task is on going
        if self.queued.is_empty() || !is_inturn {
            info!("Txs list is empty, skipping");
            // nothing to insert
            std::thread::sleep(std::time::Duration::from_millis(1000));
            return
        }

        // ready to queue in new insert task
        let transactions = self.queued.pop_front().expect("not empty");

        let events = self.pipe_line_events.take();

        let client = self.client.clone();

        // Create the mining future that creates a block, notifies the engine that drives
        // the pipeline

        let (transactions, senders): (Vec<_>, Vec<_>) = transactions
            .into_iter()
            .map(|tx| {
                let recovered = tx.to_recovered_transaction();
                let signer = recovered.signer();
                (recovered.into_signed(), signer)
            })
            .unzip();
        let mut storage = self.storage.write().await;
        let recent_bitcoin_block_header = self.bitcoin_block_header.read().await.clone();
        let authority_signers = storage.authorities.clone();
        // execute the new block
        match storage.build_and_execute(
            transactions.clone(),
            &client,
            self.chain_spec.clone(),
            recent_bitcoin_block_header,
            // TODO(armins) read vote in as param
            &None,
            &self.sk,
            &self.secp,
            &authority_signers,
        ) {
            Ok((new_header, bundle_state)) => {
                drop(storage);
                let reciepts_bundle = bundle_state.receipts().iter();
                for (index, reciepts) in reciepts_bundle.enumerate() {
                    for reciept in reciepts {
                        if index == 0 && reciept.is_none() {
                            // Prunning block, skip
                            break
                        }
                        if let Some(reciept) = reciept {
                            if !reciept.success {
                                continue
                            }
                            for log in &reciept.logs {
                                match self.process_botanix_log(log).await {
                                    Ok(_) => {}
                                    Err(err) => {
                                        error!(target: "consensus::authority", ?err, "Failed to process botanix log");
                                    }
                                }
                            }
                        }

                        info!("Reciept {:?}", reciept);
                    }
                }

                // seal the block
                let block = Block {
                    header: new_header.clone().unseal(),
                    body: transactions,
                    ommers: vec![],
                    withdrawals: None,
                };
                let sealed_block = block.clone().seal_slow();
                let sealed_block_with_senders =
                    SealedBlockWithSenders::new(sealed_block, senders).expect("senders are valid");
                self.persist_new_block(sealed_block_with_senders.clone(), bundle_state).await;
                // Notify peers
                let new_block = NewBlock { block, td: Uint::ZERO };
                let block_hash = sealed_block_with_senders.hash();
                self.network_handle.announce_block(new_block, block_hash);
            }
            Err(err) => {
                warn!(target: "consensus::authority", ?err, "failed to execute block")
            }
        }
        self.pipe_line_events = events;
    }
}
