use futures_util::{stream::FuturesUnordered, StreamExt};
use reth_botanix_lib::peg_contract::PegoutData;
use reth_consensus_common::utils;
use reth_primitives::{constants::eip225::EPOCH_LENGTH, BlockHashOrNumber};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, HeaderProvider};

use tracing::{error, info, warn};

use crate::{
    utils::{bloom_contains_pegout, find_epoch_start, make_tx_request_for_pegout_in_receipt},
    Storage,
};
use reth_provider::StateProviderFactory;

#[derive(Debug, thiserror::Error)]
pub(crate) enum EpochManagerError {
    #[error("Failed to fetch pegouts for an epoch")]
    FailedToFetchPegouts,
}

#[derive(Clone, Debug)]
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
}

impl<Client: HeaderProvider> EpochManager<Client>
where
    Client: BlockReaderIdExt + StateProviderFactory + CanonChainTracker + Clone + 'static,
{
    pub(crate) fn new(storage: Storage<Client>) -> Self {
        Self { storage }
    }

    /// Returns all pegouts in an epoch iterating through an inclusive block range
    ///
    /// # Arguments
    ///
    /// * `current_block` - The current block number
    ///
    /// # Returns
    ///
    /// A vector of [PegoutData] representing the pegouts in the epoch
    pub(crate) async fn epoch_pegouts(
        &self,
        best_block: u64,
    ) -> Result<Vec<PegoutData>, EpochManagerError> {
        let start_block = find_epoch_start(EPOCH_LENGTH, best_block);
        let storage = self.storage.inner.read().await;
        let mut pegouts: Vec<PegoutData> = vec![];
        for block in start_block..=best_block {
            match storage.client.block_by_number(block) {
                Ok(Some(block)) if bloom_contains_pegout(block.header.logs_bloom) => {
                    match storage
                        .client
                        .receipts_by_block(BlockHashOrNumber::Number(block.header.number))
                    {
                        Ok(Some(receipts)) => {
                            let mut futures = Vec::new();

                            for receipt in receipts {
                                let future = make_tx_request_for_pegout_in_receipt(receipt);
                                futures.push(future);
                            }

                            let mut results_stream = futures
                                .into_iter()
                                .map(tokio::spawn)
                                .collect::<FuturesUnordered<_>>();
                            while let Some(pegout) = results_stream.next().await {
                                match pegout {
                                    Ok(Some(pegout)) => pegouts.push(pegout),
                                    Ok(None) => continue,
                                    Err(e) => {
                                        error!("Error fetching pegout: {}", e);
                                        return Err(EpochManagerError::FailedToFetchPegouts);
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            info!("No receipts found for block {:?}", block);
                            continue;
                        }
                        Err(e) => {
                            error!("Error fetching receipts for block {:?}: {}", block, e);
                            return Err(EpochManagerError::FailedToFetchPegouts);
                        }
                    }
                }
                Ok(Some(_)) => {
                    info!("No pegouts found in block {}", block);
                    continue;
                }
                Ok(None) => {
                    error!("Block {} not found", block);
                    return Err(EpochManagerError::FailedToFetchPegouts);
                }
                Err(e) => {
                    error!("Error fetching block {}: {}", block, e);
                    return Err(EpochManagerError::FailedToFetchPegouts);
                }
            }
        }

        Ok(pegouts)
    }

    pub(crate) async fn poll(&mut self) -> bool {
        let storage = self.storage.inner.read().await;
        let signer_index = storage.signer_index;
        let signer_pk = storage.authority;
        let authority_len = storage.authorities.len() as u64;

        // get best block
        let best_block_number = match storage.client.best_block_number() {
            Ok(best_block_number) => best_block_number,
            Err(_) => {
                drop(storage);
                return false;
            }
        };

        // Check if the last signer was us
        // If so nothing to do anymore until the next timeslot
        let latest_header = storage
            .client
            .header_by_hash_or_number(BlockHashOrNumber::Number(best_block_number))
            .ok()
            .flatten();

        if latest_header.is_none() {
            drop(storage);
            warn!("No latest header found");
            return false;
        }

        let latest_header = latest_header.unwrap();

        let is_inturn = utils::is_inturn(authority_len, signer_index as u64);

        // Skip over genesis
        if latest_header.number != 0 {
            let latest_signer = utils::recovery_authority(&latest_header).unwrap();
            let current_ts = utils::unix_timestamp();
            let current_last_signer_validation = utils::validate_current_signer_against_last(
                (latest_signer, latest_header.timestamp as f64 / 60.0),
                (signer_pk, current_ts as f64 / 60.0),
            );
            if is_inturn && current_last_signer_validation.is_err() {
                // made info instead of warn since this prints as soon as
                // a block is produced and the node is still in turn
                drop(storage);
                info!("Current signer failed validation against last signer.");
                return false;
            }
        }

        drop(storage);
        info!("Epoch manager inturn: {}", is_inturn);

        is_inturn
    }
}
