use reth_primitives::constants::eip225::BLOCK_PERIOD;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use tokio::time::{Instant, Interval};
use tracing::{error, info};

use crate::{Storage, AuthorityConsensus};
use std::{
    sync::Arc,
    task::Poll,
    time::Duration,
};

#[derive(Debug)]
/// Manages the block production epochs
/// Signing time is the parent timestamp + BLOCK_PERIOD
/// If the signer is inturn then we broadcast the block
///
/// Blocks will be rejected by consensus if
/// 1. The signer is not in the federation
/// 2. signer is not inturn
/// 3. block fails common consensus checks
pub(crate) struct EpochManager {
    /// Access to storage to fetch headers.
    // TODO (armins) this should be protected by an Arc.
    pub(crate) storage: Storage,

    /// Pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`.
    pub(crate) proposal_interval: Interval,

    /// stores whether there are pending transactions (if known)
    pub(crate) has_pending_txs: bool,
}

impl EpochManager {
    pub(crate) fn naive_inverval(storage: Storage) -> Self {
        let start = Instant::now() + Duration::from_millis(BLOCK_PERIOD);
        let proposal_interval =
            tokio::time::interval_at(start, Duration::from_millis(BLOCK_PERIOD));
        Self { storage, proposal_interval, has_pending_txs: false }
    }

    pub(crate) async fn poll<Pool>(
        &mut self,
        pool: &Pool,
    ) -> Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        let storage = self.storage.inner.read().await;
        let signer_index = storage.signer_index;
        println!("signer_index: {}", signer_index);

        let authority_len = storage.authorities.len() as u64;
        let signer_index = storage.signer_index as u64;
        drop(storage);

        let is_inturn = AuthorityConsensus::is_inturn(authority_len, signer_index);
        println!("is_inturn: {}", is_inturn);

        self.proposal_interval.tick().await;
        if is_inturn {
            let transactions = pool.best_transactions().collect::<Vec<_>>();
            info!("Miner processing txs {:?}", transactions);
            // there are pending transactions if we didn't drain the pool
            self.has_pending_txs = !transactions.is_empty();

            if transactions.is_empty() {
                return Poll::Pending
            }

            // drain the pool
            Poll::Ready(transactions)
        } else {
            // TODO remove this later
            Poll::Pending
        }

        // NOTE: verify if network can/should be handled here or in the main task
        // TODO: check network handle for gossiped block
        // TODO: set gossiped block header in storage or...
        // TODO: if `None` do the following
    }
}
