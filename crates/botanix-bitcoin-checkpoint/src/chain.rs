//! Bitcoin checkpoints chain
//!
//! This module provides functionality to maintain a chain of Bitcoin checkpoints
//! with configurable confirmation depths and size limits.

use super::{checkpoint::BitcoinCheckpoint, error::BitcoinCheckpointError};
use arc_swap::ArcSwap;
use bitcoin::block::BlockHash as BitcoinBlockHash;
use std::{collections::VecDeque, fmt::Display, ops::Deref, sync::Arc};

/// Maintains a chain of Bitcoin checkpoints with configurable confirmation depths.
///
/// The chain maintains a window of blocks that satisfy specific confirmation requirements.
/// It supports operations to get blocks at specific confirmation depths, strong confirmations,
/// and chain management.
pub struct BitcoinCheckpointsChain {
    /// The number of confirmations required for a checkpoint to be considered strong
    strong_confirmation_depth: usize,

    /// Bitcoin headers chain
    /// front=oldest, back=newest
    // We update every 10 mins, so we use `ArcSwap` to make reads lock-free and fast.
    checkpoints: ArcSwap<VecDeque<BitcoinCheckpoint>>,

    /// Range of confirmation depths to keep in the window
    /// front=lowest, back=highest
    confirmation_window: std::ops::RangeInclusive<usize>,

    /// Size of the confirmation window (precalculated for efficiency)
    confirmation_window_size: usize,
}

impl BitcoinCheckpointsChain {
    /// Creates a new BitcoinCheckpointsChain with the specified parameters.
    ///
    /// ## Parameters
    /// * `strong_confirmation_depth` - How many confirmations are needed to consider a checkpoint
    ///   strong
    /// * `historical_checkpoints_count` - How many historical checkpoints to keep (depth > strong)
    /// * `weak_checkpoints_count` - How many checkpoints before the strong confirmation depth to
    ///   keep (depth < strong)
    ///
    /// ## Returns
    /// A new `BitcoinCheckpointsChain` or an error if the parameters are invalid
    ///
    /// ## Errors
    /// * `ZeroStrongConfirmationDepth` - If strong confirmation depth is zero
    /// * `WeakCheckpointsCountTooBig` - If weak checkpoints count is greater than strong
    ///   confirmation depth
    /// * `ChainParamsTooLarge` - If the parameters would cause numeric overflow
    pub fn try_new(
        strong_confirmation_depth: usize,
        historical_checkpoints_count: usize,
        weak_checkpoints_count: usize,
    ) -> Result<Self, BitcoinCheckpointError> {
        if strong_confirmation_depth == 0 {
            return Err(BitcoinCheckpointError::ZeroStrongConfirmationDepth);
        }

        if weak_checkpoints_count >= strong_confirmation_depth {
            return Err(BitcoinCheckpointError::WeakCheckpointsCountTooBig {
                weak_checkpoints_count,
                strong_confirmation_depth,
            });
        }

        let confirmations_from = strong_confirmation_depth - weak_checkpoints_count;

        let confirmations_to = strong_confirmation_depth
            .checked_add(historical_checkpoints_count)
            .ok_or(BitcoinCheckpointError::ChainParamsTooLarge {
                strong_confirmation_depth,
                historical_checkpoints_count,
                weak_checkpoints_count,
            })?;

        let confirmation_window_size = confirmations_to
            .checked_sub(confirmations_from) // width of the window
            .and_then(|v| v.checked_add(1)) // +1 for inclusive range
            .ok_or(BitcoinCheckpointError::ChainParamsTooLarge {
                strong_confirmation_depth,
                historical_checkpoints_count,
                weak_checkpoints_count,
            })?;

        let confirmation_window = confirmations_from..=confirmations_to;

        let checkpoints = VecDeque::with_capacity(confirmation_window_size);

        Ok(Self {
            strong_confirmation_depth,
            confirmation_window,
            confirmation_window_size,
            checkpoints: ArcSwap::new(Arc::new(checkpoints)),
        })
    }

    /// Adds a new checkpoint to the chain.
    ///
    /// The new checkpoint must be linked to the current tip (its previous hash
    /// must match the hash of the current tip).
    ///
    /// If the chain size exceeds the limit, the oldest checkpoints are removed.
    ///
    /// We expect `push` to be called once at 10 mins, so for interior mutability
    /// we use ArcSwap, which guarantee lock-free and fast reads
    ///
    /// ## Parameters
    /// * `checkpoint` - The checkpoint to add
    ///
    /// ## Returns
    /// Ok(()) if the checkpoint was added, or an error
    ///
    /// ## Errors
    /// * `StaleBlockAdded` - If the checkpoint doesn't connect to the current chain
    pub fn push(&self, checkpoint: BitcoinCheckpoint) -> Result<(), BitcoinCheckpointError> {
        let checkpoints = self.checkpoints.load_full();

        if let Some(recent) = checkpoints.back() {
            if checkpoint.header.prev_blockhash != recent.hash {
                return Err(BitcoinCheckpointError::StaleBlockAdded {
                    expected_prev_block_hash: recent.hash,
                    received_prev_block_hash: checkpoint.header.prev_blockhash,
                });
            }
        }

        let mut new_checkpoints = checkpoints.deref().clone();

        // Remove the oldest checkpoint if we exceed the size limit
        if new_checkpoints.len() == self.confirmation_window_size {
            new_checkpoints.pop_front();
        }

        new_checkpoints.push_back(checkpoint);

        self.checkpoints.store(Arc::new(new_checkpoints));

        Ok(())
    }

    /// Checks if a checkpoint with the given hash exists in the chain.
    ///
    /// ## Parameters
    /// * `hash` - The hash to look for
    ///
    /// ## Returns
    /// `true` if a checkpoint with the given hash exists, `false` otherwise
    pub fn contains_by_hash(&self, hash: BitcoinBlockHash) -> bool {
        let checkpoints = self.checkpoints.load();
        // We expect a few headers at most, so linear scan is optimal.
        checkpoints.iter().any(|checkpoint| checkpoint.hash == hash)
    }

    /// Gets a checkpoint at the specified confirmation depth.
    ///
    /// ## Parameters
    /// * `depth` - The confirmation depth to get the checkpoint for
    ///
    /// ## Returns
    /// The checkpoint at the specified depth, or `None` if:
    /// - The depth is outside the configured confirmation window
    /// - The chain is empty
    /// - The chain doesn't have enough blocks to reach the specified depth
    pub fn get_by_confirmation_depth(&self, depth: usize) -> Option<BitcoinCheckpoint> {
        // Confirmation depth must belong to the configured window
        if !self.confirmation_window.contains(&depth) {
            return None;
        }

        let checkpoints = self.checkpoints.load();
        if checkpoints.is_empty() {
            return None;
        }

        // Translate depth to index from back first
        let lowest_confirmation_depth = *self.confirmation_window.start(); // e.g. 4
        let index_from_back = depth - lowest_confirmation_depth;

        // Do we have enough headers?
        if index_from_back >= checkpoints.len() {
            return None;
        }

        // Calculate an index from front from the index from back :)
        let index = checkpoints.len() - 1 - index_from_back;

        checkpoints.get(index).cloned()
    }

    /// Gets the checkpoint with strong confirmation depth.
    ///
    /// ## Returns
    /// The checkpoint at the strong confirmation depth, or `None` if not available
    #[inline]
    pub fn strong(&self) -> Option<BitcoinCheckpoint> {
        self.get_by_confirmation_depth(self.strong_confirmation_depth)
    }

    /// Gets the maximum number of checkpoints this chain can hold.
    ///
    /// ## Returns
    /// The maximum number of checkpoints
    #[inline]
    pub fn size_limit(&self) -> usize {
        self.confirmation_window_size
    }

    /// Gets the number of checkpoints currently in the chain.
    ///
    /// ## Returns
    /// The number of checkpoints
    pub fn len(&self) -> usize {
        let checkpoints = self.checkpoints.load();
        checkpoints.len()
    }

    /// Checks if the chain is empty.
    ///
    /// ## Returns
    /// `true` if the chain has no checkpoints, `false` otherwise
    pub fn is_empty(&self) -> bool {
        let checkpoints = self.checkpoints.load();
        checkpoints.is_empty()
    }

    /// Clears all checkpoints from the chain.
    ///
    /// This method removes all checkpoints from the chain, resetting it to an empty state.
    /// The configuration settings such as confirmation depths and window size remain unchanged.
    pub fn clear(&self) {
        self.checkpoints.store(Arc::new(VecDeque::new()));
    }

    /// Gets the height of the most recent checkpoint.
    ///
    /// ## Returns
    /// The height of the most recent checkpoint, or `None` if the chain is empty
    pub(super) fn recent_height(&self) -> Option<u32> {
        let checkpoints = self.checkpoints.load();
        checkpoints.back().map(|checkpoint| checkpoint.height)
    }

    /// Gets the lowest confirmation depth in the window.
    ///
    /// ## Returns
    /// The lowest confirmation depth
    #[inline]
    pub(super) fn lowest_confirmation_depth(&self) -> usize {
        *self.confirmation_window.start()
    }
}

impl Display for BitcoinCheckpointsChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let checkpoints = self.checkpoints.load();

        writeln!(f, "BitcoinCheckpointsChain {{")?;

        if checkpoints.is_empty() {
            writeln!(f, "  No checkpoints ")?;
        } else {
            let shift = self.confirmation_window_size - checkpoints.len();

            for (i, checkpoint) in checkpoints.iter().enumerate() {
                let confirmations = self.confirmation_window.end().saturating_sub(i + shift);

                writeln!(f, "  {}: {}", confirmations, checkpoint)?;
            }
        }

        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{block::Header as BitcoinHeader, hashes::Hash, TxMerkleNode};
    use std::str::FromStr;

    mod try_new {
        use super::*;

        #[test]
        fn test_with_valid_parameters() {
            let chain =
                BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a checkpoint chain");

            assert_eq!(chain.confirmation_window, 4..=10);
        }

        /// Overflow during `confirmations_to + 1` is trapped.
        #[test]
        fn test_window_too_wide_is_rejected() {
            let strong = usize::MAX - 3;
            let hist = 10; // strong + hist overflows
            let weak = 1;

            let res = BitcoinCheckpointsChain::try_new(strong, hist, weak);
            assert!(matches!(res, Err(BitcoinCheckpointError::ChainParamsTooLarge { .. })));
        }

        #[test]
        fn test_weak_confirmation_depth_too_big() {
            let chain = BitcoinCheckpointsChain::try_new(1, 0, 1);

            assert!(matches!(chain, Err(BitcoinCheckpointError::WeakCheckpointsCountTooBig {
                weak_checkpoints_count,
                strong_confirmation_depth
            }) if weak_checkpoints_count == 1 && strong_confirmation_depth == 1));
        }
    }

    mod push {
        use super::*;

        #[test]
        fn test_push_first_checkpoint() {
            let chain =
                BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a checkpoint chain");

            let checkpoint = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            chain.push(checkpoint).expect("push first checkpoint");

            assert_eq!(chain.len(), 1);
        }

        #[test]
        fn test_push_consecutive_checkpoints() {
            let chain =
                BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a checkpoint chain");

            let checkpoints = create_checkpoints_and_push_to_chain(&chain, 100, 2);

            assert_eq!(chain.len(), 2);
            assert_eq!(checkpoints[0].height, 100);
            assert_eq!(checkpoints[1].height, 101);
        }

        #[test]
        fn test_push_invalid_checkpoint_chain() {
            let chain =
                BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a checkpoint chain");

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            let checkpoint1_hash = checkpoint1.hash;

            let invalid_checkpoint = create_checkpoint(
                101,
                BitcoinBlockHash::all_zeros(), // Wrong previous hash
            );

            chain.push(checkpoint1).expect("push first checkpoint");

            let result = chain.push(invalid_checkpoint);

            assert!(matches!(
                result,
                Err(BitcoinCheckpointError::StaleBlockAdded {
                    expected_prev_block_hash,
                    received_prev_block_hash
                })
                if expected_prev_block_hash == checkpoint1_hash && received_prev_block_hash == BitcoinBlockHash::all_zeros()
            ));

            assert_eq!(chain.len(), 1);
        }

        #[test]
        fn test_push_exceeding_capacity() {
            let chain = BitcoinCheckpointsChain::try_new(3, 1, 1).expect("create a valid chain"); // Size limit = 4

            // Push 5 checkpoints (exceeding capacity)
            let checkpoints = create_checkpoints_and_push_to_chain(&chain, 100, 5);

            // Should maintain size limit by removing oldest
            assert_eq!(chain.len(), 3);

            // Should not contain the first checkpoint anymore
            assert!(!chain.contains_by_hash(checkpoints[0].hash));
            assert!(!chain.contains_by_hash(checkpoints[1].hash));

            // Should contain checkpoints 3-5
            assert!(chain.contains_by_hash(checkpoints[2].hash));
            assert!(chain.contains_by_hash(checkpoints[3].hash));
            assert!(chain.contains_by_hash(checkpoints[4].hash));
        }

        /// Make sure the deque never grows past its limit (regression for the
        /// off-by-one bug in `push`).
        #[test]
        fn size_limit_is_respected() {
            let chain = BitcoinCheckpointsChain::try_new(3, 1, 1).expect("create a valid chain"); // limit = 4
            let mut prev = BitcoinBlockHash::all_zeros();

            for h in 1..=10 {
                let cp = create_checkpoint(h, prev);
                prev = cp.hash;
                chain.push(cp).expect("push checkpoint");
                assert!(chain.len() <= chain.size_limit());
            }
        }
    }

    mod contains_by_hash {
        use super::*;

        #[test]
        fn test_contains_by_hash_when_chain_is_empty() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");
            let hash = BitcoinBlockHash::all_zeros();

            assert!(!chain.contains_by_hash(hash));
        }

        #[test]
        fn test_contains_by_hash_when_hash_exists() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            let checkpoint = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            let checkpoint_hash = checkpoint.hash;

            chain.push(checkpoint).expect("push checkpoint");

            assert!(chain.contains_by_hash(checkpoint_hash));
        }

        #[test]
        fn test_contains_by_hash_when_hash_does_not_exist() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            let checkpoint = create_checkpoint(
                100,
                BitcoinBlockHash::from_str(
                    "0000000000000000000000000000000000000000000000000000000000000100",
                )
                .expect("create block hash"),
            );

            assert!(chain.push(checkpoint).is_ok());

            let hash = BitcoinBlockHash::from_str(
                "0000000000000000000000000000000000000000000000000000000000000200",
            )
            .expect("create block hash");

            assert!(!chain.contains_by_hash(hash));
        }
    }

    mod get_by_confirmation_depth {
        use super::*;

        #[test]
        fn test_get_by_confirmations_depth_empty_chain() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            assert_eq!(chain.get_by_confirmation_depth(6), None);
        }

        #[test]
        fn test_get_by_confirmations_depth_out_of_window() {
            let chain = BitcoinCheckpointsChain::try_new(6, 1, 1).expect("create a valid chain");

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());
            let checkpoint2 = create_checkpoint(100, checkpoint1.hash);
            let checkpoint3 = create_checkpoint(100, checkpoint2.hash);
            let checkpoint4 = create_checkpoint(100, checkpoint3.hash);

            chain.push(checkpoint1).expect("push checkpoint 1");
            chain.push(checkpoint2).expect("push checkpoint 2");
            chain.push(checkpoint3).expect("push checkpoint 3");
            chain.push(checkpoint4).expect("push checkpoint 4");

            // Out of range - too high
            assert_eq!(chain.get_by_confirmation_depth(11), None);

            // Out of range - too low
            assert_eq!(chain.get_by_confirmation_depth(3), None);
        }

        #[test]
        fn test_get_by_confirmations_depth_within_window_but_not_enough_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 0, 1).expect("create a valid chain");

            let checkpoint = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            chain.push(checkpoint).expect("push checkpoint");

            // Within window but not enough checkpoints
            assert_eq!(chain.get_by_confirmation_depth(6), None);
        }

        #[test]
        fn test_get_by_confirmations_depth_success() {
            let chain = BitcoinCheckpointsChain::try_new(6, 2, 2).expect("create a valid chain"); // window 4..=8

            // Add 7 checkpoints
            create_checkpoints_and_push_to_chain(&chain, 100, 7);

            // Check depths within the window
            for depth in 4..=8 {
                let checkpoint = chain.get_by_confirmation_depth(depth);
                assert!(checkpoint.is_some(), "failed to get checkpoint at depth {depth}");

                let expected_position = 10 - depth;
                let expected_height = 100 + expected_position;

                assert!(
                    matches!(checkpoint, Some(BitcoinCheckpoint { height, .. }) if height == expected_height as u32)
                );
            }
        }
    }

    mod strong {
        use super::*;

        #[test]
        fn test_strong_empty_chain() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");
            assert_eq!(chain.strong(), None);
        }

        #[test]
        fn test_strong_not_enough_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            let checkpoint = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            chain.push(checkpoint).expect("push checkpoint");

            assert_eq!(chain.strong(), None);
        }

        #[test]
        fn test_strong_with_small_window() {
            let chain = BitcoinCheckpointsChain::try_new(2, 0, 1).expect("create a valid chain");
            let checkpoint1 = create_checkpoint(1, BitcoinBlockHash::all_zeros());
            let checkpoint2 = create_checkpoint(2, checkpoint1.hash);

            chain.push(checkpoint1).expect("push first checkpoint");
            assert!(chain.strong().is_none());

            chain.push(checkpoint2).expect("push first checkpoint");
            assert!(
                matches!(chain.strong(), Some(BitcoinCheckpoint { height, .. }) if height == 1)
            );
        }

        #[test]
        fn test_strong_enough_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            // Add 11 checkpoints (enough for strong confirmation)
            create_checkpoints_and_push_to_chain(&chain, 100, 11);

            let strong_checkpoint = chain.strong();

            // After 11 blocks, the header with exactly 6 confirmations is at height 108.
            assert!(
                matches!(strong_checkpoint, Some(BitcoinCheckpoint { height, .. }) if height == 108)
            );
        }
    }

    mod size_limit {
        use super::*;

        #[test]
        fn test_size_limit() {
            let chain1 = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain"); // 10..=4 -> limit=7
            assert_eq!(chain1.size_limit(), 7);

            let chain2 = BitcoinCheckpointsChain::try_new(10, 10, 2).expect("create a valid chain"); // 20..=8 -> limit=13
            assert_eq!(chain2.size_limit(), 13);
        }
    }

    mod len {
        use super::*;

        #[test]
        fn test_len_empty() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");
            assert_eq!(chain.len(), 0);
        }

        #[test]
        fn test_len_with_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            let checkpoint2 = create_checkpoint(101, checkpoint1.hash);

            chain.push(checkpoint1).expect("push first checkpoint");
            assert_eq!(chain.len(), 1);

            chain.push(checkpoint2).expect("push second checkpoint");
            assert_eq!(chain.len(), 2);
        }
    }

    mod is_empty {
        use super::*;

        #[test]
        fn test_is_empty_new_chain() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            assert!(chain.is_empty());
        }

        #[test]
        fn test_is_empty_with_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            let checkpoint = create_checkpoint(100, BitcoinBlockHash::all_zeros());

            chain.push(checkpoint).expect("push checkpoint");

            assert!(!chain.is_empty());
        }
    }

    mod recent_height {
        use super::*;

        #[test]
        fn test_recent_height_empty_chain() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");
            assert_eq!(chain.recent_height(), None);
        }

        #[test]
        fn test_recent_height_with_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());
            let checkpoint2 = create_checkpoint(101, checkpoint1.hash);

            chain.push(checkpoint1).expect("push first checkpoint");
            assert_eq!(chain.recent_height(), Some(100));

            chain.push(checkpoint2).expect("push second checkpoint");
            assert_eq!(chain.recent_height(), Some(101));
        }
    }

    mod lowest_confirmations_depth {
        use super::*;

        #[test]
        fn test_lowest_confirmations_depth() {
            let chain1 = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");
            assert_eq!(chain1.lowest_confirmation_depth(), 4);

            let chain2 = BitcoinCheckpointsChain::try_new(10, 5, 3).expect("create a valid chain");
            assert_eq!(chain2.lowest_confirmation_depth(), 7);
        }
    }

    mod display_tests {
        use super::*;

        #[test]
        fn test_display_empty_chain() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            assert!(chain.to_string().contains("No checkpoints"));
        }

        #[test]
        fn test_display_with_only_weak_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 2, 2).expect("create a valid chain");

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());
            let checkpoint2 = create_checkpoint(101, checkpoint1.hash);

            chain.push(checkpoint1.clone()).expect("push first checkpoint");
            chain.push(checkpoint2.clone()).expect("push second checkpoint");

            // Should contain the confirmation depth and checkpoint info
            assert!(chain
                .to_string()
                .contains(format!("5: {}\n  4: {}", checkpoint1, checkpoint2).as_str()));
        }

        #[test]
        fn test_display_with_all_checkpoints() {
            let chain = BitcoinCheckpointsChain::try_new(6, 2, 2).expect("create a valid chain");

            let checkpoints = create_checkpoints_and_push_to_chain(&chain, 100, 5);

            // Should contain the confirmation depth and checkpoint info
            assert!(chain.to_string().contains(
                format!(
                    "8: {}\n  7: {}\n  6: {}\n  5: {}\n  4: {}",
                    checkpoints[0], checkpoints[1], checkpoints[2], checkpoints[3], checkpoints[4]
                )
                .as_str()
            ));
        }
    }

    mod clear {
        use super::*;

        #[test]
        fn test_clear() {
            let chain = BitcoinCheckpointsChain::try_new(6, 4, 2).expect("create a valid chain");

            create_checkpoints_and_push_to_chain(&chain, 100, 5);

            assert!(!chain.is_empty());

            chain.clear();

            assert!(chain.is_empty());
        }
    }

    fn create_checkpoint(height: u32, prev_blockhash: BitcoinBlockHash) -> BitcoinCheckpoint {
        let header = BitcoinHeader {
            version: Default::default(),
            prev_blockhash,
            merkle_root: TxMerkleNode::all_zeros(),
            time: Default::default(),
            bits: Default::default(),
            nonce: Default::default(),
        };

        BitcoinCheckpoint { height, hash: header.block_hash(), header }
    }

    fn create_checkpoints_and_push_to_chain(
        chain: &BitcoinCheckpointsChain,
        start_height: u32,
        count: usize,
    ) -> Vec<BitcoinCheckpoint> {
        let mut checkpoints = Vec::with_capacity(count);
        let mut prev_hash = BitcoinBlockHash::all_zeros();

        for i in 0..count {
            let height = start_height + i as u32;
            let checkpoint = create_checkpoint(height, prev_hash);
            prev_hash = checkpoint.hash;

            assert!(chain.push(checkpoint.clone()).is_ok(), "push checkpoint {}", i + 1);

            checkpoints.push(checkpoint);
        }

        checkpoints
    }
}
