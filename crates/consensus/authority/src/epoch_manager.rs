use reth_primitives::constants::eip225::BLOCK_PERIOD;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use tokio::time::{Instant, Interval};
use tracing::info;

use crate::Storage;
use std::{sync::Arc, task::Poll, time::Duration};

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

    /// stores whether there are pending transactions (if known)
    pub(crate) has_pending_txs: bool,

    /// Random delay to wait before proposing a block out of turn
    random_delay: Option<Interval>,
}

impl EpochManager {
    pub(crate) fn naive_inverval(storage: Storage) -> Self {
        let start = Instant::now() + Duration::from_millis(BLOCK_PERIOD);
        let proposal_interval =
            tokio::time::interval_at(start, Duration::from_millis(BLOCK_PERIOD));
        Self { storage, proposal_interval, has_pending_txs: false, random_delay: None }
    }

    pub(crate) async fn poll<Pool>(
        &mut self,
        pool: &Pool,
    ) -> Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        let random_delay = self.random_delay.take();
        let storage = self.storage.inner.read().await;
        let signer_index = storage.signer_index;
        println!("signer_index: {}", signer_index);
        let is_inturn =
            storage.best_block % (storage.authorities.len() as u64) == signer_index as u64;
        println!("is_inturn: {}", is_inturn);

        match random_delay {
            Some(mut delay) => {
                delay.tick().await;
                self.random_delay = None;
                let transactions = pool.best_transactions().collect::<Vec<_>>();
                info!("Miner processing txs {:?}", transactions);
                // there are pending transactions if we didn't drain the pool
                self.has_pending_txs = transactions.len() >= 1;

                if transactions.is_empty() {
                    return Poll::Pending
                }

                // drain the pool
                return Poll::Ready(transactions)
            }
            None => {
                self.proposal_interval.tick().await;
                if is_inturn {
                    let transactions = pool.best_transactions().collect::<Vec<_>>();
                    info!("Miner processing txs {:?}", transactions);
                    // there are pending transactions if we didn't drain the pool
                    self.has_pending_txs = transactions.len() >= 1;

                    if transactions.is_empty() {
                        return Poll::Pending
                    }

                    // drain the pool
                    return Poll::Ready(transactions)
                } else {
                    // TODO remove this later
                    return Poll::Pending;
                }
               
                // Your not in turn wait a bit then produce a block
                // NOTE: verify if network can/should be handled here or in the main task
                // TODO: check network handle for gossiped block
                // TODO: set gossiped block header in storage or...
                // TODO: if `None` do the following

                // TODO this should be random
                let duration = Duration::from_secs(6);
                self.random_delay =
                    Some(tokio::time::interval_at(Instant::now() + duration, duration));
                return Poll::Pending
            }
        }
    }
}
