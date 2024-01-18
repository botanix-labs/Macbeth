use crate::{engine_util, task::BlockProductionTask};
use reth_eth_wire::NewBlock;
use reth_primitives::{Block, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, Chain, StateProviderFactory};
use reth_transaction_pool::TransactionPool;
use ruint::Uint;
use std::{sync::Arc, task::Poll};
use tracing::{error, info};

impl<Client, Pool: TransactionPool> BlockProductionTask<Client, Pool>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
    Pool: TransactionPool,
{
    pub(crate) async fn try_build_block(&mut self) {
        // Check if we are in_turn
        let is_inturn = match self.epoch_manager.poll(&self.pool).await {
            (Poll::Pending, is_inturn) => is_inturn,
            (Poll::Ready(transactions), is_inturn) => {
                info!(target: "consensus::authority",
                    "Adding to the list of transctions, {:?}, {:?}",
                    transactions, self.queued
                );
                self.queued.push_back(transactions.clone());
                let mining_pool = self.pool.clone();
                // TODO (armins) should not be removing txs from the pool before they are mined
                mining_pool.remove_transactions(
                    transactions.iter().map(|tx| tx.hash().to_owned()).collect(),
                );
                is_inturn
            }
        };

        if !is_inturn {
            info!(target: "consensus::authority", "Not in turn, skipping");
            return;
        }

        // Check if we have transactions to insert
        if self.queued.is_empty() || !is_inturn {
            info!(target: "consensus::authority", "Txs list is empty, skipping");
            // nothing to insert
            return
        }

        let transactions = self.queued.pop_front().expect("not empty");
        let txs_cloned = transactions.clone();
        let events = self.pipe_line_events.take();
        let client = self.client.clone();

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

        // Build and execute current block template
        let (new_header, bundle_state) = match storage.build_and_execute(
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
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to execute block");
                drop(storage);
                // TODO (armins) if there are sepcific txs that failed, we should not put them back
                // in the pool
                self.queued.push_front(txs_cloned);
                return
            }
        };
        drop(storage);
        // Process Botanix specific logs
        match crate::utils::process_reciepts(
            &self.bitcoin_block_source,
            &mut self.btc_server.clone(),
            &bundle_state,
            false,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                return;
            }
        }

        // Seal the block
        let block = Block {
            header: new_header.clone().unseal(),
            body: transactions,
            ommers: vec![],
            withdrawals: None,
        };
        let sealed_block = block.clone().seal_slow();
        let sealed_block_with_senders =
            SealedBlockWithSenders::new(sealed_block.clone(), senders).expect("senders are valid");

        // TODO(armins) Should this be a FCU update?
        let _res =
            engine_util::send_beacon_new_payload(sealed_block.clone(), self.to_engine.clone())
                .await
                .unwrap();

        // update canon chain for rpc
        self.client.set_canonical_head(sealed_block.header.clone());
        self.client.set_safe(sealed_block.header.clone());
        self.client.set_finalized(sealed_block.header.clone());

        let chain = Arc::new(Chain::new(vec![sealed_block_with_senders], bundle_state));

        info!(target: "consensus::authority", "sending block notification to block chain tree");
        // send block notification
        let _ = self
            .canon_state_notification
            .send(reth_provider::CanonStateNotification::Commit { new: chain });

        // lastly prune mempool
        info!(target: "consensus::authority", "Removing txs from the pool upon recevied block");
        let tx_hashes = block.body.iter().map(|tx| tx.hash().to_owned()).collect::<Vec<_>>();
        self.pool.remove_transactions(tx_hashes);
        // Notify peers
        let new_block = NewBlock { block, td: Uint::ZERO };
        let block_hash = sealed_block.hash;
        self.network_handle.announce_block(new_block, block_hash);

        self.pipe_line_events = events;
    }
}
