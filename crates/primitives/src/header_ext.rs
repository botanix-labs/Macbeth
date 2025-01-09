use bitcoincore_rpc::{Error as BitcoindError, RpcApi};
use revm_primitives::Address;
use secp256k1::ecdsa::RecoverableSignature;
use thiserror::Error;

use crate::{
    botanix::consensus_package::{BotanixConsensusPackage, RecentHeader},
    extra_data_header::{ExtraDataHeader, ExtraDataHeaderDeserializeError},
    Bytes, Header,
};
use reth_btc_wallet::bitcoind::BitcoindFactory;

/// Authority Block signatures
pub type BlockWitness = Vec<RecoverableSignature>;
/// Extension trait for the block header
/// Mainly adding extra data header utility functions
pub trait HeaderExt {
    /// serilaizes and adds extra data header to the header
    fn add_extra_data_header(&mut self, edh: &ExtraDataHeader);

    /// Attempts to deserialize the extra data header from the header
    fn deserialize_extra_data_header(
        &self,
    ) -> Result<ExtraDataHeader, ExtraDataHeaderDeserializeError>;

    /// Creates a Botanix consensus package from the current header
    /// Meaning we take the checkpoint block hash + aggregated public key store in edh
    /// The only things we dont have is the bitcoin network (needed to validate bitcoind addresses
    /// during pegout) Lastly we need to take the blockhash and get the block header from
    /// bitcoind
    fn botanix_consensus_package(
        &self,
        btc_network: bitcoin::Network,
        bitcoind_factory: impl BitcoindFactory,
    ) -> Result<BotanixConsensusPackage, BotanixConsensusPackageError>;

    /// Get aggregate public key
    fn get_aggregate_public_key(
        &self,
    ) -> Result<secp256k1::PublicKey, ExtraDataHeaderDeserializeError>;

    /// Get the block producer address
    fn block_producer_address(&self) -> Result<Address, ExtraDataHeaderDeserializeError>;
}

/// Errors that can occur while creating a Botanix consensus package
#[derive(Debug, Error)]
pub enum BotanixConsensusPackageError {
    #[error("Failed to deserialize the Extra Data Header: {0}")]
    /// Failed to deserialize the Extra Data Header
    FailedToDeserializeExtraDataHeader(ExtraDataHeaderDeserializeError),

    #[error("Failed to create Bitcoind client: {0}")]
    /// Failed to create Bitcoind client
    FailedToCreateBitcoindClient(BitcoindError),

    #[error("Failed to retrieve the bitcoin checkpoint header: {0}")]
    /// Failed to retrieve the bitcoin checkpoint header
    FailedToRetrieveBitcoinCheckpointHeader(BitcoindError),

    #[error("Failed to retrieve the bitcoin checkpoint height: {0}")]
    /// Failed to retrieve the bitcoin checkpoint height
    FailedToRetrieveBitcoinCheckpointHeight(BitcoindError),
}

impl HeaderExt for Header {
    /// Adds extra data header to the header
    fn add_extra_data_header(&mut self, edh: &ExtraDataHeader) {
        self.extra_data = Bytes::from(edh.serialize());
    }

    /// get block producer address
    fn block_producer_address(&self) -> Result<Address, ExtraDataHeaderDeserializeError> {
        let edh = self.deserialize_extra_data_header()?;
        Ok(edh.block_producer_address)
    }

    /// deserialize the extra data header from the header
    fn deserialize_extra_data_header(
        &self,
    ) -> Result<ExtraDataHeader, ExtraDataHeaderDeserializeError> {
        let binding = self.extra_data.to_vec();
        let mut extra_data = binding.as_slice();
        ExtraDataHeader::deserialize(&mut extra_data)
    }

    /// Creates a Botanix consensus package
    fn botanix_consensus_package(
        &self,
        btc_network: bitcoin::Network,
        bitcoind_factory: impl BitcoindFactory,
    ) -> Result<BotanixConsensusPackage, BotanixConsensusPackageError> {
        let edh = match self.deserialize_extra_data_header() {
            Ok(edh) => edh,
            Err(e) => {
                return Err(BotanixConsensusPackageError::FailedToDeserializeExtraDataHeader(e))
            }
        };

        let bitcoind = match bitcoind_factory.build_and_connect() {
            Ok(bitcoind) => bitcoind,
            Err(e) => return Err(BotanixConsensusPackageError::FailedToCreateBitcoindClient(e)),
        };

        let bitcoin_checkpoint_header = match bitcoind.get_block_header(&edh.bitcoin_block_hash) {
            Ok(header) => header,
            Err(e) => {
                return Err(BotanixConsensusPackageError::FailedToRetrieveBitcoinCheckpointHeader(e))
            }
        };

        let bitcoin_checkpoint_height = match bitcoind.get_block_info(&edh.bitcoin_block_hash) {
            Ok(info) => info.height,
            Err(e) => {
                return Err(BotanixConsensusPackageError::FailedToRetrieveBitcoinCheckpointHeight(e))
            }
        };

        let bitcoin_checkpoint: RecentHeader =
            (bitcoin_checkpoint_header, bitcoin_checkpoint_height as u32);

        Ok(BotanixConsensusPackage {
            bitcoin_checkpoint,
            aggregate_public_key: edh.aggregated_public_key,
            btc_network,
        })
    }

    /// Get aggregate public key
    fn get_aggregate_public_key(
        &self,
    ) -> Result<secp256k1::PublicKey, ExtraDataHeaderDeserializeError> {
        let edh = self.deserialize_extra_data_header()?;
        Ok(edh.aggregated_public_key)
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{
        block::{BlockHash, Header as BtcHeader, Version},
        hashes::Hash,
        CompactTarget, TxMerkleNode,
    };
    use reth_btc_wallet::{bitcoind::BitcoindConfig, test_utils::MockBitcoindFactory};

    use super::*;
    use crate::Header;

    #[test]
    fn deserialize_extension_trait() {
        let mut header = Header::default();
        let edh = ExtraDataHeader::default();
        let serialized = edh.serialize();
        header.extra_data = serialized.into();
        let deserialized_edh =
            header.deserialize_extra_data_header().expect("Deserialization passed");

        assert_eq!(deserialized_edh, edh);
    }

    #[test]
    fn test_botanix_consensus_package() {
        let mut header = Header::default();
        let edh = ExtraDataHeader::default();
        header.add_extra_data_header(&edh);
        let btc_network = bitcoin::Network::Testnet;
        let bitcoind_factory = MockBitcoindFactory::new(BitcoindConfig::default());

        let res = header.botanix_consensus_package(btc_network, bitcoind_factory);
        assert!(res.is_ok());

        let BotanixConsensusPackage { bitcoin_checkpoint, aggregate_public_key, btc_network } =
            res.unwrap();

        let expected_header = BtcHeader {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::from_slice(&[0; 32]).unwrap(),
            time: 0,
            bits: CompactTarget::from_consensus(0),
            nonce: 0,
        };

        assert_eq!(bitcoin_checkpoint.0, expected_header);
        assert_eq!(bitcoin_checkpoint.1, 0);
        assert_eq!(aggregate_public_key, edh.aggregated_public_key);
        assert_eq!(btc_network, bitcoin::Network::Testnet);
    }
}
