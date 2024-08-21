use crate::bitcoind::{BitcoindError, BitcoindFactory, RpcApiExt};
use bitcoin::{
    block::{BlockHash, Header, Version},
    hashes::Hash,
    CompactTarget, TxMerkleNode,
};
use bitcoincore_rpc::{
    json::{self, GetBlockResult},
    jsonrpc::serde_json,
    Error as JsonRPCError,
};

pub struct MockBitcoind;
impl bitcoincore_rpc::RpcApi for MockBitcoind {
    fn call<T: for<'a> serde::de::Deserialize<'a>>(
        &self,
        _method: &str,
        _params: &[serde_json::Value],
    ) -> Result<T, bitcoincore_rpc::Error> {
        unimplemented!()
    }

    fn get_block_header(
        &self,
        _hash: &bitcoin::BlockHash,
    ) -> Result<bitcoin::block::Header, JsonRPCError> {
        let header = Header {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::from_slice(&[0; 32]).unwrap(),
            time: 0,
            bits: CompactTarget::from_consensus(0),
            nonce: 0,
        };
        Ok(header)
    }

    fn get_block_info(
        &self,
        _hash: &bitcoin::BlockHash,
    ) -> Result<json::GetBlockResult, JsonRPCError> {
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
}

impl MockBitcoind {
    pub fn new() -> Self {
        Self {}
    }
}

impl RpcApiExt for MockBitcoind {
    async fn is_synced(&self) -> Result<bool, BitcoindError> {
        Ok(true)
    }

    async fn wait_until_synced(&self) {}
}

#[derive(Debug, Clone)]
pub struct MockBitcoindFactory;
impl BitcoindFactory for MockBitcoindFactory {
    fn new(_config: crate::bitcoind::BitcoindConfig) -> Self {
        Self {}
    }

    fn build_and_connect(&self) -> Result<impl RpcApiExt, JsonRPCError> {
        Ok(MockBitcoind::new())
    }
}
