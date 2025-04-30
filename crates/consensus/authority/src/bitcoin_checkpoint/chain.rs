use super::checkpoint::BitcoinCheckpoint;
use super::error::BitcoinCheckpointError;
use arc_swap::ArcSwap;
use bitcoin::block::BlockHash as BitcoinBlockHash;
use std::collections::VecDeque;
use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;

pub struct BitcoinCheckpointsChain {
    strong_confirmation_depth: usize,
    /// how many headers to keep
    confirmations_window: std::ops::RangeInclusive<usize>,
    /// Bitcoin headers chain
    /// front=oldest, back=newest
    checkpoints: ArcSwap<VecDeque<BitcoinCheckpoint>>,
    chain_size_limit: usize,
}

impl BitcoinCheckpointsChain {
    pub fn new(
        strong_confirmation_depth: usize,
        historical_checkpoints_count: usize,
        week_checkpoints_count: usize,
    ) -> Self {
        // TODO: Return error
        assert_ne!(strong_confirmation_depth, 0, "pegin conf depth not set correctly");

        let confirmations_window = strong_confirmation_depth + historical_checkpoints_count
            ..=strong_confirmation_depth - week_checkpoints_count;
        let chain_size_limit = confirmations_window.start() - confirmations_window.end() + 1;

        // we push new header first and then pop the oldest one, so we need an additional slot
        let checkpoints = VecDeque::with_capacity(chain_size_limit + 1);

        Self {
            confirmations_window,
            strong_confirmation_depth,
            checkpoints: ArcSwap::new(Arc::new(checkpoints)),
            chain_size_limit,
        }
    }

    pub fn push(&self, checkpoint: BitcoinCheckpoint) -> Result<(), BitcoinCheckpointError> {
        let checkpoints = self.checkpoints.load_full();

        if let Some(recent) = checkpoints.back() {
            if checkpoint.header.prev_blockhash != recent.hash {
                return Err(BitcoinCheckpointError::StaleBlockAdded {
                    expected_prev_block_hash: recent.hash,
                    added_prev_block_hash: checkpoint.header.prev_blockhash,
                });
            }
        }

        let mut new_checkpoints = checkpoints.deref().clone();

        new_checkpoints.push_back(checkpoint);
        if checkpoints.len() > self.chain_size_limit {
            new_checkpoints.pop_front();
        }

        self.checkpoints.store(Arc::new(new_checkpoints));

        Ok(())
    }

    pub fn contains_by_hash(&self, h: BitcoinBlockHash) -> bool {
        let checkpoints = self.checkpoints.load();
        // We expect a few headers at most, so linear scan is optimal.
        checkpoints.iter().any(|checkpoint| checkpoint.hash == h)
    }

    pub fn get_by_confirmations_depth(&self, confirmations: usize) -> Option<BitcoinCheckpoint> {
        if !self.confirmations_window.contains(&confirmations) {
            return None;
        }

        let checkpoints = self.checkpoints.load();

        // TODO: Fix this
        let index = checkpoints.len() - self.confirmations_window.start() - confirmations;

        if index < 0 {
            return None;
        }

        checkpoints.get(index).cloned()
    }

    #[inline(always)]
    pub fn strong(&self) -> Option<BitcoinCheckpoint> {
        self.get_by_confirmations_depth(self.strong_confirmation_depth)
    }

    pub fn size_limit(&self) -> usize {
        self.chain_size_limit
    }

    pub fn len(&self) -> usize {
        let checkpoints = self.checkpoints.load();
        checkpoints.len()
    }

    pub(super) fn recent_height(&self) -> Option<u32> {
        let checkpoints = self.checkpoints.load();
        checkpoints.back().map(|checkpoint| checkpoint.height)
    }

    pub(super) fn lowest_confirmations_depth(&self) -> usize {
        *self.confirmations_window.end()
    }
}

impl Display for BitcoinCheckpointsChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let checkpoints = self.checkpoints.load();

        writeln!(f, "BitcoinCheckpointsChain {{")?;

        if checkpoints.is_empty() {
            writeln!(f, "  No checkpoints ")?;
        } else {
            let index_shift = self.chain_size_limit - checkpoints.len();

            for (i, checkpoint) in checkpoints.iter().enumerate() {
                let confirmations = self.confirmations_window.start() + i + index_shift;

                writeln!(f, "  {}: {}", confirmations, checkpoint)?;
            }
        }

        write!(f, "}}")
    }
}
