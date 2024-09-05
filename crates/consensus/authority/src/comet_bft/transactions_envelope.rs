use bitcoin::{
    consensus::encode::{self, Decodable, Encodable},
    hex::DisplayHex,
};
use bytes::Buf;
use prost::bytes::Bytes;
use reth_primitives::TransactionSigned;
use reth_revm::primitives::bitvec::vec;
use std::io;
use thiserror::Error;

/// Errors that can occur when deserializing TransactionsEnvelope
#[derive(Debug, Error)]
pub enum TransactionsEnvelopeDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] NonDeterministicDataDeserializeError),
    #[error("invalid data format")]
    /// Invalid data format
    InvalidFormat(#[from] bitcoin::consensus::encode::Error),
}

// Type that wraps txs with additional non-deterministic data needed for consensus
#[derive(Debug, Clone)]
pub(crate) struct TransactionsEnvelope {
    pub(crate) format: u16,
    pub(crate) non_deterministic_data: NonDeterministicData,
    pub(crate) txs: Vec<Bytes>,
}

impl TransactionsEnvelope {
    pub(crate) fn version_default() -> u16 {
        0
    }

    pub(crate) fn new(non_deterministic_data: NonDeterministicData, txs: Vec<Bytes>) -> Self {
        Self { format: TransactionsEnvelope::version_default(), non_deterministic_data, txs }
    }

    pub(crate) fn serialize(&self) -> Result<Vec<Bytes>, TransactionsEnvelopeDeserializeError> {
        // needs to be in little endian since this is what bitcoin::consensus_decode() expects
        let format = Bytes::copy_from_slice(self.format.to_le_bytes().as_slice());
        let non_deterministic_data = match self.non_deterministic_data.serialize() {
            Ok(vec) => Bytes::copy_from_slice(vec.as_slice()),
            Err(e) => {
                return Err(TransactionsEnvelopeDeserializeError::Io(
                    NonDeterministicDataDeserializeError::Io(e),
                ))
            }
        };

        // combine all Bytes
        let mut result = vec![format, non_deterministic_data];
        result.extend(self.txs.clone());

        Ok(result)
    }

    pub(crate) fn deserialize(
        bytes: &Vec<Bytes>,
    ) -> Result<Self, TransactionsEnvelopeDeserializeError> {
        // get the txs byte vecs which are after the first 2 byte vecs:
        // first byte vec is the version,
        // second byte vec is the non-deterministic data
        let mut txs = bytes.iter().skip(2).into_iter().map(|tx| tx.clone()).collect::<Vec<_>>();

        // convert Vec<Bytes> to Vec<u8>
        let reader_inner: Vec<u8> = bytes.clone().into_iter().flatten().collect();
        let reader = &mut io::Cursor::new(reader_inner);
        let format = match u16::consensus_decode(reader) {
            Ok(format) => format,
            Err(e) => return Err(TransactionsEnvelopeDeserializeError::InvalidFormat(e)),
        };
        let non_deterministic_data = NonDeterministicData::deserialize(reader)?;

        Ok(Self { format, non_deterministic_data, txs })
    }
}

/// Errors that can occur when deserializing NonDeterministicData
#[derive(Debug, Error)]
pub enum NonDeterministicDataDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] io::Error),
    #[error("invalid data format")]
    /// Invalid data format
    Decoding(#[from] encode::Error),
    #[error("invalid version")]
    /// Invalid NonDeterministicData, version
    InvalidVersion,
}

/// Type that encapsulates non-deterministic data needed for consensus
#[derive(Debug, Clone)]
pub struct NonDeterministicData {
    pub version: u16,
    pub bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub aggregated_public_key: secp256k1::PublicKey,
}

impl NonDeterministicData {
    pub fn version_default() -> u16 {
        0
    }

    pub fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
    ) -> Self {
        Self {
            version: NonDeterministicData::version_default(),
            bitcoin_block_hash,
            aggregated_public_key,
        }
    }

    pub fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version.consensus_encode(&mut writer)?;

        Ok(writer.to_vec())
    }

    pub fn deserialize(
        reader: &mut impl io::Read,
    ) -> Result<Self, NonDeterministicDataDeserializeError> {
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes).map_err(|e| {
            println!("Error: {:?}", e);
            encode::Error::ParseFailed("malformed aggregate public key")
        })?;
        let version = u16::consensus_decode(reader)?;
        if version != NonDeterministicData::version_default() {
            return Err(NonDeterministicDataDeserializeError::InvalidVersion);
        }

        Ok(Self { version, bitcoin_block_hash, aggregated_public_key })
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{hashes::Hash, BlockHash};

    use super::*;

    #[test]
    fn test_version_default() {
        let version = NonDeterministicData::version_default();
        assert_eq!(version, 0);
    }

    #[test]
    fn test_non_deterministic_data_new() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ndd = NonDeterministicData::new(bitcoin_block_hash, pk);
        assert_eq!(ndd.version, NonDeterministicData::version_default());
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
    }

    #[test]
    fn test_non_deterministic_data_deserialize() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk: secp256k1::PublicKey = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ev = NonDeterministicData::new(bitcoin_block_hash, pk);
        let res = ev.serialize().unwrap();
        let mut reader = io::Cursor::new(res);
        let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized.version, ev.version);
        assert_eq!(deserialized.bitcoin_block_hash, ev.bitcoin_block_hash);
        assert_eq!(deserialized.aggregated_public_key, ev.aggregated_public_key);
    }

    #[test]
    fn test_transactions_envelope_version_default() {
        let version = TransactionsEnvelope::version_default();
        assert_eq!(version, 0);
    }

    #[test]
    fn test_transactions_envelope_new() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ndd = NonDeterministicData::new(bitcoin_block_hash, pk);
        let txs = vec![Bytes::copy_from_slice(b"tx1"), Bytes::copy_from_slice(b"tx2")];
        let envelope = TransactionsEnvelope::new(ndd, txs.clone());
        assert_eq!(envelope.format, TransactionsEnvelope::version_default());
        assert_eq!(
            envelope.non_deterministic_data.version,
            NonDeterministicData::version_default()
        );
        assert_eq!(envelope.non_deterministic_data.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(envelope.non_deterministic_data.aggregated_public_key, pk);
        assert_eq!(envelope.txs, txs);
    }

    #[test]
    fn test_transactions_envelope_serialize_deserialize() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let ndd = NonDeterministicData::new(bitcoin_block_hash, pk);
        let tx1 = TransactionSigned::default().envelope_encoded();
        let tx2 = TransactionSigned::default().envelope_encoded();
        let txs: Vec<Bytes> = vec![tx1.clone(), tx2.clone()]
            .iter()
            .map(|tx| prost::bytes::Bytes::copy_from_slice(tx))
            .collect::<_>();
        let envelope = TransactionsEnvelope::new(ndd, txs.clone());

        let mut res = envelope.serialize().expect("envelope to be serialized");
        let deserialized =
            TransactionsEnvelope::deserialize(&mut res).expect("envelope to be deserialized");
        assert_eq!(deserialized.format, envelope.format);
        assert_eq!(
            deserialized.non_deterministic_data.version,
            envelope.non_deterministic_data.version
        );
        assert_eq!(
            deserialized.non_deterministic_data.bitcoin_block_hash,
            envelope.non_deterministic_data.bitcoin_block_hash
        );
        assert_eq!(
            deserialized.non_deterministic_data.aggregated_public_key,
            envelope.non_deterministic_data.aggregated_public_key
        );

        // convert txs to TransactionSigned and compare
        // could also just convert reth_primitive bytes (vec![tx1, tx2]) to prost::bytes::Bytes and compare
        let deserialized_txs = deserialized
            .txs
            .iter()
            .map(|tx| {
                let signed_tx =
                    TransactionSigned::decode_enveloped(&mut tx.to_vec().as_slice()).unwrap();
                signed_tx
            })
            .collect::<Vec<_>>();
        let expected_txs = vec![tx1, tx2]
            .iter()
            .map(|tx| {
                let signed_tx =
                    TransactionSigned::decode_enveloped(&mut tx.to_vec().as_slice()).unwrap();
                signed_tx
            })
            .collect::<Vec<_>>();
        assert_eq!(deserialized_txs, expected_txs);
    }
}
