use async_trait::async_trait;
use bitcoin::{block::Header, consensus::deserialize, Txid};
use hex::FromHexError;
use std::str::FromStr;

#[derive(Debug)]
pub enum BlockSourceError {
    BlockHeaderRetrievalFailed,
    BlockTipRetrievalFailed,
    BlockHashRetrievalFailed,
    TransactionBroadcastFailed(String),
    HexDecodeFailed,
    BitcoinEncodingFailed,
    ReqwestError(reqwest::Error),
    ParsingIntError(std::num::ParseIntError),
    ParsingBlockSourceResponseError,
}

impl From<FromHexError> for BlockSourceError {
    fn from(_err: FromHexError) -> Self {
        BlockSourceError::HexDecodeFailed
    }
}

impl From<bitcoin::consensus::encode::Error> for BlockSourceError {
    fn from(_err: bitcoin::consensus::encode::Error) -> Self {
        BlockSourceError::BitcoinEncodingFailed
    }
}

impl From<reqwest::Error> for BlockSourceError {
    fn from(err: reqwest::Error) -> Self {
        BlockSourceError::ReqwestError(err)
    }
}

#[async_trait]
pub trait BlockSource {
    /// Hex encoded block hash
    async fn get_block_header(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Header, BlockSourceError>;
    /// Hex encoded raw tx
    async fn broadcast_tx(&self, raw_tx: &str) -> Result<Txid, BlockSourceError>;

    async fn get_tip(&self) -> Result<u32, BlockSourceError>;

    async fn get_txids(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Vec<bitcoin::Txid>, BlockSourceError>;

    async fn get_block_hash(&self, height: u32) -> Result<bitcoin::BlockHash, BlockSourceError>;
}

#[derive(Debug, Clone)]
pub struct MempoolSpace {
    url: String,
}

impl MempoolSpace {
    pub fn new(url: String) -> Self {
        MempoolSpace { url }
    }
}

#[async_trait]
impl BlockSource for MempoolSpace {
    async fn get_block_header(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Header, BlockSourceError> {
        let response = reqwest::get(format!("{}/block/{}/header", self.url, block_hash)).await?;

        if response.status().is_success() {
            let raw_response = response.text().await?;
            let mut header_bytes = [0u8; 80];
            hex::decode_to_slice(raw_response, &mut header_bytes as &mut [u8])?;
            let header: Header = deserialize(&header_bytes)?;
            return Ok(header)
        }
        Err(BlockSourceError::BlockHeaderRetrievalFailed)
    }
    async fn get_block_hash(&self, height: u32) -> Result<bitcoin::BlockHash, BlockSourceError> {
        let response = reqwest::get(format!("{}/block-height/{}", self.url, height)).await?;

        if response.status().is_success() {
            let raw_response = response.text().await?;
            let block_hash: bitcoin::BlockHash = match raw_response.parse() {
                Ok(hash) => hash,
                Err(_e) => return Err(BlockSourceError::ParsingBlockSourceResponseError),
            };
            return Ok(block_hash)
        }

        Err(BlockSourceError::BlockHashRetrievalFailed)
    }

    async fn get_tip(&self) -> Result<u32, BlockSourceError> {
        let response = reqwest::get(format!("{}/blocks/tip/height", self.url)).await?;

        if response.status().is_success() {
            let raw_response = response.text().await?;
            let height: u32 = raw_response.parse().map_err(BlockSourceError::ParsingIntError)?;
            return Ok(height)
        }

        Err(BlockSourceError::BlockTipRetrievalFailed)
    }

    async fn get_txids(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Vec<Txid>, BlockSourceError> {
        let response = reqwest::get(format!("{}/block/{}/txids", self.url, block_hash)).await?;

        if response.status().is_success() {
            let raw_response: Vec<String> = response.json().await.unwrap();
            let mut txids: Vec<Txid> = vec![];
            for txid in raw_response {
                txids.push(bitcoin::Txid::from_str(txid.as_str()).expect("valid txids"));
            }

            return Ok(txids)
        }
        Err(BlockSourceError::BlockHeaderRetrievalFailed)
    }

    async fn broadcast_tx(&self, raw_tx: &str) -> Result<Txid, BlockSourceError> {
        let client = reqwest::Client::new();
        let response =
            client.post(format!("{}/tx", self.url)).body(raw_tx.to_string()).send().await?;

        if response.status().is_success() {
            let raw_response = response.text().await?;
            let mut txid_bytes = [0u8; 32];
            hex::decode_to_slice(raw_response, &mut txid_bytes as &mut [u8])?;
            let tx_id: Txid = deserialize(&txid_bytes)?;
            return Ok(tx_id)
        }

        let error_message = response.text().await?;
        Err(BlockSourceError::TransactionBroadcastFailed(error_message))
    }
}
