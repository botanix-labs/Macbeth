//! Optimism Consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use reth_consensus::{Consensus, ConsensusError};
use reth_consensus_common::{validation, validation::validate_header_extradata};
use reth_primitives::{ChainSpec, Header, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT_HASH, U256};
use std::{sync::Arc, time::SystemTime};

/// Optimism consensus implementation.
///
/// Provides basic checks as outlined in the execution specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimismBeaconConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl OptimismBeaconConsensus {
    /// Create a new instance of [OptimismBeaconConsensus]
    ///
    /// # Panics
    ///
    /// If given chain spec is not optimism [ChainSpec::is_optimism]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        assert!(chain_spec.is_optimism(), "optimism consensus only valid for optimism chains");
        Self { chain_spec }
    }
}

impl Consensus for OptimismBeaconConsensus {
    /// Validate header
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validation::validate_header_standalone(header, &self.chain_spec)?;
        Ok(())
    }

    /// Validate header against parent
    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        header.validate_against_parent(parent, &self.chain_spec).map_err(ConsensusError::from)?;
        Ok(())
    }

    /// Validate header with total difficulty
    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    /// Validate block
    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        Ok(())
    }

    /// Validate extra data header
    fn validate_extra_data_header(
        &self,
        header: &Header,
        authority_signers: &[secp256k1::PublicKey],
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    /// Validate block beneficiary
    fn validate_block_beneficiary(&self, header: &Header) -> Result<(), ConsensusError> {
        Ok(())
    }

    /// Validates header standalone according to the authority consensus rules.
    fn validate_header_standalone(
        &self,
        header: &Header,
        authority_signers: &[secp256k1::PublicKey],
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}
