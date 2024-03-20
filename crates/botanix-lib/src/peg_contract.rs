use std::str::FromStr;

use bitcoin::{
    block::Header,
    consensus::{
        encode::{self as btcencode, Decodable},
        Encodable, ReadExt,
    },
    merkle_tree::PartialMerkleTree,
    secp256k1::PublicKey,
};

// use bitcoin::{self};
use ethers::types::U256;
use reth_primitives::Address;
use thiserror::Error;

use reth_btc_wallet::{address, key};

use crate::utils::AmountExt;

const PEGIN_META_VERSION: u32 = 0;
const _PEGOUT_META_VERSION: u32 = 0;

#[derive(Debug)]
pub struct PeginData {
    pub account: Address,
    pub amount: U256,
    pub bitcoin_block_height: u32,
    pub meta: Vec<PeginMeta>,
}

impl PeginData {
    /// Validates multiple pegin proofs against the current bitcoin block hash
    /// Returns the aggregate value of all the pegin amounts
    pub fn validate(
        &self,
        bitcoin_block: &(bitcoin::block::Header, u32),
        aggregate_pk: &secp256k1::PublicKey,
    ) -> Result<U256, PeginError> {
        // the aggregate value from all the pegin proofs
        let mut aggregate_value = U256::from_str_radix("0", 10).expect("valid amount");
        for pegin in &self.meta {
            if pegin.version != PEGIN_META_VERSION {
                return Err(PeginError::Invalid("invalid meta version: only accepting version 0"));
            }

            if pegin.block_headers.is_empty() {
                return Err(PeginError::Invalid("no block headers found"));
            }

            // pegin block headers list should contain the tip header as the last element in the
            // list
            if pegin.block_headers.last().expect("header should exist").block_hash() !=
                bitcoin_block.0.block_hash()
            {
                return Err(PeginError::Invalid("recent block hash mismatch"));
            }

            let op = pegin.outpoint;
            if pegin.tx.txid() != op.txid {
                return Err(PeginError::Invalid("invalid tx or outpoint: txid"));
            }

            if pegin.tx.output.len() < op.vout as usize {
                return Err(PeginError::Invalid("invalid tx or outpoint: output idx"));
            }

            let tpk = key::tweak_frost_verifying_key(aggregate_pk, &self.account.0 .0)
                .map_err(|_e| PeginError::InvalidTweak())?;
            let gateway_script = address::generate_taproot_scriptpubkey(&tpk);

            let output = &pegin.tx.output[op.vout as usize];
            if gateway_script != output.script_pubkey {
                return Err(PeginError::Invalid("invalid script pubkey"));
            }

            let output_value = bitcoin::Amount::from_sat(output.value).to_wei();
            // if output_value < self.amount {
            //     return Err(PeginError::Invalid("invalid amount"));
            // }
            aggregate_value += output_value;

            let mut txids = Vec::with_capacity(1);
            let mut idxs = Vec::with_capacity(1);
            let merkle = &pegin.merkle_proof;
            let root = merkle.extract_matches(&mut txids, &mut idxs).unwrap();
            if !txids.contains(&op.txid) {
                return Err(PeginError::Invalid("invalid merkle proof: inclusion"));
            }

            // check that the user provided an actual valid block header sequence
            let mut iter = pegin.block_headers.iter().peekable();
            while let Some(header) = iter.next() {
                if let Some(next) = iter.peek() {
                    if next.prev_blockhash != header.block_hash() {
                        return Err(PeginError::Invalid("invalid block header sequence"));
                    }
                }
            }

            // look for the nth deep header in the set of block headers provided by user
            if !pegin
                .block_headers
                .iter()
                .any(|header| header.block_hash() == bitcoin_block.0.block_hash())
            {
                return Err(PeginError::Invalid("block header not found"));
            }
            // At this point the user proven that the proof is n blocks deep
            // (assuming that `block_header` is n blocks deep)
            // Now we must check the merkle proof is in one of the blocks provided
            let _confirmed_block_position = pegin
                .block_headers
                .iter()
                .position(|header| header.merkle_root == root)
                .ok_or(PeginError::Invalid("merkle proof and block header mismatch"))?;

            // calculate how many blocks deep the user block is
            // Note: we know that the tip is always at position n. i.e the len of array - 1
            // TODO (armins) Most likely the user block is at position 0. Although this is not
            // guaranteed

            let diff = pegin.block_headers.len() - 1;
            // the latest block height minus the position of the user block in the list is the
            // height of the user block
            if bitcoin_block.1 - (diff as u32) != self.bitcoin_block_height {
                return Err(PeginError::InvalidBitcoinBlockHeight());
            }
        }

        Ok(aggregate_value)
    }
}

#[derive(Debug)]
pub struct PeginMeta {
    pub version: u32,
    pub merkle_proof: PartialMerkleTree,
    pub outpoint: bitcoin::OutPoint,
    pub address: Address,
    pub aggregate_publickey: secp256k1::PublicKey,
    /// Bitcoin block header containing the pegin transaction
    pub block_headers: Vec<Header>,
    pub tx: bitcoin::Transaction,
}

impl PeginMeta {
    // TODO serialize()
    pub fn serialize(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.version.consensus_encode(&mut bytes).unwrap();
        self.outpoint.consensus_encode(&mut bytes).unwrap();
        bytes.extend_from_slice(self.address.0.as_slice());
        bytes.extend_from_slice(&self.aggregate_publickey.serialize());
        btcencode::VarInt(self.block_headers.len() as u64).consensus_encode(&mut bytes).unwrap();
        for header in &self.block_headers {
            header.consensus_encode(&mut bytes).unwrap();
        }
        self.merkle_proof.consensus_encode(&mut bytes).unwrap();
        self.tx.consensus_encode(&mut bytes).unwrap();

        bytes
    }

    pub fn deserialize(mut bytes: &[u8]) -> Result<(PeginMeta, usize), PeginError> {
        // bytes is a list of proofs
        let proofs_size = bytes.len();

        Ok((
            PeginMeta {
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
                    PublicKey::from_slice(&pk_bytes).map_err(PeginError::InvalidPublicKey)?
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
            },
            // list of proofs size - bytes left = proof size
            proofs_size - bytes.len(),
        ))
    }
}

#[derive(Debug, Error)]
pub enum PeginError {
    #[error("invalid data format")]
    InvalidFormat(#[from] btcencode::Error),
    #[error("invalid pegin proof")]
    Invalid(&'static str),
    #[error("invalid public key format")]
    InvalidPublicKey(secp256k1::Error),
    #[error("invalid bitcoin block height")]
    InvalidBitcoinBlockHeight(),
    #[error("invalid tweak: failed to tweak aggregate public key")]
    InvalidTweak(),
}

#[derive(Debug, Error)]
pub enum PegoutError {
    #[error("invalid pegout proof")]
    Invalid(&'static str),
}

#[derive(Debug)]
pub struct PegoutData {
    pub amount: bitcoin::Amount,
    pub destination: bitcoin::Address,
    pub network: bitcoin::Network,
}

impl PegoutData {
    pub fn new(amount: bitcoin::Amount, address: String) -> Result<Self, PegoutError> {
        // TODO (armins) This should be coming from config
        let network = bitcoin::Network::Regtest;
        // Check for valid addres
        let destination: bitcoin::address::Address<bitcoin::address::NetworkUnchecked> =
            bitcoin::address::Address::from_str(address.as_str())
                .map_err(|_e| PegoutError::Invalid("Invalid Bitcoin Address"))?;

        // For is address if valid for network
        let network_checked_destination = destination
            .require_network(network)
            .map_err(|_e| PegoutError::Invalid("Address not valid for network"))?;

        Ok(Self { amount, destination: network_checked_destination, network })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bitcoin::{
        absolute::LockTime, block::Version, hash_types::TxMerkleNode, hashes::Hash, BlockHash,
        CompactTarget, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid,
    };
    use secp256k1::rand::thread_rng;

    use super::*;

    #[test]
    fn serialize_pegin_metadata() {
        // random txid
        let txid = Txid::all_zeros();

        let pegin_metadata = PeginMeta {
            version: PEGIN_META_VERSION,
            merkle_proof: PartialMerkleTree::from_txids(&[txid], &[true]),
            outpoint: OutPoint { txid, vout: 0 },
            address: Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap(),
            aggregate_publickey: PublicKey::from_str(
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
                version: 1,
                lock_time: LockTime::from_str("0").unwrap(),
                input: vec![TxIn {
                    previous_output: OutPoint { txid, vout: 0 },
                    sequence: bitcoin::Sequence::MAX,
                    script_sig: bitcoin::ScriptBuf::new(),
                    witness: Default::default(),
                }],
                output: vec![TxOut { value: 100, script_pubkey: ScriptBuf::new() }],
            },
        };

        let serialized = pegin_metadata.serialize();
        let (deserialized, size) = PeginMeta::deserialize(&serialized).unwrap();
        assert_eq!(pegin_metadata.version, deserialized.version);
        assert_eq!(pegin_metadata.outpoint, deserialized.outpoint);
        assert_eq!(pegin_metadata.address, deserialized.address);
        assert_eq!(pegin_metadata.aggregate_publickey, deserialized.aggregate_publickey);
        assert_eq!(pegin_metadata.block_headers.len(), deserialized.block_headers.len());
        assert_eq!(pegin_metadata.tx, deserialized.tx);
        assert_eq!(
            pegin_metadata.merkle_proof.num_transactions(),
            deserialized.merkle_proof.num_transactions()
        );
        assert_eq!(serialized.len(), size);
    }

    #[test]
    fn deserialize_pegin_metadata() {
        // Proof generated by side-car service
        let pegin_metadata_vec = hex::decode("000000002e5523bcd1b329e8a1a66b7d31719e94a33483eae77f5a677e6634d84ce55f470000000014194f42f33a9b3d5fe9e7ba8501be24d00b07b50376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762010080732aa97865f6b4be36ba861d397401e956d23e129940bb8a03000000000000000000b0d5ec7a0f49793b896db8f4a2cb4ec37e6b2dbd8e90e23d23f860abc9a76b70f1d2ba6494380517307685f90e00000005ce88336dc1340fed95a5f334a536b459d9af3aa3f44eb7b64de3a75e01812f021403c7f4a599f74775069cd3e3e589456fd672037ee1c2c9570f6776fb2e864a3e06d4e3858fdfa9f987053290aac66ef9b7c28fcaf3d3d64724d65a5fc11a2365557cde0d0e465dbfa1f2730617416133b2347ca5170fc1c0dd86f019356acf331d671f862a2841476864ef8639511b9e6f29c25b3c150680b626f9c185be81022f000100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();
        let (meta, size) = PeginMeta::deserialize(&pegin_metadata_vec.as_slice()).unwrap();
        println!("meta {:?}", meta);
        println!("meta usize {:?}", size);
        assert_eq!(meta.version, PEGIN_META_VERSION);
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

        let gateway_script = address::generate_taproot_scriptpubkey(&pk);

        let tx_out = TxOut { value: 100_u64, script_pubkey: gateway_script };
        let tx: Transaction = Transaction {
            version: 1_i32,
            lock_time: LockTime::from_str("0").unwrap(),
            input: vec![tx_in],
            output: vec![tx_out],
        };

        let outpoint = OutPoint { txid: tx.txid(), vout: 0 };

        let txids = vec![
            Txid::from_str("4fccd63b48697a66ae4155b183f7595694354def0345ac4b950a5765a7b90526")
                .expect("valid txid"),
            tx.txid(),
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
            nonce: if nonce.is_some() { nonce.unwrap() } else { 0_u32 },
        };

        HeaderMetadata { header, merkle_proof, outpoint, tx }
    }

    fn pegin_data_setup(
        version: Option<u32>,
        block_headers: Option<Vec<Header>>,
        pk: &secp256k1::PublicKey,
    ) -> PeginData {
        let header_metadata = create_header_metadata(None, pk);

        let meta = PeginMeta {
            version: if version.is_some() { version.unwrap() } else { 0_u32 },
            merkle_proof: header_metadata.merkle_proof,
            outpoint: header_metadata.outpoint,
            address: Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap(),
            aggregate_publickey: *pk,
            block_headers: if block_headers.is_some() {
                block_headers.unwrap()
            } else {
                vec![header_metadata.header]
            },
            tx: header_metadata.tx,
        };

        PeginData {
            account: Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap(),
            // 100 sats converted to wei
            amount: U256::from_str_radix("1000000000000", 10).unwrap(),
            bitcoin_block_height: 1_u32,
            meta: vec![meta],
        }
    }

    #[test]
    fn validate_pegin_data() {
        let secp: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
        let mut rng = thread_rng();
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &secp256k1::SecretKey::new(&mut rng));

        let pegin_data = pegin_data_setup(None, None, &pk);
        let header = pegin_data.meta.first().unwrap().block_headers.first().unwrap();

        let aggregate_amount = pegin_data.validate(&(*header, 1_u32), &pk).expect("valid");
        println!("aggregate amount {:?}", aggregate_amount);
    }

    #[test]
    #[should_panic(expected = "invalid meta version: only accepting version 0")]
    fn validate_pegin_data_with_incorrect_version() {
        let secp: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
        let mut rng = thread_rng();
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &secp256k1::SecretKey::new(&mut rng));

        let pegin_data = pegin_data_setup(Some(1_u32), None, &pk);
        let header = pegin_data.meta.first().unwrap().block_headers.first().unwrap();

        pegin_data.validate(&(*header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "no block headers found")]
    fn validate_pegin_data_without_headers() {
        let secp: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
        let mut rng = thread_rng();
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &secp256k1::SecretKey::new(&mut rng));

        let pegin_data = pegin_data_setup(None, Some(Vec::new()), &pk);
        let header = create_header_metadata(None, &pk).header;

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    #[test]
    #[should_panic(expected = "recent block hash mismatch")]
    fn validate_pegin_data_with_incorrect_block_hash() {
        let secp: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
        let mut rng = thread_rng();
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &secp256k1::SecretKey::new(&mut rng));

        let pegin_data = pegin_data_setup(None, None, &pk);
        let header = create_header_metadata(Some(1_u32), &pk).header;

        pegin_data.validate(&(header, 1_u32), &pk).unwrap();
    }

    // TODO: scott - add tests for all possible errors returned in validate()
}
