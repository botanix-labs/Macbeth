use futures_util::{stream::Fuse, StreamExt};
use reth_consensus_common::utils;
use reth_primitives::{constants::eip225::BLOCK_PERIOD, TxHash};
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use tokio::{
    sync::mpsc::Receiver,
    time::{Instant, Interval},
};
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::{error, info};

use crate::{AuthorityConsensus, Storage};
use std::{sync::Arc, task::Poll, time::Duration};

#[derive(Debug)]
/// Manages the block production epochs
///
/// Blocks will be rejected by consensus if
/// 1. The signer is not in the federation
/// 2. signer is not inturn
/// 3. block fails common consensus checks
pub(crate) struct EpochManager<Client> {
    /// Access to storage to fetch headers.
    // TODO (armins) this should be protected by an Arc.
    pub(crate) storage: Storage<Client>,

    /// Pollable interval to lock nodes proposing for a min time defined by `BLOCK_PERIOD`.
    pub(crate) proposal_interval: Interval,

    /// stores whether there are pending transactions (if known)
    pub(crate) has_pending_txs: bool,
}

impl<Client> EpochManager<Client> {
    pub(crate) fn naive_inverval(storage: Storage<Client>) -> Self {
        let start = Instant::now() + Duration::from_millis(BLOCK_PERIOD);
        let proposal_interval =
            tokio::time::interval_at(start, Duration::from_millis(BLOCK_PERIOD));
        Self { storage, proposal_interval, has_pending_txs: false }
    }

    pub(crate) async fn poll<Pool>(
        &mut self,
        pool: &Pool,
    ) -> (Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>, bool)
    where
        Pool: TransactionPool,
    {
        let storage = self.storage.inner.read().await;
        let signer_index = storage.signer_index;
        let signer_pk = storage.authority;
        let authority_len = storage.authorities.len() as u64;

        // Check if the last signer was us
        // If so nothing to do anymore until the next timeslot
        let latest_header = storage.headers.get(&storage.best_block).expect("best block");
        // Skip over genesis
        if latest_header.number != 0 {
            let latest_signer = utils::recovery_authority(&latest_header).unwrap();
            let current_ts = utils::unix_timestamp();
            if let Err(_) = utils::validate_current_signer_against_last(
                (latest_signer, latest_header.timestamp / 60),
                (signer_pk, current_ts / 60),
            ) {
                return (Poll::Pending, false)
            }
        }

        drop(storage);
        let is_inturn = AuthorityConsensus::is_inturn(authority_len, signer_index as u64);
        info!("Epoch manager inturn: {}", is_inturn);

        // drain the pool
        let transactions = pool.best_transactions().collect::<Vec<_>>();
        info!("Miner processing txs {:?}", transactions);
        self.has_pending_txs = !transactions.is_empty();
        if self.has_pending_txs {
            return (Poll::Ready(transactions), is_inturn)
        } else {
            return (Poll::Pending, is_inturn)
        }
    }
}
