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
    proposal_interval: Interval,

    random_delay: Option<Pin<Box<Sleep>>>,

    /// The block number of the current block
    pub block_number: u64,

    /// Number of signers in the current epoch
    pub signer_count: u32,

    /// Zero-based index of the block signer in the sorted list of current authorized signers.
    pub signer_index: usize,

    /// Number of consecutive blocks of which a signer can only sign 1
    pub signer_limit: u32,
}

impl EpochManager {
    pub fn new(storage: Storage, network: NetworkHandle) -> Self {
        // get the header for the best known block
        let header = storage.headers.get(&storage.best_block);

        if header.extra_data.len() > 97 {
            let signature = &header.extra_data[header.extra_data.len() - 65..];
            let signer_slice = &header.extra_data[32..header.extradata.len() - 65];
            let extra = &header.extra_data[0..32];

            self.storage = storage;

            self.signer_count = &signer_slice.len() / 32;

            self.signer_limit = (self.signer_count / 2) + 1;

            self.signer_index = &signer_slice.chunks().position(|&x| x == public_key);

            self.block_number = storage.best_block;
        } else {
            // TODO: handle when there are no signers
            // NOTE: this shouldn't be a case unless genesis config is setup incorrectly
            todo!()
        }
    }

    // interval to lock production for BLOCK_PERIOD since last timestamp
    fn set_interval() {
        let best_header = self.storage.headers.get(&self.storage.best_block);
        let timestamp = best_header.timestamp;
        self.proposal_interval = tokio::time::interval_at(timestamp, constants::BLOCK_PERIOD);
    }

    pub(crate) fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Pool<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        let is_inturn = (self.block_number % self.signer_count == self.signer_index);
        match self.random_delay {
            Some(ref mut delay) => {
                if delay.poll(cx).is_ready() {
                    self.random_delay = None;
                    // drain pool
                    return Poll::Ready(pool.best_transactions().collect())
                }
                Poll::Pending
            }
            None => {
                if !self.proposal_interval.poll_tick(cx).is_ready() {
                    return Poll::Pending
                } else {
                    if is_inturn {
                        return Poll::Ready(pool.best_transactions().collect())
                    } else {
                        let duration = Duration::from_secs(6);
                        self.random_delay = Some(tokio::time::sleep_until(duration));
                        Poll::Pending
                    }
                }
            }
        }
        // inturn
    }
}
