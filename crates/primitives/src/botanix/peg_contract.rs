use core::fmt::Debug;
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

/// Version 0 of the pegin metadata format
pub const PEGIN_META_VERSION_V0: u32 = 0;
/// Version 1 of the pegin metadata format with reference block hash
pub const PEGIN_META_VERSION_V1: u32 = 1;
const _PEGOUT_META_VERSION: u32 = 0;

/// Pegin data structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeginData {
    /// Account the pegin is sent from
    pub account: Address,
    /// Amount of the pegin denominated in wei
    pub amount: U256,
    /// Bitcoin block height the pegin is confirmed in
    pub bitcoin_block_height: u32,
}

/// Deserializes pegin metadata from bytes
/// 
/// Returns a boxed trait object implementing `PeginMetaTrait` and the number of bytes consumed
pub fn deserialize_meta(mut bytes: &[u8]) -> Result<(Box<dyn PeginMetaTrait>, usize), PeginDataError> {
    let proofs_size = bytes.len();
    let meta = PeginMeta {
        version: <u32>::consensus_decode(&mut bytes)?,
        outpoint: Decodable::consensus_decode(&mut bytes)?,
        address: {
            let mut address_slice = [0u8; 20];
            bytes.read_slice(&mut address_slice)?;
            Address::from_slice(&address_slice)
        },
        aggregate_public_key: {
            // compressed schnorr public key
            let mut pk_bytes = [0u8; 33];
            bytes.read_slice(&mut pk_bytes)?;
            PublicKey::from_slice(&pk_bytes).map_err(PeginDataError::InvalidPublicKey)?
        },
        block_headers: {
            let len = btcencode::VarInt::consensus_decode(&mut bytes)?.0;
            let mut ret = Vec::with_capacity(len as usize);
            for _ in 0..len {
                ret.push(Decodable::consensus_decode(&mut bytes)?);
            }

            ret
        },
        merkle_proof: PartialMerkleTree::consensus_decode(&mut bytes)?,
        tx: Decodable::consensus_decode(&mut bytes)?,
    };
    match meta.version {
        PEGIN_META_VERSION_V0 => Ok((Box::new(meta) as Box<dyn PeginMetaTrait>, proofs_size - bytes.len())),
        PEGIN_META_VERSION_V1 => {
            let meta_v1 = PeginMetaV1 {
                inner: meta,
                ref_block_hash: {
                    let mut hash = [0u8; 32];
                    bytes.read_slice(&mut hash)?;
                    B256::from_slice(&hash)
                },
            };
            Ok((Box::new(meta_v1) as Box<dyn PeginMetaTrait>, proofs_size - bytes.len()))
        },
        _ => {
            Err(PeginDataError::Invalid("Invalid meta format"))
        },
    }
}

/// Trait for handling pegin metadata operations including serialization,
/// deserialization, and validation of Bitcoin pegins.
pub trait PeginMetaTrait: Debug + 'static + Send + Sync {
    /// Serialize a pegin meta
    fn serialize(&self) -> Result<Vec<u8>, PeginDataError>;

    /// Validates a pegin proof against the current bitcoin block hash
    /// Returns the value of the pegin
    fn validate(
        &self,
        bitcoin_commitment: &(bitcoin::block::Header, u32),
        aggregate_pk: &secp256k1::PublicKey,
        pegin_data: PeginData,
    ) -> Result<U256, PeginDataError>;

    /// Converts the complete pegin metadata to a minimal essential representation
    /// containing only the data needed for transaction records
    fn to_essential(&self) -> EssentialPeginData;

    /// For equality comparison - returns true if self equals other
    fn clone_box(&self) -> Box<dyn PeginMetaTrait>;

    /// For equality comparison
    fn equals(&self, other: &dyn PeginMetaTrait) -> bool;
    
    /// Type tag for type identification (simple integer instead of `TypeId`)
    fn type_tag(&self) -> u32;

    /// Returns a reference to the trait object as a type-erased `&dyn Any`
    /// This allows for downcasting to concrete types with `downcast_ref`
    fn as_any(&self) -> &dyn std::any::Any;
}

impl Clone for Box<dyn PeginMetaTrait> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl PartialEq for Box<dyn PeginMetaTrait> {
    fn eq(&self, other: &Self) -> bool {
        // Objects are equal if they have the same type tag and the equals method returns true
        self.type_tag() == other.type_tag() && self.equals(other.as_ref())
    }
}

impl Eq for Box<dyn PeginMetaTrait> {}

/// Pegin metadata structure
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeginMeta {
    /// Version of the pegin metadata
    pub version: u32,
    /// Merkle proof for the pegin tx
    pub merkle_proof: PartialMerkleTree,
    /// Outpoint of the pegin tx
    pub outpoint: bitcoin::OutPoint,
    /// final destination address of the pegin
    pub address: Address,
    /// Aggregate public key the funds were sent to
    pub aggregate_public_key: secp256k1::PublicKey,
    /// Bitcoin block headers starting with the block the pegin is confirmed in,
    /// going up until at least the mainchain commitment or beyond.
    /// NB We need to allow to go beyond because between the user crafting the tx and
    /// it getting confirmed, the commitment might update.
    pub block_headers: Vec<Header>,
    /// Pegin tx
    pub tx: bitcoin::Transaction,
}

impl PeginMetaTrait for PeginMeta {
    fn serialize(&self) -> Result<Vec<u8>, PeginDataError> {
        let mut bytes = Vec::new();
        self.version.consensus_encode(&mut bytes)?;
        self.outpoint.consensus_encode(&mut bytes)?;
        bytes.extend_from_slice(self.address.0.as_slice());
        bytes.extend_from_slice(&self.aggregate_public_key.serialize());
        btcencode::VarInt(self.block_headers.len() as u64).consensus_encode(&mut bytes)?;
        for header in &self.block_headers {
            header.consensus_encode(&mut bytes)?;
        }
        self.merkle_proof.consensus_encode(&mut bytes)?;
        self.tx.consensus_encode(&mut bytes)?;

        Ok(bytes)
    }

    fn validate(
        &self,
        bitcoin_commitment: &(bitcoin::block::Header, u32),
        aggregate_pk: &secp256k1::PublicKey,
        pegin_data: PeginData,
    ) -> Result<U256, PeginDataError> {
        let commit_hash = bitcoin_commitment.0.block_hash();

        // pegin block headers list should contain the commitment header
        if !&self.block_headers.iter().any(|h| h.block_hash() == commit_hash) {
            return Err(PeginDataError::Invalid("recent block hash mismatch"));
        }

        // Then let's validate the merkle proof.
        let merkle = &self.merkle_proof;
        let mut txids = Vec::with_capacity(1);
        let mut idxs = Vec::with_capacity(1);
        let root = merkle.extract_matches(&mut txids, &mut idxs).unwrap();
        if !txids.contains(&self.outpoint.txid) {
            return Err(PeginDataError::Invalid("invalid merkle proof: inclusion"));
        }

        // And check that the merkle proof is indeed for the first header provided.
        if self.block_headers[0].merkle_root != root {
            return Err(PeginDataError::Invalid("merkle proof and block header mismatch"));
        }

        // then check that the merkle proof was indeed for the pegin tx
        if self.tx.compute_txid() != self.outpoint.txid {
            return Err(PeginDataError::Invalid("invalid tx or outpoint: txid"));
        }

        if self.tx.output.len() < self.outpoint.vout as usize {
            return Err(PeginDataError::Invalid("invalid tx or outpoint: output idx"));
        }

        let encoded_pk = aggregate_pk.serialize();
        let vk = frost::VerifyingKey::deserialize(&encoded_pk)
            .map_err(PeginDataError::FrostError)?;
        let tpk = generate_tweaked_public_key(&vk, &pegin_data.account.into())
            .map_err(|_e| PeginDataError::InvalidTweak())?;
        let gateway_script = generate_taproot_scriptpubkey(&tpk);

        let output = &self.tx.output[self.outpoint.vout as usize];
        if gateway_script != output.script_pubkey {
            return Err(PeginDataError::Invalid("invalid script pubkey"));
        }

        let output_value = bitcoin::Amount::from_sat(output.value.to_sat()).to_wei();

        // check that the user provided an actual valid block header sequence
        let headers = &self.block_headers;
        let mut iter = headers.iter().peekable();
        while let Some(header) = iter.next() {
            if let Some(next) = iter.peek() {
                if next.prev_blockhash != header.block_hash() {
                    return Err(PeginDataError::Invalid("invalid block header sequence"));
                }
            }
        }

        // calculate pegin txs block depth
        let diff = self
            .block_headers
            .iter()
            .rev()
            .skip_while(|h| h.block_hash() != commit_hash)
            .count() -
            1; // minus one for the commitment itself
            // the latest block height minus the position of the user block in the list is the
            // height of the user block
        if bitcoin_commitment.1 - (diff as u32) != pegin_data.bitcoin_block_height {
            return Err(PeginDataError::InvalidBitcoinBlockHeight);
        }

        // If any of the inputs are coinbase and the tx is not coinbase, return an error
        if self.tx.is_coinbase() && (diff as u32) < COINBASE_MATURITY {
            return Err(PeginDataError::Invalid("spending non-mature coinbase"));
        }

        Ok(output_value)
    }

    fn to_essential(&self) -> EssentialPeginData {
        EssentialPeginData {
            outpoint: self.outpoint,
            address: self.address,
            tx: self.tx.clone(),
        }
    }

    fn clone_box(&self) -> Box<dyn PeginMetaTrait> {
        Box::new(self.clone())
    }

    fn equals(&self, other: &dyn PeginMetaTrait) -> bool {
        // Only compare if the other object has the same type tag
        if other.type_tag() != self.type_tag() {
            return false;
        }

        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.version == other.version &&
            self.address == other.address &&
            self.aggregate_public_key == other.aggregate_public_key &&
            self.merkle_proof == other.merkle_proof &&
            self.block_headers == other.block_headers &&
            self.tx == other.tx &&
            self.outpoint == other.outpoint
        } else {
            false
        }
    }
    
    fn type_tag(&self) -> u32 {
        // A unique identifier for this type
        PEGIN_META_VERSION_V0
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Pegin metadata structure V1, extends V0 with reference block hash
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeginMetaV1 {
    /// Inner V0 pegin metadata
    pub inner: PeginMeta,
    /// Reference block hash
    pub ref_block_hash: B256,
}

impl PeginMetaTrait for PeginMetaV1 {
    fn serialize(&self) -> Result<Vec<u8>, PeginDataError> {
        let mut bytes = self.inner.serialize()?;
        self.ref_block_hash.consensus_encode(&mut bytes)?;
        Ok(bytes)
    }

    fn validate(
        &self,
        bitcoin_commitment: &(bitcoin::block::Header, u32),
        aggregate_pk: &secp256k1::PublicKey,
        pegin_data: PeginData,
    ) -> Result<U256, PeginDataError> {
        self.inner.validate(bitcoin_commitment, aggregate_pk, pegin_data)
    }

    fn to_essential(&self) -> EssentialPeginData {
        EssentialPeginData {
            outpoint: self.inner.outpoint,
            address: self.inner.address,
            tx: self.inner.tx.clone(),
        }
    }

    fn clone_box(&self) -> Box<dyn PeginMetaTrait> {
        Box::new(self.clone())
    }

    fn equals(&self, other: &dyn PeginMetaTrait) -> bool {
        // Only compare if the other object has the same type tag
        if other.type_tag() != self.type_tag() {
            return false;
        }

        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.inner.equals(&other.inner) &&
            self.ref_block_hash == other.ref_block_hash
        } else {
            false
        }
    }
    
    fn type_tag(&self) -> u32 {
        // A unique identifier for this type
        PEGIN_META_VERSION_V1
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Essential pegin data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EssentialPeginData {
    /// Outpoint of the pegin transaction
    pub outpoint: bitcoin::OutPoint,
    /// The complete pegin transaction
    pub tx: bitcoin::Transaction,
    /// Destination address for the pegin
    pub address: Address,
}

/// Error type for pegin data
#[derive(Debug, Error)]
pub enum PeginDataError {
    /// Invalid data format
    #[error("invalid data format")]
    InvalidFormat(#[from] btcencode::Error),
    /// Invalid pegin proof
    #[error("invalid pegin proof")]
    Invalid(&'static str),
    /// Invalid public key format
    #[error("invalid public key format")]
    InvalidPublicKey(secp256k1::Error),
    /// Invalid bitcoin block height
    #[error("invalid bitcoin block height")]
    InvalidBitcoinBlockHeight,
    /// Invalid tweak: failed to tweak aggregate public key
    #[error("invalid tweak: failed to tweak aggregate public key")]
    InvalidTweak(),
    /// Frost related error
    #[error("frost error {0}")]
    FrostError(frost::Error),
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
    Invalid(&'static str),
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
        address: String,
        btc_network: bitcoin::Network,
    ) -> Result<Self, PegoutDataError> {
        // Check for valid address
        let destination: bitcoin::address::Address<bitcoin::address::NetworkUnchecked> =
            bitcoin::address::Address::from_str(address.as_str())
                .map_err(|_e| PegoutDataError::Invalid("Invalid Bitcoin Address"))?;

        // For is address if valid for network
        let network_checked_destination = destination
            .require_network(btc_network)
            .map_err(|_e| PegoutDataError::Invalid("Address not valid for network"))?;

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
        BlockHash, CompactTarget, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid,
    };
    use revm_primitives::hex;
    use secp256k1::PublicKey;

    fn create_test_pegin_meta() -> PeginMeta {
        let txid = Txid::all_zeros();
        PeginMeta {
            version: PEGIN_META_VERSION_V0,
            merkle_proof: PartialMerkleTree::from_txids(&[txid], &[true]),
            outpoint: OutPoint { txid, vout: 0 },
            address: Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap(),
            aggregate_public_key: PublicKey::from_str(
                "0376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762",
            )
            .unwrap(),
            block_headers: vec![Header {
                version: Version::default(),
                prev_blockhash: BlockHash::all_zeros(),
                merkle_root: TxMerkleNode::from_slice(&[0; 32]).unwrap(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: 0,
            }],
            tx: Transaction {
                version: bitcoin::transaction::Version(1),
                lock_time: LockTime::from_str("0").unwrap(),
                input: vec![TxIn {
                    previous_output: OutPoint { txid, vout: 0 },
                    sequence: bitcoin::Sequence::MAX,
                    script_sig: bitcoin::ScriptBuf::new(),
                    witness: Default::default(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(100),
                    script_pubkey: ScriptBuf::new(),
                }],
            },
        }
    }

    fn create_test_pegin_meta_v1() -> PeginMetaV1 {
        let inner = create_test_pegin_meta();
        
        PeginMetaV1 {
            inner,
            ref_block_hash: B256::from_slice(&[0; 32]),
        }
    }

    #[test]
    fn serialize_pegin_metadata_v0() {
        let pegin_metadata = create_test_pegin_meta();

        let serialized = pegin_metadata.serialize().unwrap();
        let (deserialized, size) = deserialize_meta(&serialized).unwrap();
        let deserialized = deserialized.as_any().downcast_ref::<PeginMeta>().unwrap();
        assert_eq!(pegin_metadata.version, deserialized.version);
        assert_eq!(pegin_metadata.outpoint, deserialized.outpoint);
        assert_eq!(pegin_metadata.address, deserialized.address);
        assert_eq!(pegin_metadata.aggregate_public_key, deserialized.aggregate_public_key);
        assert_eq!(pegin_metadata.block_headers.len(), deserialized.block_headers.len());
        assert_eq!(pegin_metadata.tx, deserialized.tx);
        assert_eq!(
            pegin_metadata.merkle_proof.num_transactions(),
            deserialized.merkle_proof.num_transactions()
        );
        assert_eq!(serialized.len(), size);
    }

    #[test]
    fn serialize_pegin_metadata_v1() {
        let pegin_metadata = create_test_pegin_meta_v1();
        let serialized = pegin_metadata.serialize().unwrap();
        let (deserialized, size) = deserialize_meta(&serialized).unwrap();
        let deserialized = deserialized.as_any().downcast_ref::<PeginMetaV1>().unwrap();
        assert_eq!(pegin_metadata.inner.version, deserialized.inner.version);
        assert_eq!(pegin_metadata.inner.outpoint, deserialized.inner.outpoint);
        assert_eq!(pegin_metadata.inner.address, deserialized.inner.address);
        assert_eq!(
            pegin_metadata.inner.aggregate_public_key,
            deserialized.inner.aggregate_public_key
        );
        assert_eq!(
            pegin_metadata.inner.block_headers.len(),
            deserialized.inner.block_headers.len()
        );
        assert_eq!(pegin_metadata.inner.tx, deserialized.inner.tx);
        assert_eq!(
            pegin_metadata.inner.merkle_proof.num_transactions(),
            deserialized.inner.merkle_proof.num_transactions()
        );
        assert_eq!(serialized.len(), size);
        assert_eq!(pegin_metadata.ref_block_hash, deserialized.ref_block_hash);
    }

    #[test]
    fn deserialize_pegin_metadata_v0() {
        // Proof generated by side-car service
        let pegin_metadata_vec = hex::decode("000000002e5523bcd1b329e8a1a66b7d31719e94a33483eae77f5a677e6634d84ce55f470000000014194f42f33a9b3d5fe9e7ba8501be24d00b07b50376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762010080732aa97865f6b4be36ba861d397401e956d23e129940bb8a03000000000000000000b0d5ec7a0f49793b896db8f4a2cb4ec37e6b2dbd8e90e23d23f860abc9a76b70f1d2ba6494380517307685f90e00000005ce88336dc1340fed95a5f334a536b459d9af3aa3f44eb7b64de3a75e01812f021403c7f4a599f74775069cd3e3e589456fd672037ee1c2c9570f6776fb2e864a3e06d4e3858fdfa9f987053290aac66ef9b7c28fcaf3d3d64724d65a5fc11a2365557cde0d0e465dbfa1f2730617416133b2347ca5170fc1c0dd86f019356acf331d671f862a2841476864ef8639511b9e6f29c25b3c150680b626f9c185be81022f000100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();
        let (meta, size) = deserialize_meta(pegin_metadata_vec.as_slice()).unwrap();
        let meta = meta.as_any().downcast_ref::<PeginMeta>().unwrap();
        println!("meta {:?}", meta);
        println!("meta usize {:?}", size);
        assert_eq!(meta.version, PEGIN_META_VERSION_V0);
        assert_eq!(meta.merkle_proof.num_transactions(), 14);
        assert_eq!(
            meta.address.0.as_slice(),
            hex::decode("14194f42f33a9b3d5fe9e7ba8501be24d00b07b5").unwrap()
        );
        assert_eq!(
            meta.aggregate_public_key,
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

        let (meta, size) = deserialize_meta(pegin_metadata_vec.as_slice()).unwrap();
        let meta = meta.as_any().downcast_ref::<PeginMetaV1>().unwrap();
        println!("meta {:?}", meta);
        println!("meta usize {:?}", size);

        assert_eq!(meta.inner.version, PEGIN_META_VERSION_V1);
        assert_eq!(meta.inner.merkle_proof.num_transactions(), 14);
        assert_eq!(
            meta.inner.address.0.as_slice(),
            hex::decode("14194f42f33a9b3d5fe9e7ba8501be24d00b07b5").unwrap()
        );
        assert_eq!(
            meta.inner.aggregate_public_key,
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
    ) -> (PeginData, Vec<PeginMeta>) {
        let header_metadata = create_header_metadata(None, pk);
        let destination_address =
            Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap();

        let meta = PeginMeta {
            version: version.unwrap_or_default(),
            merkle_proof: header_metadata.merkle_proof,
            outpoint: header_metadata.outpoint,
            address: destination_address,
            aggregate_public_key: *pk,
            block_headers: if let Some(block_headers) = block_headers {
                block_headers
            } else {
                vec![header_metadata.header]
            },
            tx: header_metadata.tx,
        };

        (
            PeginData {
                account: destination_address,
                // 100 sats converted to wei
                amount: U256::from_str_radix("1000000000000", 10).unwrap(),
                bitcoin_block_height: 1_u32,
            },
            vec![meta]
        )
    }

    fn random_pk() -> secp256k1::PublicKey {
        let secp = secp256k1::Secp256k1::new();
        let mut rng = rand::thread_rng();
        secp256k1::PublicKey::from_secret_key(&secp, &secp256k1::SecretKey::new(&mut rng))
    }

    #[test]
    fn validate_pegin_data() {
        let pk = random_pk();
        let (pegin_data, pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        let aggregate_amount = pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data.clone()).expect("valid");
        assert_eq!(aggregate_amount, pegin_data.amount);
    }

    #[test]
    #[should_panic(expected = "recent block hash mismatch")]
    fn validate_pegin_data_without_headers() {
        let pk = random_pk();
        let (pegin_data, pegin_meta) = pegin_data_setup(None, Some(Vec::new()), &pk);
        let header = create_header_metadata(None, &pk).header;

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "recent block hash mismatch")]
    fn validate_pegin_data_with_incorrect_block_hash() {
        let pk = random_pk();
        let (pegin_data, pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = create_header_metadata(Some(1_u32), &pk).header;

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid merkle proof: inclusion")]
    fn validate_pegin_data_with_invalid_merkle_proof() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        let different_txid = bitcoin::Txid::all_zeros();
        let different_txids = vec![different_txid];
        let matches = vec![true];

        pegin_meta[0].merkle_proof = PartialMerkleTree::from_txids(&different_txids, &matches);

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid merkle proof: inclusion")]
    fn validate_pegin_data_with_invalid_outpoint() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        pegin_meta[0].outpoint.txid = bitcoin::Txid::all_zeros();

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "merkle proof and block header mismatch")]
    fn validate_pegin_data_with_mismatched_merkle_root() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);

        pegin_meta[0].block_headers[0].merkle_root = TxMerkleNode::all_zeros();

        let header = pegin_meta[0].block_headers[0];
        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "merkle proof and block header mismatch")]
    fn validate_pegin_data_with_same_txid_different_root() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        let original_txid = pegin_meta[0].outpoint.txid;

        let txids = vec![original_txid];
        let matches = vec![true];
        pegin_meta[0].merkle_proof = PartialMerkleTree::from_txids(&txids, &matches);

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid tx or outpoint: txid")]
    fn validate_pegin_data_with_invalid_tx() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        pegin_meta[0].tx.version = bitcoin::transaction::Version(999);

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid tx or outpoint: output idx")]
    fn validate_pegin_data_with_invalid_outpoint_vout() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        pegin_meta[0].outpoint.vout = 2;

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid script pubkey")]
    fn validate_pegin_data_with_invalid_script_pubkey() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);

        pegin_meta[0].tx.output[0].script_pubkey = bitcoin::ScriptBuf::new();

        let new_txid = pegin_meta[0].tx.compute_txid();

        let txids = vec![new_txid];
        let matches = vec![true];
        let merkle_proof = PartialMerkleTree::from_txids(&txids, &matches);

        let mut txids = Vec::with_capacity(1);
        let mut idxs = Vec::with_capacity(1);
        let root = merkle_proof.extract_matches(&mut txids, &mut idxs).unwrap();

        pegin_meta[0].block_headers[0].merkle_root = root;
        pegin_meta[0].merkle_proof = merkle_proof;
        pegin_meta[0].outpoint.txid = new_txid;
        

        let modified_header = pegin_meta[0].block_headers[0];
        pegin_meta[0].validate(&(modified_header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid script pubkey")]
    fn validate_pegin_data_with_different_account() {
        let pk = random_pk();
        let (mut pegin_data, pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        pegin_data.account = Address::with_last_byte(1);

        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid script pubkey")]
    fn validate_pegin_data_with_different_pubkey() {
        let pk = random_pk();
        let (pegin_data, pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];
        let different_pk = random_pk();

        pegin_meta[0].validate(&(header, 1_u32), &different_pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid block header sequence")]
    fn validate_pegin_data_with_invalid_block_sequence() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        let second_header = bitcoin::block::Header {
            version: header.version,
            prev_blockhash: header.block_hash(),
            merkle_root: header.merkle_root,
            time: header.time + 1,
            bits: header.bits,
            nonce: header.nonce,
        };

        pegin_meta[0].block_headers.push(second_header);
        pegin_meta[0].block_headers[1].prev_blockhash = bitcoin::BlockHash::all_zeros();
        
        pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    #[should_panic(expected = "invalid block header sequence")]
    fn validate_pegin_data_with_broken_block_chain_in_middle() {
        let pk = random_pk();
        let (pegin_data, mut pegin_meta) = pegin_data_setup(None, None, &pk);
        let first_header = pegin_meta[0].block_headers[0];

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

        pegin_meta[0].block_headers.push(second_header);
        pegin_meta[0].block_headers.push(third_header);

        pegin_meta[0].validate(&(first_header, 1_u32), &pk, pegin_data).unwrap();
    }

    #[test]
    fn validate_pegin_data_with_invalid_block_height() {
        let pk = random_pk();
        let (mut pegin_data, pegin_meta) = pegin_data_setup(None, None, &pk);
        let header = pegin_meta[0].block_headers[0];

        pegin_data.bitcoin_block_height = 999;

        assert!(matches!(
            pegin_meta[0].validate(&(header, 1_u32), &pk, pegin_data),
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

        let meta = vec![PeginMeta {
            version: PEGIN_META_VERSION_V0,
            merkle_proof: merkle_proof.clone(),
            outpoint,
            address: destination_address,
            aggregate_public_key: pk,
            block_headers: vec![header],
            tx: tx.clone(),
        }];

        let amount = U256::from_str_radix("1000000000000", 10).unwrap();

        let pegin_data = PeginData {
            account: destination_address,
            amount,
            bitcoin_block_height: 1_u32,
        };

        let res = meta[0].validate(&(header, 1_u32), &pk, pegin_data).unwrap_err();
        assert!(matches!(res, PeginDataError::Invalid("spending non-mature coinbase")));

        // Create a chain of 100 blocks with the coinbase tx in the last 100 blocks
        let mut headers = vec![header];
        for i in 1..101 {
            let mut header = create_header_metadata(None, &pk).header;
            header.prev_blockhash = headers[i - 1].block_hash();
            headers.push(header);
        }

        let meta = vec![PeginMeta {
            version: PEGIN_META_VERSION_V0,
            merkle_proof,
            outpoint,
            address: destination_address,
            aggregate_public_key: pk,
            block_headers: headers.clone(),
            tx,
        }];

        let pegin_data = PeginData {
            account: destination_address,
            amount,
            bitcoin_block_height: 0_u32,
        };

        let res = meta[0].validate(&(headers[100], 100_u32), &pk, pegin_data).unwrap();
        assert_eq!(res, amount);
    }
}
