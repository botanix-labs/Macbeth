use reth_consensus_common::utils;
use reth_primitives::BlockHashOrNumber;
use reth_provider::{BlockReaderIdExt, CanonChainTracker, HeaderProvider};
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};

use tracing::info;

use crate::Storage;
use reth_provider::StateProviderFactory;
use std::{sync::Arc, task::Poll};

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

    /// stores whether there are pending transactions (if known)
    pub(crate) has_pending_txs: bool,
}

impl<Client: HeaderProvider> EpochManager<Client>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
{
    pub(crate) fn new(storage: Storage<Client>) -> Self {
        Self { storage, has_pending_txs: false }
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

        // get best block
        let best_block_number = match storage.client.best_block_number() {
            Ok(best_block_number) => best_block_number,
            Err(_) => return (Poll::Pending, false),
        };

        // Check if the last signer was us
        // If so nothing to do anymore until the next timeslot
        let latest_header = storage
            .client
            .header_by_hash_or_number(BlockHashOrNumber::Number(best_block_number))
            .ok()
            .flatten();

        if latest_header.is_none() {
            return (Poll::Pending, false)
        }

        let latest_header = latest_header.unwrap();

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
        let is_inturn = utils::is_inturn(authority_len, signer_index as u64);
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
