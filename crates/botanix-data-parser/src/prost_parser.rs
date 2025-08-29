use bytes::Bytes;
use displaydoc::Display as DisplayDoc;
use thiserror::Error;

/// Prost encode/decode error types.
#[derive(Debug, DisplayDoc, Error)]
pub enum ProstError {
    /// serde prost encode error
    ProstEncode(#[from] prost::EncodeError),
    /// serde prost decode error
    ProstDecode(#[from] prost::DecodeError),
}

/// Prost Message Wrapper allowing serialization/deserialization
#[allow(dead_code)]
pub struct ProstMessageSerdelizer<T: prost::Message>(pub T);

#[allow(dead_code)]
impl<T> ProstMessageSerdelizer<T>
where
    T: prost::Message + std::default::Default,
{
    /// Method to serialize
    pub fn serialize(&self) -> Result<Vec<u8>, ProstError> {
        let mut buf = Vec::new();
        self.0.encode(&mut buf).map_err(ProstError::ProstEncode)?;
        Ok(buf)
    }

    /// Method to deserialize
    pub fn deserialize(buf: Vec<u8>) -> Result<T, ProstError> {
        T::decode(Bytes::from(buf)).map_err(ProstError::ProstDecode)
    }
}

#[cfg(test)]
mod test {
    use crate::{
        prost_parser::ProstMessageSerdelizer, DataParser, SerializationType,
        DEFAULT_COMPRESSION_STRATEGY,
    };
    use bitcoin::{hashes::Hash, Txid};
    use btc_server_client::{GetAllUtxosResponse, TxOut, Utxo};
    use rand::{thread_rng, Rng};

    #[tokio::test]
    async fn test_compress_decompress_json() {
        // generate some data
        let data = serde_json::json!({
            "name": "george",
            "date_of_birth": 1987,
            "male": "male"
        });

        let parser = DataParser::default().with_compression_strategy(&DEFAULT_COMPRESSION_STRATEGY);
        let compressed_serialized_data = parser.encode(&data).await.unwrap();
        let decompressed_deserialized_data =
            parser.decode::<serde_json::Value>(&compressed_serialized_data).await.unwrap();
        assert_eq!(data, decompressed_deserialized_data);
    }

    #[tokio::test]
    async fn test_serialize_deserialize_json() {
        // generate some data
        let data = serde_json::json!({
            "name": "george",
            "date_of_birth": 1987,
            "male": "male"
        });

        // serialize and compress the data
        let parser = DataParser::default().with_serialization_type(SerializationType::Json);
        let serialized_compressed_data = parser.encode(&data).await.unwrap();

        // deserialize and decompress the data
        let deserialized_decompressed_data =
            parser.decode::<serde_json::Value>(&serialized_compressed_data).await.unwrap();
        assert_eq!(data, deserialized_decompressed_data);
    }

    #[tokio::test]
    async fn test_utxo_serde() {
        let mut rng = thread_rng();
        let mut utxos = vec![];
        // generate utxos
        for _ in 0..100 {
            let txid = Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap().to_byte_array().to_vec();
            let script_pubkey = rng.gen::<[u8; 32]>().to_vec();
            let vout = rng.gen_range(0..u32::MAX);
            let utxo = Utxo {
                outpoint: Some(btc_server_client::OutPoint { txid: txid.clone(), vout }),
                output: Some(TxOut {
                    script_pubkey: Some(btc_server_client::ScriptBuf { script: script_pubkey }),
                    value: rng.gen::<u64>(),
                }),
                eth_address: "0x0".to_string(),
            };
            utxos.push(utxo);
        }

        // create a prost message
        let prost_utxos = GetAllUtxosResponse { utxos };

        // serialize the prost message
        let prost_message_wrapper = ProstMessageSerdelizer(prost_utxos.clone());
        let prost_serialized = prost_message_wrapper.serialize().unwrap();
        println!("Serialized to bytes: {:?}", prost_serialized);

        // now decompress the prost message
        let prost_deserialized =
            ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(prost_serialized).unwrap();
        println!("Deserialized to bytes: {:?}", prost_deserialized);

        assert!(prost_utxos == prost_deserialized);
    }

    #[tokio::test]
    async fn test_utxo_serde_compress_decompress() {
        let mut rng = thread_rng();
        let mut utxos = vec![];
        // generate utxos
        for _ in 0..100 {
            let txid = Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap().to_byte_array().to_vec();
            let script_pubkey = rng.gen::<[u8; 32]>().to_vec();
            let vout = rng.gen_range(0..u32::MAX);
            let utxo = Utxo {
                outpoint: Some(btc_server_client::OutPoint { txid: txid.clone(), vout }),
                output: Some(TxOut {
                    script_pubkey: Some(btc_server_client::ScriptBuf { script: script_pubkey }),
                    value: rng.gen::<u64>(),
                }),
                eth_address: "0x0".to_string(),
            };
            utxos.push(utxo);
        }

        // create a prost message
        let prost_utxos = GetAllUtxosResponse { utxos };

        // serialize the prost message
        let prost_message_wrapper = ProstMessageSerdelizer(prost_utxos.clone());
        let prost_serialized = prost_message_wrapper.serialize().unwrap();

        // now compress the prost message
        let parser = DataParser::default().with_serialization_type(SerializationType::Json);
        let prost_serialized_compressed = parser.compress(&prost_serialized).await.unwrap();
        println!(
            "Compressed to bytes: serialized: {:?} bytes, ser+compressed {:?} bytes",
            prost_serialized.len(),
            prost_serialized_compressed.len()
        );

        assert!(
            prost_serialized.len() >= prost_serialized_compressed.len(),
            "serialized message length is greater or equal to the compressed length"
        );

        // now decompress the prost message
        let prost_serialized_decompressed =
            parser.decompress(&prost_serialized_compressed).await.unwrap();
        let prost_serialized_decompressed_clone = prost_serialized_decompressed.clone();
        let prost_deserialized = ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(
            prost_serialized_decompressed,
        )
        .unwrap();
        println!(
            "Serialized + decompressed: {:?} bytes, Deserialized {:?} bytes",
            prost_serialized_decompressed_clone.len(),
            prost_deserialized
        );

        assert!(
            !prost_deserialized.utxos.is_empty(),
            "deserialized message length is greater than 0"
        );

        assert!(prost_utxos == prost_deserialized);
    }
}
