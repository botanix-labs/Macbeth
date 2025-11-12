use crate::{
    bitcoind::{BitcoindClient, BitcoindFactory, BitcoindRpc},
    error::BitcoindError,
};
use async_trait::async_trait;
use bitcoin::{
    address::NetworkUnchecked,
    block::{BlockHash, Header, Version},
    hashes::Hash,
    Address, Amount, CompactTarget, TxMerkleNode,
};
use bitcoincore_rpc::{
    json::{GetBlockResult, LoadWalletResult},
    Error as JsonRPCError,
};
use std::str::FromStr;
pub struct MockBitcoind;

impl MockBitcoind {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for MockBitcoind {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BitcoindRpc for MockBitcoind {
    async fn is_synced(&self) -> Result<bool, BitcoindError> {
        Ok(true)
    }

    async fn wait_until_synced(&self) {}

    fn get_best_block_hash_rpc(&self) -> Result<bitcoin::BlockHash, BitcoindError> {
        Ok(bitcoin::BlockHash::all_zeros())
    }

    fn get_block_header_rpc(
        &self,
        _h: &bitcoin::BlockHash,
    ) -> Result<bitcoin::blockdata::block::Header, BitcoindError> {
        Ok(Header {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::from_slice(&[0; 32]).unwrap(),
            time: 0,
            bits: CompactTarget::from_consensus(0),
            nonce: 0,
        })
    }

    fn get_block_hash_rpc(&self, _height: u64) -> Result<bitcoin::BlockHash, BitcoindError> {
        Ok(bitcoin::BlockHash::all_zeros())
    }

    fn get_block_info_rpc(&self, _h: &bitcoin::BlockHash) -> Result<GetBlockResult, BitcoindError> {
        let block_info_result = GetBlockResult {
            hash: BlockHash::all_zeros(),
            confirmations: 0,
            strippedsize: None,
            size: 0,
            weight: 0,
            height: 0,
            version: 0,
            version_hex: None,
            merkleroot: TxMerkleNode::from_slice(&[0; 32]).unwrap(),
            tx: vec![],
            time: 0,
            mediantime: None,
            nonce: 0,
            bits: String::from("foo"),
            difficulty: 0.0,
            chainwork: vec![],
            n_tx: 0,
            previousblockhash: None,
            nextblockhash: None,
        };
        Ok(block_info_result)
    }

    fn get_txids_rpc(&self, _h: &bitcoin::BlockHash) -> Result<Vec<bitcoin::Txid>, BitcoindError> {
        Ok(vec![])
    }

    fn get_estimate_smart_fee_rpc(
        &self,
    ) -> Result<crate::bitcoind::EstimateSmartFeeResult, BitcoindError> {
        Ok(crate::bitcoind::EstimateSmartFeeResult {
            fee_rate: Some(Amount::from_sat(1000)),
            errors: None,
            blocks: 1,
        })
    }
    fn get_block_count_rpc(&self) -> Result<u64, BitcoindError> {
        Ok(0)
    }
    fn load_wallet_rpc(&self, _wallet: &str) -> Result<LoadWalletResult, BitcoindError> {
        Ok(LoadWalletResult { name: String::from("mock_wallet"), warning: None })
    }

    fn get_new_address_rpc(
        &self,
        _label: Option<&str>,
        _address_type: Option<bitcoincore_rpc::json::AddressType>,
    ) -> Result<Address<NetworkUnchecked>, BitcoindError> {
        Ok(bitcoin::Address::from_str("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq").unwrap())
    }

    fn generate_to_address_rpc(
        &self,
        _blocks: u64,
        _address: &bitcoin::Address,
    ) -> Result<Vec<bitcoin::BlockHash>, BitcoindError> {
        Ok(vec![bitcoin::BlockHash::all_zeros()])
    }

    fn create_wallet_rpc(
        &self,
        _wallet: &str,
        _disable_private_keys: Option<bool>,
        _blank: Option<bool>,
        _passphrase: Option<&str>,
        _avoid_reuse: Option<bool>,
    ) -> Result<LoadWalletResult, BitcoindError> {
        Ok(LoadWalletResult { name: String::from("mock_wallet"), warning: None })
    }
}
#[derive(Debug, Clone)]
pub struct MockBitcoindFactory;
impl BitcoindFactory for MockBitcoindFactory {
    fn new(_config: crate::bitcoind::BitcoindConfig) -> Self {
        Self {}
    }

    fn build_and_connect(&self) -> Result<BitcoindClient, JsonRPCError> {
        Ok(BitcoindClient::new_boxed(Box::new(MockBitcoind::new())))
    }
}
