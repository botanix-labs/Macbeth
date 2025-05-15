use std::str::FromStr;

use crate::{botanix::utils::AmountExt, Address};
use bitcoin::{
    self,
    block::Header,
    consensus::{
        encode::{self as btcencode, Decodable},
        Encodable, ReadExt,
    },
    constants::COINBASE_MATURITY,
    merkle_tree::PartialMerkleTree,
    TxOut,
};
use btcserverlib::{
    pegout_id::PegoutId,
    wallet::address::{generate_taproot_scriptpubkey, generate_tweaked_public_key},
};
use ethers::types::U256;
use frost_secp256k1_tr as frost;
use revm_primitives::B256;
use secp256k1::PublicKey;
use thiserror::Error;
use tracing::info;

/// Version 0 of the pegin metadata format
pub const PEGIN_META_VERSION_V0: u32 = 0;
/// Version 1 of the pegin metadata format with reference block hash
pub const PEGIN_META_VERSION_V1: u32 = 1;
const _PEGOUT_META_VERSION: u32 = 0;

/// Standard bitcoin header size
const BITCOIN_HEADER_SIZE: usize = Header::SIZE; // 80 bytes

/// Bitcoin's difficulty adjustment period, a reasonable maximum
const MAX_BITCOIN_BLOCK_HEADERS: u64 = 2016;

/// Pegin data structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeginData {
    /// Account the pegin is sent from
    pub account: Address,
    /// Amount of the pegin denominated in wei
    pub amount: U256,
    /// Bitcoin block height the pegin is confirmed in
    pub bitcoin_block_height: u32,
    /// Pegin metadata
    pub meta: Vec<PeginMeta>,
}

impl PeginData {
    /// Validates multiple pegin proofs against the current bitcoin block hash
    /// Returns the aggregate value of all the pegin amounts
    pub fn validate(
        &self,
        bitcoin_commitment: &(bitcoin::block::Header, u32),
        aggregate_pk: &secp256k1::PublicKey,
    ) -> Result<U256, PeginDataError> {
        info!(self_btc_height = self.bitcoin_block_height, self_amount = %self.amount, num_meta_proofs = self.meta.len(),
              input_btc_checkpoint_hash = %bitcoin_commitment.0.block_hash(), input_btc_checkpoint_height = bitcoin_commitment.1,
              "PeginDataValidate: Starting validation");

        // the aggregate value from all the pegin proofs
        let mut aggregate_value = U256::from_str_radix("0", 10).expect("valid amount");
        let commit_hash = bitcoin_commitment.0.block_hash();
        for pegin in &self.meta {
            if ![PEGIN_META_VERSION_V0, PEGIN_META_VERSION_V1].contains(&pegin.version()) {
                return Err(PeginDataError::Invalid(
                    "invalid meta version: only accepting version 0 or 1",
                ));
            };

            let pegin = match pegin {
                PeginMeta::V0(meta) => meta,
                PeginMeta::V1(meta) => &meta.inner,
            };

            let pegin_version_for_log = pegin.version;
            let pegin_block_headers_len_for_log = pegin.block_headers.len();

            info!(self_btc_height = self.bitcoin_block_height, pegin_version = pegin_version_for_log,
                  current_commit_hash_for_check = %commit_hash, num_pegin_block_headers = pegin_block_headers_len_for_log,
                  "PeginDataValidate: About to check for recent block hash mismatch");

            // pegin block headers list should contain the commitment header
            if !pegin.block_headers.iter().any(|h| h.block_hash() == commit_hash) {
                return Err(PeginDataError::Invalid("recent block hash mismatch"));
            }

            // Then let's validate the merkle proof.
            let merkle = &pegin.merkle_proof;
            let mut txids = Vec::with_capacity(1);
            let mut idxs = Vec::with_capacity(1);
            let root = merkle.extract_matches(&mut txids, &mut idxs)?;
            if !txids.contains(&pegin.outpoint.txid) {
                return Err(PeginDataError::Invalid("invalid merkle proof: inclusion"));
            }
            // And check that the merkle proof is indeed for the first header provided.
            if pegin.block_headers[0].merkle_root != root {
                return Err(PeginDataError::Invalid("merkle proof and block header mismatch"));
            }

            // then check that the merkle proof was indeed for the pegin tx
            if pegin.tx.compute_txid() != pegin.outpoint.txid {
                return Err(PeginDataError::Invalid("invalid tx or outpoint: txid"));
            }
            if pegin.tx.output.len() < pegin.outpoint.vout as usize {
                return Err(PeginDataError::Invalid("invalid tx or outpoint: output idx"));
            }

            let encoded_pk = aggregate_pk.serialize();
            let vk = frost::VerifyingKey::deserialize(&encoded_pk)
                .map_err(PeginDataError::FrostError)?;
            let tpk = generate_tweaked_public_key(&vk, &self.account.into())
                .map_err(|_e| PeginDataError::InvalidTweak)?;
            let gateway_script = generate_taproot_scriptpubkey(&tpk);

            let output = &pegin.tx.output[pegin.outpoint.vout as usize];
            if gateway_script != output.script_pubkey {
                return Err(PeginDataError::Invalid("invalid script pubkey"));
            }

            let output_value = bitcoin::Amount::from_sat(output.value.to_sat()).to_wei();
            aggregate_value += output_value;

            // check that the user provided an actual valid block header sequence
            let mut iter = pegin.block_headers.iter().peekable();
            while let Some(header) = iter.next() {
                if let Some(next) = iter.peek() {
                    if next.prev_blockhash != header.block_hash() {
                        return Err(PeginDataError::Invalid("invalid block header sequence"));
                    }
                }
            }

            // calculate pegin txs block depth
            let diff = pegin
                .block_headers
                .iter()
                .rev()
                .skip_while(|h| h.block_hash() != commit_hash)
                .count() -
                1; // minus one for the commitment itself
                   // the latest block height minus the position of the user block in the list is the
                   // height of the user block
            if bitcoin_commitment.1 - (diff as u32) != self.bitcoin_block_height {
                return Err(PeginDataError::InvalidBitcoinBlockHeight);
            }

            info!(self_btc_height = self.bitcoin_block_height, calculated_diff = diff,
                  bitcoin_commitment_height = bitcoin_commitment.1,
                  derived_pegin_height_for_check = (bitcoin_commitment.1.saturating_sub(diff as u32)),
                  "PeginDataValidate: About to check bitcoin_block_height consistency (after diff calc)");

            // If any of the inputs are coinbase and the tx is not coinbase, return an error
            if pegin.tx.is_coinbase() && (diff as u32) < COINBASE_MATURITY {
                return Err(PeginDataError::Invalid("spending non-mature coinbase"));
            }
        }

        Ok(aggregate_value)
    }
}

/// Pegin metadata structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeginMetaV0 {
    /// Version of the pegin metadata
    pub version: u32,
    /// Merkle proof for the pegin tx
    pub merkle_proof: PartialMerkleTree,
    /// Outpoint of the pegin tx
    pub outpoint: bitcoin::OutPoint,
    /// final destination address of the pegin
    pub address: Address,
    /// Aggregate public key the funds were sent to
    pub aggregate_publickey: secp256k1::PublicKey,
    /// Bitcoin block headers starting with the block the pegin is confirmed in,
    /// going up until at least the mainchain commitment or beyond.
    /// NB We need to allow to go beyond because between the user crafting the tx and
    /// it getting confirmed, the commitment might update.
    pub block_headers: Vec<Header>,
    /// Pegin tx
    pub tx: bitcoin::Transaction,
}

impl PeginMetaV0 {
    /// Serialize a pegin meta
    pub fn serialize(&self) -> Result<Vec<u8>, PeginDataError> {
        let mut bytes = Vec::new();
        self.version.consensus_encode(&mut bytes)?;
        self.outpoint.consensus_encode(&mut bytes)?;
        bytes.extend_from_slice(self.address.0.as_slice());
        bytes.extend_from_slice(&self.aggregate_publickey.serialize());
        btcencode::VarInt(self.block_headers.len() as u64).consensus_encode(&mut bytes)?;
        for header in &self.block_headers {
            header.consensus_encode(&mut bytes)?;
        }
        self.merkle_proof.consensus_encode(&mut bytes)?;
        self.tx.consensus_encode(&mut bytes)?;

        Ok(bytes)
    }

    /// Deserialize a pegin meta
    pub fn deserialize(mut bytes: &[u8]) -> Result<(Self, usize), PeginDataError> {
        // bytes is a list of proofs
        let proofs_size = bytes.len();
        Ok((
            Self {
                version: <u32>::consensus_decode(&mut bytes)?,
                outpoint: Decodable::consensus_decode(&mut bytes)?,
                address: {
                    let mut address_slice = [0u8; 20];
                    bytes.read_slice(&mut address_slice)?;
                    Address::from_slice(&address_slice)
                },
                aggregate_publickey: {
                    // compressed schnorr public key
                    let mut pk_bytes = [0u8; 33];
                    bytes.read_slice(&mut pk_bytes)?;
                    PublicKey::from_slice(&pk_bytes).map_err(PeginDataError::InvalidPublicKey)?
                },
                block_headers: {
                    let len = btcencode::VarInt::consensus_decode(&mut bytes)?.0;

                    if len > MAX_BITCOIN_BLOCK_HEADERS {
                        return Err(PeginDataError::TooManyBlockHeaders(len));
                    }

                    if len as usize > bytes.len() / BITCOIN_HEADER_SIZE {
                        return Err(PeginDataError::InvalidLength {
                            claimed: len,
                            remaining_bytes: bytes.len(),
                        });
                    }
                    let mut block_headers = Vec::with_capacity(len as usize);
                    for _ in 0..len {
                        block_headers.push(Decodable::consensus_decode(&mut bytes)?);
                    }

                    block_headers
                },
                merkle_proof: PartialMerkleTree::consensus_decode(&mut bytes)?,
                tx: Decodable::consensus_decode(&mut bytes)?,
            },
            // list of proofs size - bytes left = proof size
            proofs_size - bytes.len(),
        ))
    }

    /// Get the txout for the pegin
    pub fn txout(&self) -> &TxOut {
        self.tx
            .output
            .get(self.outpoint.vout as usize)
            .expect("we check on creation that vout exists")
    }
}

/// Pegin metadata structure V1, extends V0 with reference block hash
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeginMetaV1 {
    /// Inner V0 pegin metadata
    pub inner: PeginMetaV0,
    /// Reference block hash
    pub ref_block_hash: B256,
}

impl PeginMetaV1 {
    /// Serialize a pegin meta
    pub fn serialize(&self) -> Result<Vec<u8>, PeginDataError> {
        let mut bytes = self.inner.serialize()?;
        self.ref_block_hash.consensus_encode(&mut bytes)?;
        Ok(bytes)
    }

    /// Deserialize a pegin meta
    pub fn deserialize(mut bytes: &[u8]) -> Result<(Self, usize), PeginDataError> {
        // bytes is a list of proofs
        let proofs_size = bytes.len();
        let (inner, inner_size) = PeginMetaV0::deserialize(bytes)?;
        bytes = &bytes[inner_size..];

        let ref_block_hash = {
            let mut hash = [0u8; 32];
            bytes.read_slice(&mut hash)?;
            B256::from_slice(&hash)
        };
        Ok((Self { inner, ref_block_hash }, proofs_size - bytes.len()))
    }

    /// Get the txout for the pegin
    pub fn txout(&self) -> &TxOut {
        self.inner.txout()
    }
}

/// Pegin metadata enum that can hold different versions of pegin metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeginMeta {
    /// Version 0 of pegin metadata
    V0(PeginMetaV0),
    /// Version 1 of pegin metadata with reference block hash
    V1(PeginMetaV1),
}

impl PeginMeta {
    /// Serialize the pegin metadata to bytes
    pub fn serialize(&self) -> Result<Vec<u8>, PeginDataError> {
        match self {
            Self::V0(meta) => meta.serialize(),
            Self::V1(meta) => meta.serialize(),
        }
    }

    /// Deserialize bytes into pegin metadata
    pub fn deserialize(bytes: &[u8]) -> Result<(Self, usize), PeginDataError> {
        // Read the version first
        let mut bytes_clone = bytes;
        let version = u32::consensus_decode(&mut bytes_clone)?;

        match version {
            PEGIN_META_VERSION_V0 => {
                let (meta, size) = PeginMetaV0::deserialize(bytes)?;
                Ok((Self::V0(meta), size))
            }
            PEGIN_META_VERSION_V1 => {
                let (meta, size) = PeginMetaV1::deserialize(bytes)?;
                Ok((Self::V1(meta), size))
            }
            _ => Err(PeginDataError::Invalid("Invalid pegin meta version")),
        }
    }

    /// Get the version of the pegin metadata
    pub const fn version(&self) -> u32 {
        match self {
            Self::V0(meta) => meta.version,
            Self::V1(meta) => meta.inner.version,
        }
    }

    /// Get the merkle proof from the pegin metadata
    pub const fn merkle_proof(&self) -> &PartialMerkleTree {
        match self {
            Self::V0(meta) => &meta.merkle_proof,
            Self::V1(meta) => &meta.inner.merkle_proof,
        }
    }

    /// Get the outpoint from the pegin metadata
    pub const fn outpoint(&self) -> &bitcoin::OutPoint {
        match self {
            Self::V0(meta) => &meta.outpoint,
            Self::V1(meta) => &meta.inner.outpoint,
        }
    }

    /// Get the address from the pegin metadata
    pub const fn address(&self) -> Address {
        match self {
            Self::V0(meta) => meta.address,
            Self::V1(meta) => meta.inner.address,
        }
    }

    /// Get the aggregate public key from the pegin metadata
    pub const fn aggregate_publickey(&self) -> secp256k1::PublicKey {
        match self {
            Self::V0(meta) => meta.aggregate_publickey,
            Self::V1(meta) => meta.inner.aggregate_publickey,
        }
    }

    /// Get the block headers from the pegin metadata
    pub const fn block_headers(&self) -> &Vec<Header> {
        match self {
            Self::V0(meta) => &meta.block_headers,
            Self::V1(meta) => &meta.inner.block_headers,
        }
    }

    /// Get the transaction from the pegin metadata
    pub const fn tx(&self) -> &bitcoin::Transaction {
        match self {
            Self::V0(meta) => &meta.tx,
            Self::V1(meta) => &meta.inner.tx,
        }
    }

    /// Get the reference block hash from the pegin metadata (V1 only)
    pub const fn ref_block_hash(&self) -> Option<B256> {
        match self {
            Self::V0(_) => None,
            Self::V1(meta) => Some(meta.ref_block_hash),
        }
    }

    /// Get the transaction output from the pegin metadata
    pub fn txout(&self) -> &TxOut {
        match self {
            Self::V0(meta) => meta.txout(),
            Self::V1(meta) => meta.inner.txout(),
        }
    }
}

/// Error type for pegin data
#[derive(Debug, Error)]
pub enum PeginDataError {
    /// Invalid data format
    #[error("invalid data format: {0}")]
    InvalidFormat(#[from] btcencode::Error),
    /// Invalid pegin proof
    #[error("invalid pegin proof: {0}")]
    Invalid(&'static str),
    /// Invalid public key format
    #[error("invalid public key format: {0}")]
    InvalidPublicKey(secp256k1::Error),
    /// Invalid bitcoin block height
    #[error("invalid bitcoin block height")]
    InvalidBitcoinBlockHeight,
    /// Invalid tweak: failed to tweak aggregate public key
    #[error("invalid tweak: failed to tweak aggregate public key")]
    InvalidTweak,
    /// Frost related error
    #[error("frost error: {0}")]
    FrostError(frost::Error),
    /// Claimed length of block headers is greater than what could fit in the remaining bytes of
    /// the message.
    #[error("invalid bitcoin block header. Claimed = {claimed}, remaining = {remaining_bytes}")]
    InvalidLength {
        /// the number of block headers claimed in the message
        claimed: u64,
        /// the actual number of bytes remaining in the message
        remaining_bytes: usize,
    },
    /// Invalid merkle block: failed to extract matching Txid's.
    #[error("invalid merkle block: {0}")]
    InvalidMerkleBlock(#[from] bitcoin::merkle_tree::MerkleBlockError),
    /// Error when the number of block headers exceeds the maximum allowable limit.
    #[error("too many bitcoin block headers {0}")]
    TooManyBlockHeaders(u64),
}

impl From<bitcoin::io::Error> for PeginDataError {
    fn from(err: bitcoin::io::Error) -> Self {
        // `bitcoin::io::Error` is converted to
        // `bitcoin::consensus::encode::Error::Io(_)`
        Self::InvalidFormat(err.into())
    }
}

/// Error type for pegout data
#[derive(Debug, Error)]
pub enum PegoutDataError {
    /// Invalid pegout proof
    #[error("invalid pegout proof")]
    Invalid(&'static str, ethers::types::U256),
}

/// Pegout data structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PegoutData {
    /// Amount to be pegged out
    pub amount: bitcoin::Amount,
    /// Destination address
    pub destination: bitcoin::Address,
    /// Network the pegout should be performed on
    pub network: bitcoin::Network,
}

/// Pegout with `PegoutId` data structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PegoutWithId {
    /// Pegout data
    pub data: PegoutData,
    /// Pegout id
    pub id: PegoutId,
}

impl PegoutData {
    /// Create a new pegout data
    pub fn new(
        amount: bitcoin::Amount,
        eth_amount: ethers::types::U256,
        address: String,
        btc_network: bitcoin::Network,
    ) -> Result<Self, PegoutDataError> {
        // Check for valid address
        let destination: bitcoin::address::Address<bitcoin::address::NetworkUnchecked> =
            bitcoin::address::Address::from_str(address.as_str())
                .map_err(|_e| PegoutDataError::Invalid("Invalid Bitcoin Address", eth_amount))?;

        // For is address if valid for network
        let network_checked_destination = destination
            .require_network(btc_network)
            .map_err(|_e| PegoutDataError::Invalid("Address not valid for network", eth_amount))?;

        Ok(Self { amount, destination: network_checked_destination, network: btc_network })
    }

    /// current version of the pegout data structure
    pub const fn version() -> u8 {
        0
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use bitcoin::{
        absolute::LockTime, block::Version, hash_types::TxMerkleNode, hashes::Hash, Amount,
        BlockHash, CompactTarget, OutPoint, Transaction, TxIn, TxOut, Txid,
    };
    use revm_primitives::hex;
    use secp256k1::PublicKey;

    fn create_test_pegin_meta(
        version: Option<u32>,
        block_headers: Option<Vec<Header>>,
        pk: &secp256k1::PublicKey,
        header_metadata: HeaderMetadata,
        destination_address: Address,
    ) -> PeginMeta {
        let meta_v0 = PeginMetaV0 {
            version: version.unwrap_or_default(),
            merkle_proof: header_metadata.merkle_proof,
            outpoint: header_metadata.outpoint,
            address: destination_address,
            aggregate_publickey: *pk,
            block_headers: if let Some(block_headers) = block_headers {
                block_headers
            } else {
                vec![header_metadata.header]
            },
            tx: header_metadata.tx,
        };
        match meta_v0.version {
            PEGIN_META_VERSION_V1 => {
                PeginMeta::V1(PeginMetaV1 { inner: meta_v0, ref_block_hash: B256::random() })
            }
            _ => PeginMeta::V0(meta_v0),
        }
    }

    #[test]
    fn serialize_pegin_metadata_v0() {
        let pk = random_pk();
        let header_metadata = create_header_metadata(None, &pk);
        let destination_address =
            Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap();

        let pegin_metadata =
            create_test_pegin_meta(Some(0_u32), None, &pk, header_metadata, destination_address);

        let serialized = pegin_metadata.serialize().unwrap();
        let (deserialized, size) = PeginMetaV0::deserialize(&serialized).unwrap();
        assert_eq!(pegin_metadata.version(), deserialized.version);
        assert_eq!(pegin_metadata.outpoint(), &deserialized.outpoint);
        assert_eq!(pegin_metadata.address(), deserialized.address);
        assert_eq!(pegin_metadata.aggregate_publickey(), deserialized.aggregate_publickey);
        assert_eq!(pegin_metadata.block_headers().len(), deserialized.block_headers.len());
        assert_eq!(pegin_metadata.tx(), &deserialized.tx);
        assert_eq!(
            pegin_metadata.merkle_proof().num_transactions(),
            deserialized.merkle_proof.num_transactions()
        );
        assert_eq!(serialized.len(), size);
    }

    #[test]
    fn serialize_pegin_metadata_v1() {
        let pk = random_pk();
        let header_metadata = create_header_metadata(None, &pk);
        let destination_address =
            Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap();

        let pegin_metadata =
            create_test_pegin_meta(Some(1_u32), None, &pk, header_metadata, destination_address);
        let serialized = pegin_metadata.serialize().unwrap();
        let (deserialized, size) = PeginMetaV1::deserialize(&serialized).unwrap();
        assert_eq!(pegin_metadata.version(), deserialized.inner.version);
        assert_eq!(pegin_metadata.outpoint(), &deserialized.inner.outpoint);
        assert_eq!(pegin_metadata.address(), deserialized.inner.address);
        assert_eq!(pegin_metadata.aggregate_publickey(), deserialized.inner.aggregate_publickey);
        assert_eq!(pegin_metadata.block_headers().len(), deserialized.inner.block_headers.len());
        assert_eq!(pegin_metadata.tx(), &deserialized.inner.tx);
        assert_eq!(
            pegin_metadata.merkle_proof().num_transactions(),
            deserialized.inner.merkle_proof.num_transactions()
        );
        assert_eq!(serialized.len(), size);
        assert_eq!(pegin_metadata.ref_block_hash().unwrap(), deserialized.ref_block_hash);
    }

    #[test]
    fn deserialize_pegin_metadata_v0() {
        // Proof generated by side-car service
        let pegin_metadata_vec = hex::decode("000000002e5523bcd1b329e8a1a66b7d31719e94a33483eae77f5a677e6634d84ce55f470000000014194f42f33a9b3d5fe9e7ba8501be24d00b07b50376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762010080732aa97865f6b4be36ba861d397401e956d23e129940bb8a03000000000000000000b0d5ec7a0f49793b896db8f4a2cb4ec37e6b2dbd8e90e23d23f860abc9a76b70f1d2ba6494380517307685f90e00000005ce88336dc1340fed95a5f334a536b459d9af3aa3f44eb7b64de3a75e01812f021403c7f4a599f74775069cd3e3e589456fd672037ee1c2c9570f6776fb2e864a3e06d4e3858fdfa9f987053290aac66ef9b7c28fcaf3d3d64724d65a5fc11a2365557cde0d0e465dbfa1f2730617416133b2347ca5170fc1c0dd86f019356acf331d671f862a2841476864ef8639511b9e6f29c25b3c150680b626f9c185be81022f000100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();
        let (meta, size) = PeginMetaV0::deserialize(pegin_metadata_vec.as_slice()).unwrap();
        println!("meta {:?}", meta);
        println!("meta usize {:?}", size);
        assert_eq!(meta.version, PEGIN_META_VERSION_V0);
        assert_eq!(meta.merkle_proof.num_transactions(), 14);
        assert_eq!(
            meta.address.0.as_slice(),
            hex::decode("14194f42f33a9b3d5fe9e7ba8501be24d00b07b5").unwrap()
        );
        assert_eq!(
            meta.aggregate_publickey,
            PublicKey::from_str(
                "0376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762"
            )
            .unwrap()
        );

        assert_eq!(meta.block_headers.len(), 1);
    }

    #[test]
    fn deserialize_pegin_metadata_v1() {
        // Create V1 pegin metadata with ref_block_hash
        let mut pegin_metadata_vec = hex::decode("010000002e5523bcd1b329e8a1a66b7d31719e94a33483eae77f5a677e6634d84ce55f470000000114194f42f33a9b3d5fe9e7ba8501be24d00b07b50376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762010080732aa97865f6b4be36ba861d397401e956d23e129940bb8a03000000000000000000b0d5ec7a0f49793b896db8f4a2cb4ec37e6b2dbd8e90e23d23f860abc9a76b70f1d2ba6494380517307685f90e00000005ce88336dc1340fed95a5f334a536b459d9af3aa3f44eb7b64de3a75e01812f021403c7f4a599f74775069cd3e3e589456fd672037ee1c2c9570f6776fb2e864a3e06d4e3858fdfa9f987053290aac66ef9b7c28fcaf3d3d64724d65a5fc11a2365557cde0d0e465dbfa1f2730617416133b2347ca5170fc1c0dd86f019356acf331d671f862a2841476864ef8639511b9e6f29c25b3c150680b626f9c185be81022f000100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();

        // Add ref_block_hash (32 bytes of zeros) to the end
        pegin_metadata_vec.extend_from_slice(&[0; 32]);

        let (meta, size) = PeginMetaV1::deserialize(pegin_metadata_vec.as_slice()).unwrap();
        println!("meta {:?}", meta);
        println!("meta usize {:?}", size);

        assert_eq!(meta.inner.version, PEGIN_META_VERSION_V1);
        assert_eq!(meta.inner.merkle_proof.num_transactions(), 14);
        assert_eq!(
            meta.inner.address.0.as_slice(),
            hex::decode("14194f42f33a9b3d5fe9e7ba8501be24d00b07b5").unwrap()
        );
        assert_eq!(
            meta.inner.aggregate_publickey,
            PublicKey::from_str(
                "0376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762"
            )
            .unwrap()
        );

        assert_eq!(meta.inner.block_headers.len(), 1);
        assert_eq!(meta.ref_block_hash, B256::from_slice(&[0; 32]));
    }

    #[test]
    fn deserialize_pmt() {
        let pmt_hex = hex::decode("0e00000005ce88336dc1340fed95a5f334a536b459d9af3aa3f44eb7b64de3a75e01812f021403c7f4a599f74775069cd3e3e589456fd672037ee1c2c9570f6776fb2e864a3e06d4e3858fdfa9f987053290aac66ef9b7c28fcaf3d3d64724d65a5fc11a2365557cde0d0e465dbfa1f2730617416133b2347ca5170fc1c0dd86f019356acf331d671f862a2841476864ef8639511b9e6f29c25b3c150680b626f9c185be81022f00").unwrap();
        let pmt = PartialMerkleTree::consensus_decode(&mut pmt_hex.as_slice()).unwrap();
        assert_eq!(pmt.num_transactions(), 14);
        let mut txids = Vec::with_capacity(14);
        pmt.extract_matches(&mut txids, &mut Vec::new()).unwrap();

        println!("txids {:?}", txids);
    }

    // validate() tests and setup
    struct HeaderMetadata {
        header: Header,
        merkle_proof: PartialMerkleTree,
        outpoint: OutPoint,
        tx: Transaction,
    }

    fn create_header_metadata(nonce: Option<u32>, pk: &secp256k1::PublicKey) -> HeaderMetadata {
        // create dummy transaction -- spending utxo that doesn't exist
        let temp_outpoint: OutPoint = OutPoint {
            txid: bitcoin::Txid::from_str(
                "9c6ee70c67738bb63bddc4a15de391cc13f950cb8715de75b508f52efe6d88bb",
            )
            .unwrap(),
            vout: 0_u32,
        };
        let tx_in = TxIn {
            previous_output: temp_outpoint,
            sequence: bitcoin::Sequence::MAX,
            script_sig: bitcoin::ScriptBuf::new(),
            witness: Default::default(),
        };

        let account = Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap();
        let pk_encoded = pk.serialize();
        let vk = frost::VerifyingKey::deserialize(&pk_encoded).unwrap();
        let tpk = generate_tweaked_public_key(&vk, &account.into()).unwrap();
        let gateway_script = generate_taproot_scriptpubkey(&tpk);

        let tx_out = TxOut { value: Amount::from_sat(100), script_pubkey: gateway_script };
        let tx: Transaction = Transaction {
            version: bitcoin::transaction::Version(1_i32),
            lock_time: LockTime::from_str("0").unwrap(),
            input: vec![tx_in],
            output: vec![tx_out],
        };

        let outpoint = OutPoint { txid: tx.compute_txid(), vout: 0 };

        let txids = vec![
            // Another random txid
            Txid::from_str("4fccd63b48697a66ae4155b183f7595694354def0345ac4b950a5765a7b90526")
                .expect("valid txid"),
            tx.compute_txid(),
        ];
        let mut tx_matches = vec![txids[1]];
        let mut vouts = vec![0];
        let merkle_proof = {
            let matches = vec![false, true];
            PartialMerkleTree::from_txids(&txids, &matches)
        };
        let merkle_root = merkle_proof.extract_matches(&mut tx_matches, &mut vouts).unwrap();

        let header = Header {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root,
            time: 0_u32,
            bits: CompactTarget::from_consensus(0),
            nonce: nonce.unwrap_or_default(),
        };

        HeaderMetadata { header, merkle_proof, outpoint, tx }
    }

    fn pegin_data_setup(
        version: Option<u32>,
        block_headers: Option<Vec<Header>>,
        pk: &secp256k1::PublicKey,
    ) -> PeginData {
        let header_metadata = create_header_metadata(None, pk);
        let destination_address =
            Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap();

        let meta = create_test_pegin_meta(
            version,
            block_headers,
            pk,
            header_metadata,
            destination_address,
        );

        PeginData {
            account: destination_address,
            // 100 sats converted to wei
            amount: U256::from_str_radix("1000000000000", 10).unwrap(),
            bitcoin_block_height: 1_u32,
            meta: vec![meta],
        }
    }

    fn random_pk() -> secp256k1::PublicKey {
        let secp = secp256k1::Secp256k1::new();
        let mut rng = rand::thread_rng();
        secp256k1::PublicKey::from_secret_key(&secp, &secp256k1::SecretKey::new(&mut rng))
    }

    #[test]
    fn validate_pegin_data() {
        let pk = random_pk();
        let pegin_data = pegin_data_setup(None, None, &pk);
        let header = pegin_data.meta.first().unwrap().block_headers().first().unwrap();

        let aggregate_amount = pegin_data.validate(&(*header, 1_u32), &pk).expect("valid");
        assert_eq!(aggregate_amount, pegin_data.amount);
    }

    #[test]
    #[should_panic(expected = "invalid meta version: only accepting version 0 or 1")]
    fn validate_pegin_data_with_incorrect_version() {
        let pk = random_pk();
        let pegin_data = pegin_data_setup(Some(2_u32), None, &pk);
        let header = pegin_data.meta.first().unwrap().block_headers().first().unwrap();

        pegin_data.validate(&(*header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "recent block hash mismatch")]
    fn validate_pegin_data_without_headers() {
        let pk = random_pk();
        let pegin_data = pegin_data_setup(None, Some(Vec::new()), &pk);
        let header = create_header_metadata(None, &pk).header;

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "recent block hash mismatch")]
    fn validate_pegin_data_with_incorrect_block_hash() {
        let pk = random_pk();
        let pegin_data = pegin_data_setup(None, None, &pk);
        let header = create_header_metadata(Some(1_u32), &pk).header;

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid merkle proof: inclusion")]
    fn validate_pegin_data_with_invalid_merkle_proof() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        let different_txid = bitcoin::Txid::all_zeros();
        let different_txids = vec![different_txid];
        let matches = vec![true];

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => {
                meta.merkle_proof = PartialMerkleTree::from_txids(&different_txids, &matches)
            }
            PeginMeta::V1(meta) => {
                meta.inner.merkle_proof = PartialMerkleTree::from_txids(&different_txids, &matches)
            }
        };

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid merkle proof: inclusion")]
    fn validate_pegin_data_with_invalid_outpoint() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => meta.outpoint.txid = bitcoin::Txid::all_zeros(),
            PeginMeta::V1(meta) => meta.inner.outpoint.txid = bitcoin::Txid::all_zeros(),
        };

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "merkle proof and block header mismatch")]
    fn validate_pegin_data_with_mismatched_merkle_root() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => meta.block_headers[0].merkle_root = TxMerkleNode::all_zeros(),
            PeginMeta::V1(meta) => {
                meta.inner.block_headers[0].merkle_root = TxMerkleNode::all_zeros()
            }
        };

        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();
        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "merkle proof and block header mismatch")]
    fn validate_pegin_data_with_same_txid_different_root() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        let original_txid = pegin_data.meta.first_mut().unwrap().outpoint().txid;

        let txids = vec![original_txid];
        let matches = vec![true];
        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => {
                meta.merkle_proof = PartialMerkleTree::from_txids(&txids, &matches)
            }
            PeginMeta::V1(meta) => {
                meta.inner.merkle_proof = PartialMerkleTree::from_txids(&txids, &matches)
            }
        };

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid tx or outpoint: txid")]
    fn validate_pegin_data_with_invalid_tx() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => meta.tx.version = bitcoin::transaction::Version(999),
            PeginMeta::V1(meta) => meta.inner.tx.version = bitcoin::transaction::Version(999),
        };

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid tx or outpoint: output idx")]
    fn validate_pegin_data_with_invalid_outpoint_vout() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => meta.outpoint.vout = 2,
            PeginMeta::V1(meta) => meta.inner.outpoint.vout = 2,
        };

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid script pubkey")]
    fn validate_pegin_data_with_invalid_script_pubkey() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => meta.tx.output[0].script_pubkey = bitcoin::ScriptBuf::new(),
            PeginMeta::V1(meta) => {
                meta.inner.tx.output[0].script_pubkey = bitcoin::ScriptBuf::new()
            }
        };

        let new_txid = pegin_data.meta.first_mut().unwrap().tx().compute_txid();

        let txids = vec![new_txid];
        let matches = vec![true];
        let merkle_proof = PartialMerkleTree::from_txids(&txids, &matches);

        let mut txids = Vec::with_capacity(1);
        let mut idxs = Vec::with_capacity(1);
        let root = merkle_proof.extract_matches(&mut txids, &mut idxs).unwrap();

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => {
                meta.block_headers[0].merkle_root = root;
                meta.merkle_proof = merkle_proof;
                meta.outpoint.txid = new_txid;
            }
            PeginMeta::V1(meta) => {
                meta.inner.block_headers[0].merkle_root = root;
                meta.inner.merkle_proof = merkle_proof;
                meta.inner.outpoint.txid = new_txid;
            }
        }

        let modified_header =
            *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();
        pegin_data.validate(&(modified_header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid script pubkey")]
    fn validate_pegin_data_with_different_account() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        pegin_data.account = Address::with_last_byte(1);

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid script pubkey")]
    fn validate_pegin_data_with_different_pubkey() {
        let pk = random_pk();
        let pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first().unwrap().block_headers().first().unwrap();
        let different_pk = random_pk();

        pegin_data.validate(&(header, 1_u32), &different_pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid block header sequence")]
    fn validate_pegin_data_with_invalid_block_sequence() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first().unwrap().block_headers().first().unwrap();

        let second_header = bitcoin::block::Header {
            version: header.version,
            prev_blockhash: header.block_hash(),
            merkle_root: header.merkle_root,
            time: header.time + 1,
            bits: header.bits,
            nonce: header.nonce,
        };
        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => {
                meta.block_headers.push(second_header);
                meta.block_headers[1].prev_blockhash = bitcoin::BlockHash::all_zeros();
            }
            PeginMeta::V1(meta) => {
                meta.inner.block_headers.push(second_header);
                meta.inner.block_headers[1].prev_blockhash = bitcoin::BlockHash::all_zeros();
            }
        };

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid block header sequence")]
    fn validate_pegin_data_with_broken_block_chain_in_middle() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let first_header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        let second_header = bitcoin::block::Header {
            version: first_header.version,
            prev_blockhash: first_header.block_hash(),
            merkle_root: first_header.merkle_root,
            time: first_header.time + 1,
            bits: first_header.bits,
            nonce: first_header.nonce,
        };

        let third_header = bitcoin::block::Header {
            version: first_header.version,
            prev_blockhash: first_header.block_hash(),
            merkle_root: first_header.merkle_root,
            time: first_header.time + 2,
            bits: first_header.bits,
            nonce: first_header.nonce,
        };

        match pegin_data.meta.first_mut().unwrap() {
            PeginMeta::V0(meta) => {
                meta.block_headers.push(second_header);
                meta.block_headers.push(third_header);
            }
            PeginMeta::V1(meta) => {
                meta.inner.block_headers.push(second_header);
                meta.inner.block_headers.push(third_header);
            }
        }

        pegin_data.validate(&(first_header, 1_u32), &pk).unwrap();
    }

    #[test]
    fn validate_pegin_data_with_invalid_block_height() {
        let pk = random_pk();
        let mut pegin_data = pegin_data_setup(None, None, &pk);
        let header = *pegin_data.meta.first_mut().unwrap().block_headers().first().unwrap();

        pegin_data.bitcoin_block_height = 999;

        assert!(matches!(
            pegin_data.validate(&(header, 1_u32), &pk),
            Err(PeginDataError::InvalidBitcoinBlockHeight)
        ));
    }

    #[test]
    fn validate_coinbase_maturity() {
        let pk = random_pk();
        let destination_address = Address::random();
        let coinbase_tx_in = TxIn {
            previous_output: OutPoint::null(),
            sequence: bitcoin::Sequence::MAX,
            script_sig: bitcoin::ScriptBuf::new(),
            witness: Default::default(),
        };

        let pk_encoded = pk.serialize();
        let vk = frost::VerifyingKey::deserialize(&pk_encoded).unwrap();
        let tpk = generate_tweaked_public_key(&vk, &destination_address.into()).unwrap();
        let gateway_script = generate_taproot_scriptpubkey(&tpk);

        let tx_out = TxOut { value: Amount::from_sat(100), script_pubkey: gateway_script };
        let tx: Transaction = Transaction {
            version: bitcoin::transaction::Version(1_i32),
            lock_time: LockTime::ZERO,
            input: vec![coinbase_tx_in],
            output: vec![tx_out],
        };
        let outpoint = OutPoint { txid: tx.compute_txid(), vout: 0 };
        let txids = vec![
            // Another random txid
            Txid::from_str("4fccd63b48697a66ae4155b183f7595694354def0345ac4b950a5765a7b90526")
                .expect("valid txid"),
            tx.compute_txid(),
        ];
        let mut tx_matches = vec![txids[1]];
        let mut vouts = vec![0];
        let merkle_proof = {
            let matches = vec![false, true];
            PartialMerkleTree::from_txids(&txids, &matches)
        };
        let merkle_root = merkle_proof.extract_matches(&mut tx_matches, &mut vouts).unwrap();
        let header = Header {
            version: Version::default(),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root,
            time: 0_u32,
            bits: CompactTarget::from_consensus(0),
            nonce: 0_u32,
        };

        let meta = PeginMetaV0 {
            version: PEGIN_META_VERSION_V0,
            merkle_proof: merkle_proof.clone(),
            outpoint,
            address: destination_address,
            aggregate_publickey: pk,
            block_headers: vec![header],
            tx: tx.clone(),
        };

        let amount = U256::from_str_radix("1000000000000", 10).unwrap();

        let pegin_data = PeginData {
            account: destination_address,
            amount,
            bitcoin_block_height: 1_u32,
            meta: vec![PeginMeta::V0(meta)],
        };

        let res = pegin_data.validate(&(header, 1_u32), &pk).unwrap_err();
        assert!(matches!(res, PeginDataError::Invalid("spending non-mature coinbase")));

        // Create a chain of 100 blocks with the coinbase tx in the last 100 blocks
        let mut headers = vec![header];
        for i in 1..101 {
            let mut header = create_header_metadata(None, &pk).header;
            header.prev_blockhash = headers[i - 1].block_hash();
            headers.push(header);
        }

        let meta = PeginMetaV0 {
            version: PEGIN_META_VERSION_V0,
            merkle_proof,
            outpoint,
            address: destination_address,
            aggregate_publickey: pk,
            block_headers: headers.clone(),
            tx,
        };

        let pegin_data = PeginData {
            account: destination_address,
            amount,
            bitcoin_block_height: 0_u32,
            meta: vec![PeginMeta::V0(meta)],
        };

        let res = pegin_data.validate(&(headers[100], 100_u32), &pk).unwrap();
        assert_eq!(res, amount);
    }
}
