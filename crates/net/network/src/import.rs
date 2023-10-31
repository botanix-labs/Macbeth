use crate::message::NewBlockMessage;
use reth_eth_wire::NewBlock;
use reth_interfaces::consensus::{Consensus, ConsensusError};
use reth_primitives::{
    constants::eip225::{DIFF_INTURN, DIFF_NOTURN, DIFF_NOVOTE},
    Header, PeerId,
};
use std::{
    collections::VecDeque,
    task::{Context, Poll}, sync::Arc,
};

/// Abstraction over block import.
pub trait BlockImport: Send + Sync {
    /// Invoked for a received `NewBlock` broadcast message from the peer.
    ///
    /// > When a `NewBlock` announcement message is received from a peer, the client first verifies
    /// > the basic header validity of the block, checking whether the proof-of-work value is valid.
    ///
    /// This is supposed to start verification. The results are then expected to be returned via
    /// [`BlockImport::poll`].
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage);

    /// Returns the results of a [`BlockImport::on_new_block`]
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockImportOutcome>;
}

/// Outcome of the [`BlockImport`]'s block handling.
pub struct BlockImportOutcome {
    /// Sender of the `NewBlock` message.
    pub peer: PeerId,
    /// The result after validating the block
    pub result: Result<BlockValidation, BlockImportError>,
}

/// Represents the successful validation of a received `NewBlock` message.
#[derive(Debug)]
pub enum BlockValidation {
    /// Basic Header validity check, after which the block should be relayed to peers via a
    /// `NewBlock` message
    ValidHeader {
        /// received block
        block: NewBlockMessage,
    },
    /// Successfully imported: state-root matches after execution. The block should be relayed via
    /// `NewBlockHashes`
    ValidBlock {
        /// validated block.
        block: NewBlockMessage,
    },
}

/// Represents the error case of a failed block import
#[derive(Debug, thiserror::Error)]
pub enum BlockImportError {
    /// Consensus error
    #[error(transparent)]
    Consensus(#[from] reth_interfaces::consensus::ConsensusError),
}

/// An implementation of `BlockImport` used in Proof-of-Stake consensus that does nothing.
///
/// Block propagation over devp2p is invalid in POS: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProofOfStakeBlockImport;

impl BlockImport for ProofOfStakeBlockImport {
    fn on_new_block(&mut self, _peer_id: PeerId, _incoming_block: NewBlockMessage) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportOutcome> {
        Poll::Pending
    }
}

/// An implementation of `BlockImport` used in Proof-of-Authority consensus
#[derive(Debug)]
#[non_exhaustive]
pub struct ProofOfAuthorityBlockImport<C> {
    queue: VecDeque<(PeerId, NewBlockMessage)>,
    // TODO(networking) Maybe use Concensus, maybe not. Still not sure how this should be coded
    _consensus: C,
}

impl<C> ProofOfAuthorityBlockImport<C> {
    /// Creates Proof of Authority Block Import with the provided consensus mechanism
    pub fn new(_consensus: C) -> Self {
        Self { queue: VecDeque::new(), _consensus }
    }

    /// Vaidates a header on block import
    fn validate_header(&mut self, _header: Header) -> Result<(), ConsensusError> {
        // TODO (networking) This will need to be updated with more checks
        Ok(())
    }

    /// Validates a block on block import
    fn validate_new_block(&mut self, block: Arc<NewBlock>) -> Result<(), ConsensusError> {
        if block.td != DIFF_INTURN && block.td != DIFF_NOTURN && block.td != DIFF_NOVOTE {
            return Err(ConsensusError::AuthorityDifficultyInvalid)
        }
        self.validate_header(block.block.header.clone())?;
        // TODO (networking) This will need to be updated with more checks
        Ok(())
    }
}

impl<C> BlockImport for ProofOfAuthorityBlockImport<C>
where
    C: Consensus,
{
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage) {
        self.queue.push_back((peer_id, incoming_block));
    }

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportOutcome> {
        if let Some(pair) = self.queue.pop_front() {
            let block = pair.1.block.clone();
            let result = self
                .validate_new_block(block)
                .map_err(BlockImportError::Consensus)
                .map(|_| BlockValidation::ValidHeader { block: pair.1.clone() });
            
            return Poll::Ready(BlockImportOutcome { peer: pair.0, result })
        }
        Poll::Pending
    }
}
