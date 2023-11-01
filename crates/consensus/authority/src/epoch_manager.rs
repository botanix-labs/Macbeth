use reth_primitives::constants::eip225::BLOCK_PERIOD;
use reth_transaction_pool::{ValidPoolTransaction, TransactionPool};
use tokio::time::{Interval, Instant};

use crate::Storage;
use std::{
    sync::Arc,
    task::{Context, Poll}, time::Duration,
};

#[derive(Debug)]
/// Manages the block production epochs
/// Signing time is the parent timestamp + BLOCK_PERIOD
/// If the signer is inturn then we broadcast the block
/// If the signer is not inturn then we wait random amount time
///
/// Blocks will be rejected by consensus if 
/// 1. The signer is not in the federation
/// 2. The signer has broadcasted > 1 in SIGNER_LIMIT consecutive blocks
pub(crate) struct EpochManager {
    /// Access to storage to fetch headers.
    // TODO (armins) this should be protected by an Arc.
    pub(crate) storage: Storage,

    /// Pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`.
    pub(crate) proposal_interval: Interval,
}

impl EpochManager {
    pub fn naive_inverval(storage: Storage) -> Self {
        let start = Instant::now();
        let proposal_interval = tokio::time::interval_at(start, Duration::from_millis(BLOCK_PERIOD) );
        Self { storage, proposal_interval }
    }

    pub(crate) fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        if self.proposal_interval.poll_tick(cx).is_ready() {
            println!("Time going off");
            // drain the pool
            return Poll::Ready(pool.best_transactions().collect())
        }
        Poll::Pending
    }
}
