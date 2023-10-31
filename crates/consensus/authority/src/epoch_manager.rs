use crate::storage::Storage;
use std::{
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

#[derive(Debug)]
pub struct EpochManager {
    /// access to storage to fetch headers
    storage: Storage,

    /// pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`
    proposal: Interval,
}

impl EpochManager {
    pub fn new(storage: Storage) -> Self {
        let timestamp = storage.headers.get(&storage.best_block).timestamp;
        let proposal = Self::calculate_optimal_time_naively(timestamp);
        Self { storage, proposal }
    }

    fn calculate_optimal_time_naively(timestamp: u64) {
        self.proposal = tokio::time::interval_at(timestamp, constants::BLOCK_PERIOD);
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
