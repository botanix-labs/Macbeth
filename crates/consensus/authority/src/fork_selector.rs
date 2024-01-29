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
        let mut forks: BTreeMap<u64, u64> = BTreeMap::new(); // timestamp - chain_id

        // extract all chains that currently exist
        let all_chain_ids = self.client.chain_ids();

        // get all block hashes from a sidechain that are not part of the canonical chain.
        for chain_id in all_chain_ids {
            let chain_id_blocks_hashes = self.client.all_chain_hashes(chain_id);
            if chain_id_blocks_hashes.keys().len() > MAX_FORK_DEPTH {
                continue
            }

            // sort by timestamp - chain id
            chain_id_blocks_hashes.iter().for_each(|(chain_id, block_hash)| {
                let sealed_block = self.client.block_by_hash(*block_hash);
                if let Some(sealed_block) = sealed_block {
                    forks.insert(sealed_block.timestamp, *chain_id);
                }
            });
        }

        // since keys are ordered by timestamp, get the minimum key value (earliest timestamp)
        if let Some((_total_difficulty, chain_id)) = forks.iter().rev().nth(0) {
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
