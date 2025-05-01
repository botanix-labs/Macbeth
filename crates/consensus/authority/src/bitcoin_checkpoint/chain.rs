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
    confirmation_window: std::ops::RangeInclusive<usize>,
    /// Bitcoin headers chain
    /// front=oldest, back=newest
    checkpoints: ArcSwap<VecDeque<BitcoinCheckpoint>>,
    chain_size_limit: usize,
}

impl BitcoinCheckpointsChain {
    /// Creates a new BitcoinCheckpointsChain
    ///
    /// ## Params
    /// * `strong_confirmation_depth` - how many confirmations are needed to consider a checkpoints strong
    /// * `historical_checkpoints_count` - how many historical checkpoints to keep
    /// * `weak_checkpoints_count` - how many checkpoints before the strong confirmation depth to keep
    // TODO: Should it be u8?
    pub fn try_new(
        strong_confirmation_depth: usize,
        historical_checkpoints_count: usize,
        weak_checkpoints_count: usize,
    ) -> Result<Self, BitcoinCheckpointError> {
        if strong_confirmation_depth == 0 {
            return Err(BitcoinCheckpointError::ZeroStrongConfirmationDepth);
        }

        if weak_checkpoints_count > strong_confirmation_depth {
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

        let confirmations_window = confirmations_from..=confirmations_to;

        let chain_size_limit = confirmations_to
            .checked_sub(confirmations_from) // width of the window
            .and_then(|v| v.checked_add(1)) // +1 for inclusive range
            .ok_or(BitcoinCheckpointError::ChainParamsTooLarge {
                strong_confirmation_depth,
                historical_checkpoints_count,
                weak_checkpoints_count,
            })?;

        //We push new header first and then pop the oldest one, so we need an additional slot
        let checkpoints = VecDeque::with_capacity(chain_size_limit + 1);

        Ok(Self {
            confirmation_window: confirmations_window,
            strong_confirmation_depth,
            checkpoints: ArcSwap::new(Arc::new(checkpoints)),
            chain_size_limit,
        })
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
        if new_checkpoints.len() > self.chain_size_limit {
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

    pub fn get_by_confirmation_depth(&self, depth: usize) -> Option<BitcoinCheckpoint> {
        // Confirmation depth must belong to the configured window
        if !self.confirmation_window.contains(&depth) {
            return None;
        }

        let checkpoints = self.checkpoints.load();

        if checkpoints.is_empty() {
            return None;
        }

        // Translate depth to index

        // how many depths we could keep but haven't accumulated yet
        let shift = self.chain_size_limit - checkpoints.len();

        // if shift > end we haven’t reached even the deepest kept depth
        let end = *self.confirmation_window.end();
        if shift > end {
            return None;
        }

        let deepest_kept = end - shift;

        // we don't have enough checkpoints
        if depth > deepest_kept {
            return None;
        }

        let index = deepest_kept - depth;

        checkpoints.get(index).cloned()
    }

    #[inline(always)]
    pub fn strong(&self) -> Option<BitcoinCheckpoint> {
        self.get_by_confirmation_depth(self.strong_confirmation_depth)
    }

    pub fn size_limit(&self) -> usize {
        self.chain_size_limit
    }

    pub fn len(&self) -> usize {
        let checkpoints = self.checkpoints.load();
        checkpoints.len()
    }

    pub fn is_empty(&self) -> bool {
        let checkpoints = self.checkpoints.load();
        checkpoints.is_empty()
    }

    pub(super) fn recent_height(&self) -> Option<u32> {
        let checkpoints = self.checkpoints.load();
        checkpoints.back().map(|checkpoint| checkpoint.height)
    }

    pub(super) fn lowest_confirmations_depth(&self) -> usize {
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
            let index_shift = self.chain_size_limit - checkpoints.len();

            for (i, checkpoint) in checkpoints.iter().enumerate() {
                let confirmations = self.confirmation_window.end() - i - index_shift;

                writeln!(f, "  {}: {}", confirmations, checkpoint)?;
            }
        }

        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::block::Header as BitcoinHeader;
    use bitcoin::hashes::Hash;
    use bitcoin::TxMerkleNode;
    use std::str::FromStr;

    // Helper function to create a test checkpoint
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
        fn window_too_wide_is_rejected() {
            let strong = usize::MAX - 3;
            let hist = 10; // strong + hist overflows
            let weak = 1;

            let res = BitcoinCheckpointsChain::try_new(strong, hist, weak);
            assert!(matches!(res, Err(BitcoinCheckpointError::ChainParamsTooLarge { .. })));
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

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());
            let checkpoint2 = create_checkpoint(101, checkpoint1.hash);

            chain.push(checkpoint1).expect("push first checkpoint");
            chain.push(checkpoint2).expect("push second checkpoint");

            assert_eq!(chain.len(), 2);
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
                    added_prev_block_hash
                })
                if expected_prev_block_hash == checkpoint1_hash && added_prev_block_hash == BitcoinBlockHash::all_zeros()
            ));

            assert_eq!(chain.len(), 1);
        }

        #[test]
        fn test_push_exceeding_capacity() {
            let chain = BitcoinCheckpointsChain::try_new(3, 1, 1).expect("create a valid chain"); // Size limit = 4

            // Push 5 checkpoints (exceeding capacity)
            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());
            let checkpoint1_hash = checkpoint1.hash;

            let checkpoint2 = create_checkpoint(101, checkpoint1_hash);
            let checkpoint2_hash = checkpoint2.hash;

            let checkpoint3 = create_checkpoint(102, checkpoint2_hash);
            let checkpoint3_hash = checkpoint3.hash;

            let checkpoint4 = create_checkpoint(103, checkpoint3_hash);
            let checkpoint4_hash = checkpoint4.hash;

            let checkpoint5 = create_checkpoint(104, checkpoint4_hash);
            let checkpoint5_hash = checkpoint5.hash;

            chain.push(checkpoint1).expect("push checkpoint 1");
            chain.push(checkpoint2).expect("push checkpoint 2");
            chain.push(checkpoint3).expect("push checkpoint 3");
            chain.push(checkpoint4).expect("push checkpoint 4");
            chain.push(checkpoint5).expect("push checkpoint 5");

            // Should maintain size limit by removing oldest
            assert_eq!(chain.len(), 3);

            // Should not contain the first checkpoint anymore
            assert!(!chain.contains_by_hash(checkpoint1_hash));

            // Should contain checkpoints 3-5
            assert!(chain.contains_by_hash(checkpoint3_hash));
            assert!(chain.contains_by_hash(checkpoint4_hash));
            assert!(chain.contains_by_hash(checkpoint5_hash));
        }

        /// Make sure the deque never grows past its limit (regression for the
        /// off-by-one bug in `push`).
        #[test]
        fn size_limit_is_respected() {
            let chain = BitcoinCheckpointsChain::try_new(3, 1, 1).unwrap(); // limit = 4
            let mut prev = BitcoinBlockHash::all_zeros();

            for h in 1..=10 {
                let cp = create_checkpoint(h, prev);
                prev = cp.hash;
                chain.push(cp).unwrap();
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
            let chain = BitcoinCheckpointsChain::try_new(6, 2, 2).expect("create a valid chain"); // window 10..=4

            // Add 11 checkpoints (enough for all positions in window)
            let mut previous_block_hash = BitcoinBlockHash::all_zeros();
            for i in 1..=7 {
                let checkpoint = create_checkpoint(100 + i, previous_block_hash);

                previous_block_hash = checkpoint.hash;

                assert!(chain.push(checkpoint).is_ok(), "failed to push checkpoint {i}")
            }

            // Check depths within the window
            for depth in 4..=8 {
                let checkpoint = chain.get_by_confirmation_depth(depth);

                assert!(checkpoint.is_some(), "failed to get checkpoint at depth {depth}");

                let expected_position = 11 - depth;
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
            let chain = BitcoinCheckpointsChain::try_new(1, 0, 1).unwrap();
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
            let mut previous_block_hash = BitcoinBlockHash::all_zeros();
            for i in 1..=11 {
                let checkpoint = create_checkpoint(100 + i, previous_block_hash);

                previous_block_hash = checkpoint.hash;

                assert!(chain.push(checkpoint).is_ok(), "push checkpoint {i}")
            }

            let strong_checkpoint = chain.strong();
            // After 11 blocks, the header with exactly 6 confirmations is at height 109.
            assert!(
                matches!(strong_checkpoint, Some(BitcoinCheckpoint { height, .. }) if height == 109)
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
            assert_eq!(chain1.lowest_confirmations_depth(), 4);

            let chain2 = BitcoinCheckpointsChain::try_new(10, 5, 3).expect("create a valid chain");
            assert_eq!(chain2.lowest_confirmations_depth(), 7);
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

            let checkpoint1 = create_checkpoint(100, BitcoinBlockHash::all_zeros());
            let checkpoint2 = create_checkpoint(101, checkpoint1.hash);
            let checkpoint3 = create_checkpoint(102, checkpoint2.hash);
            let checkpoint4 = create_checkpoint(103, checkpoint3.hash);
            let checkpoint5 = create_checkpoint(104, checkpoint4.hash);

            chain.push(checkpoint1.clone()).expect("push checkpoint 1");
            chain.push(checkpoint2.clone()).expect("push checkpoint 2");
            chain.push(checkpoint3.clone()).expect("push checkpoint 3");
            chain.push(checkpoint4.clone()).expect("push checkpoint 4");
            chain.push(checkpoint5.clone()).expect("push checkpoint 5");

            // Should contain the confirmation depth and checkpoint info
            assert!(chain.to_string().contains(
                format!(
                    "8: {}\n  7: {}\n  6: {}\n  5: {}\n  4: {}",
                    checkpoint1, checkpoint2, checkpoint3, checkpoint4, checkpoint5
                )
                .as_str()
            ));
        }
    }
}
