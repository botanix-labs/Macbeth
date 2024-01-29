use reth_interfaces::{
    blockchain_tree::BlockchainTreeEngine, consensus::BlockForkSelectionCriteria,
};
use reth_primitives::{revm_primitives::FixedBytes, U256};
use std::collections::BTreeMap;

const MAX_FORK_DEPTH: usize = 1;

/// Fork Block Selector
pub(crate) struct PoaForkBlockSelector<Client> {
    client: Client,
}

impl<Client> BlockForkSelectionCriteria<Client> for PoaForkBlockSelector<Client>
where
    Client: BlockchainTreeEngine,
{
    fn select(&self) -> Option<(u64, u64, FixedBytes<32>)> {
        let mut forks: BTreeMap<U256, u64> = BTreeMap::new();

        // extract all chains that currently exist
        let all_chain_ids = self.client.chain_ids();

        // get all block hashes from a sidechain that are not part of the canonical chain.
        for chain_id in all_chain_ids {
            let chain_id_blocks_hashes = self.client.all_chain_hashes(chain_id);
            if chain_id_blocks_hashes.keys().len() > MAX_FORK_DEPTH {
                continue
            }

            let mut total_difficulty = U256::from(0);
            for (_block_id, block_hash) in chain_id_blocks_hashes {
                let sealed_block = self.client.block_by_hash(block_hash);
                if let Some(sealed_block) = sealed_block {
                    total_difficulty += sealed_block.difficulty;
                }
            }
            forks.insert(total_difficulty, chain_id);
        }

        // since keys are ordered, get the maximum key value
        if let Some((_total_difficulty, chain_id)) = forks.last_key_value() {
            let best_fork_blocknumhash = self.client.canonical_fork(*chain_id);
            best_fork_blocknumhash
                .map(|blocknumhash| (*chain_id, blocknumhash.number, blocknumhash.hash))
        } else {
            None
        }
    }
}

impl<Client> PoaForkBlockSelector<Client>
where
    Client: BlockchainTreeEngine,
{
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }
}
