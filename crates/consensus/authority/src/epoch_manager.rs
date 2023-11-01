use reth_primitives::constants::eip225::BLOCK_PERIOD;
use reth_transaction_pool::{ValidPoolTransaction, TransactionPool};
use tokio::time::Interval;

use crate::constants;
use crate::storage::Storage;
use std::{
    sync::Arc,
    task::{Context, Poll},
};

#[derive(Debug)]
/// Manages the block production epochs
pub struct EpochManager {
    /// Access to storage to fetch headers.
    // TODO (armins) this should be protected by an Arc.
    storage: Storage,

    /// Pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`.
    proposal: Interval,
}

impl EpochManager {
    pub fn new(&mut self, storage: Storage) -> Self {
        let timestamp = storage.headers.get(&storage.best_block).timestamp;
        let proposal = self.calculate_optimal_time_naively(timestamp);
        Self { storage, proposal }
    }

    fn calculate_optimal_time_naively(&mut self, timestamp: u64) {
        self.proposal = tokio::time::interval_at(timestamp, BLOCK_PERIOD);
    }

    pub(crate) fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Pool<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        if self.proposal.poll_tick(cx).is_ready() {
            // drain the pool
            return Poll::Ready(pool.best_transactions().collect())
        }
        Poll::Pending
    }
}
