use crate::message::NewBlockMessage;
use reth_consensus_common::validation;
use reth_interfaces::consensus::ConsensusError;
use reth_primitives::{ChainSpec, Header, PeerId, SealedBlock};

use std::{
    collections::VecDeque,
    task::{Context, Poll},
};
use tracing::info;

/// Abstraction over block import.
pub trait BlockImport: std::fmt::Debug + Send + Sync {
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
#[derive(Debug)]
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
pub struct ProofOfAuthorityBlockImport {
    queue: VecDeque<(PeerId, NewBlockMessage)>,

    chain_spec: ChainSpec,
}

impl ProofOfAuthorityBlockImport {
    /// Creates Proof of Authority Block Import with the provided consensus mechanism
    pub fn new(chain_spec: ChainSpec) -> Self {
        Self { queue: VecDeque::new(), chain_spec }
    }

    /// Vaidates a header on block import
    fn validate_header(&mut self, header: &Header) -> Result<(), ConsensusError> {
        validation::validate_header_with_total_difficulty(header, header.difficulty)?;
        Ok(())
    }

    /// Fully Validates a block on block import
    fn validate_new_block(&mut self, new_block: &SealedBlock) -> Result<(), ConsensusError> {
        validation::validate_block_standalone(new_block, &self.chain_spec)?;
        Ok(())
    }
}

impl BlockImport for ProofOfAuthorityBlockImport {
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage) {
        info!("on_new_block, peer_id: {:?}, incoming_block: {:?}", peer_id, incoming_block);
        self.queue.push_back((peer_id, incoming_block));
    }

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportOutcome> {
        if let Some(pair) = self.queue.pop_front() {
            let block = pair.1.block.clone();
            let result: Result<BlockValidation, BlockImportError> = self
                // TODO(armins) is is possible not to clone the block again
                // TODO(armins) validate header
                .validate_new_block(&block.block.clone().seal_slow())
                .map_err(BlockImportError::Consensus)
                .map(|_| BlockValidation::ValidHeader { block: pair.1 });

            return Poll::Ready(BlockImportOutcome { peer: pair.0, result })
        }
        Poll::Pending
    }
}
