use crate::Storage;
use reth_consensus_common::utils;
use reth_primitives::BlockHashOrNumber;
use reth_provider::{BlockReaderIdExt, CanonChainTracker, HeaderProvider, StateProviderFactory};
use tracing::{debug, info, warn};

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
                debug!("Already produced the block for this turn.");
                return false;
            }
        }

        drop(storage);
        info!("Epoch manager inturn: {}", is_inturn);

        is_inturn
    }
}
