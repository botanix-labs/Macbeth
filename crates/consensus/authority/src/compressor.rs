use async_compression::{
    tokio::write::{
        BrotliDecoder, BrotliEncoder, BzDecoder, BzEncoder, DeflateDecoder, DeflateEncoder,
        GzipDecoder, GzipEncoder, LzmaDecoder, LzmaEncoder, ZlibDecoder, ZlibEncoder, ZstdDecoder,
        ZstdEncoder,
    },
    Level,
};
use bytes::Bytes;
use displaydoc::Display as DisplayDoc;
use serde::Deserialize;
use strum::{AsRefStr, EnumIter, EnumString};
use thiserror::Error;
use tokio::io::AsyncWriteExt as _; // for `write_all` and `shutdown`

/// Password hashing error types.
#[derive(Debug, DisplayDoc, Error)]
pub enum CompressionError {
    /// Compression/Decompression zlib error
    Zlib(std::io::Error),
    /// Compression/Decompression gzip error
    Gzip(std::io::Error),
    /// Compression/Decompression brotli error
    Brotli(std::io::Error),
    /// Compression/Decompression bz error
    Bz(std::io::Error),
    /// Compression/Decompression lzma error
    Lzma(std::io::Error),
    /// Compression/Decompression deflate error
    Deflate(std::io::Error),
    /// Compression/Decompression zstd error
    Zstd(std::io::Error),
}

/// Password hashing error types.
#[derive(Debug, DisplayDoc, Error)]
pub enum SerdeError {
    /// serde bincode error
    Bincode(#[from] bincode::ErrorKind),
    /// serde postcard error
    Postcard(#[from] postcard::Error),
    /// serde prost encode error
    ProstEncode(#[from] prost::EncodeError),
    /// serde prost decode error
    ProstDecode(#[from] prost::DecodeError),
}

/// Password hashing error types.
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// compression error: {0}
    Compression(#[from] CompressionError),
    /// serde error: {0}
    Serde(#[from] SerdeError),
}

#[derive(
    Debug, Copy, Clone, Deserialize, EnumString, AsRefStr, EnumIter, strum_macros::Display,
)]
pub enum CompressionType {
    #[strum(serialize = "none")]
    None,
    #[strum(serialize = "zlib")]
    Zlib,
    #[strum(serialize = "gzip")]
    Gzip,
    #[strum(serialize = "brotli")]
    Brotli,
    #[strum(serialize = "bz")]
    Bz,
    #[strum(serialize = "lzma")]
    Lzma,
    #[strum(serialize = "deflate")]
    Deflate,
    #[strum(serialize = "zstd")]
    Zstd,
}

#[derive(
    Debug, Copy, Clone, Deserialize, EnumString, AsRefStr, EnumIter, strum_macros::Display,
)]
pub enum SerializationType {
    #[strum(serialize = "bincode")]
    Bincode,
    #[strum(serialize = "postcard")]
    Postcard,
}

pub struct ProstMessageSerdelizer<T: prost::Message>(pub T);

impl<T> ProstMessageSerdelizer<T>
where
    T: prost::Message + std::default::Default,
{
    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        let mut buf = Vec::new();
        self.0.encode(&mut buf).map_err(|e| Error::Serde(SerdeError::ProstEncode(e)))?;
        Ok(buf)
    }

    pub fn deserialize(buf: Vec<u8>) -> Result<T, Error> {
        //let x = Bytes::from(buf);
        T::decode(Bytes::from(buf)).map_err(|e| Error::Serde(SerdeError::ProstDecode(e)))
    }
}

#[derive(Debug)]
pub struct Compressor {
    compression_type: CompressionType,
    compression_level: Level,
    serialization_type: SerializationType,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            compression_type: CompressionType::Zlib,
            compression_level: Level::Best,
            serialization_type: SerializationType::Bincode,
        }
    }

    pub fn set_compression_type(&mut self, compression_type: CompressionType) {
        self.compression_type = compression_type;
    }

    pub fn set_compression_level(&mut self, compression_level: Level) {
        self.compression_level = compression_level;
    }

    pub fn set_serialization_type(&mut self, serialization_type: SerializationType) {
        self.serialization_type = serialization_type;
    }

    pub async fn serialize_and_compress(
        &self,
        in_data: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let serialized_data = self.serialize(in_data).await?;
        let compressed_data = self.compress(&serialized_data[..]).await?;
        Ok(compressed_data)
    }

    pub async fn decompress_and_deserialize(
        &self,
        in_data: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let decompressed_data = self.decompress(in_data).await?;
        let deserialized_data = self.deserialize(&decompressed_data[..]).await?;
        Ok(deserialized_data)
    }

    pub async fn serialize(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.serialization_type {
            SerializationType::Bincode => {
                bincode::serialize(in_data).map_err(|e| Error::Serde(SerdeError::Bincode(*e)))
            }
            SerializationType::Postcard => {
                postcard::to_allocvec(in_data).map_err(|e| Error::Serde(SerdeError::Postcard(e)))
            }
        }
    }

    pub async fn deserialize(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.serialization_type {
            SerializationType::Bincode => {
                bincode::deserialize(in_data).map_err(|e| Error::Serde(SerdeError::Bincode(*e)))
            }
            SerializationType::Postcard => {
                postcard::from_bytes(in_data).map_err(|e| Error::Serde(SerdeError::Postcard(e)))
            }
        }
    }

    pub async fn compress(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.compression_type {
            CompressionType::None => Ok(in_data.to_vec()),
            CompressionType::Zlib => {
                let mut encoder = ZlibEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zlib(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zlib(e)))?;
                Ok(encoder.into_inner())
            }
            CompressionType::Gzip => {
                let mut encoder = GzipEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Gzip(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Gzip(e)))?;
                Ok(encoder.into_inner())
            }
            CompressionType::Brotli => {
                let mut encoder = BrotliEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Brotli(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Brotli(e)))?;
                Ok(encoder.into_inner())
            }
            CompressionType::Bz => {
                let mut encoder = BzEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Bz(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Bz(e)))?;
                Ok(encoder.into_inner())
            }
            CompressionType::Lzma => {
                let mut encoder = LzmaEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Lzma(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Lzma(e)))?;
                Ok(encoder.into_inner())
            }
            CompressionType::Deflate => {
                let mut encoder = DeflateEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Deflate(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Deflate(e)))?;
                Ok(encoder.into_inner())
            }
            CompressionType::Zstd => {
                let mut encoder = ZstdEncoder::with_quality(Vec::new(), self.compression_level);
                encoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zstd(e)))?;
                encoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zstd(e)))?;
                Ok(encoder.into_inner())
            }
        }
    }

    pub async fn decompress(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.compression_type {
            CompressionType::None => Ok(in_data.to_vec()),
            CompressionType::Zlib => {
                let mut decoder = ZlibDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zlib(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zlib(e)))?;
                Ok(decoder.into_inner())
            }
            CompressionType::Gzip => {
                let mut decoder = GzipDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Gzip(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Gzip(e)))?;
                Ok(decoder.into_inner())
            }
            CompressionType::Brotli => {
                let mut decoder = BrotliDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Brotli(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Brotli(e)))?;
                Ok(decoder.into_inner())
            }
            CompressionType::Bz => {
                let mut decoder = BzDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Bz(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Bz(e)))?;
                Ok(decoder.into_inner())
            }
            CompressionType::Lzma => {
                let mut decoder = LzmaDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Lzma(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Lzma(e)))?;
                Ok(decoder.into_inner())
            }
            CompressionType::Deflate => {
                let mut decoder = DeflateDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Deflate(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Deflate(e)))?;
                Ok(decoder.into_inner())
            }
            CompressionType::Zstd => {
                let mut decoder = ZstdDecoder::new(Vec::new());
                decoder
                    .write_all(in_data)
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zstd(e)))?;
                decoder
                    .shutdown()
                    .await
                    .map_err(|e| Error::Compression(CompressionError::Zstd(e)))?;
                Ok(decoder.into_inner())
            }
        }
    }
}

#[cfg(test)]
pub mod test {
    use crate::compressor::{Compressor, ProstMessageSerdelizer};
    use bitcoin::{
        absolute::LockTime, blockdata::script::Script, hashes::Hash, psbt::Psbt, Address, Amount,
        FeeRate, ScriptBuf, Sequence, Transaction, TxIn, Txid,
    };
    use bytes::Bytes;
    use client::{GetAllUtxosResponse, OutPoint, Utxo};
    use rand::{thread_rng, Rng, RngCore};
    use serde_json::Value;
    use std::{collections::BTreeMap, str::FromStr};

    #[tokio::test]
    async fn test_compress_decompress_json() {
        // generate some data
        let data = serde_json::json!({
            "name": "george",
            "date_of_birth": 1987,
            "male": "male"
        });

        // compress the data
        let compressor = Compressor::new();
        let bytes = serde_json::to_vec(&data).unwrap();
        let compressed_data = compressor.compress(bytes.as_slice()).await.unwrap();
        println!("Compressed data {:?}", compressed_data);

        // decompress the data
        let decompressed_data = compressor.decompress(compressed_data.as_slice()).await.unwrap();
        println!("Decompressed data {:?}", decompressed_data);

        // check and compare
        let original_data: Value = serde_json::from_slice(decompressed_data.as_slice()).unwrap();
        assert_eq!(data, original_data);
    }

    #[tokio::test]
    async fn test_serialize_deserialize_json() {
        // generate some data
        let data = serde_json::json!({
            "name": "george",
            "date_of_birth": 1987,
            "male": "male"
        });

        // serialize the data
        let compressor = Compressor::new();
        let bytes = serde_json::to_vec(&data).unwrap();
        let serialized_data = compressor.serialize(bytes.as_slice()).await.unwrap();
        println!("Serialized data {:?}", serialized_data);

        // deserialize the data
        let deserialized_data = compressor.deserialize(serialized_data.as_slice()).await.unwrap();
        println!("Deserialized data {:?}", deserialized_data);

        // check and compare
        let original_data: Value = serde_json::from_slice(deserialized_data.as_slice()).unwrap();
        assert_eq!(data, original_data);
    }

    #[tokio::test]
    async fn test_utxo_serde() {
        let mut rng = thread_rng();
        let mut utxos = vec![];
        // generate utxos
        for _ in 0..100 {
            let txid = Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap().to_byte_array().to_vec();
            let vout = rng.gen_range(0..u32::MAX);
            let utxo = Utxo {
                outpoint: Some(OutPoint { txid, vout }),
                output: rng.gen::<u32>(),
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
            ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(prost_serialized.into())
                .unwrap();
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
            let vout = rng.gen_range(0..u32::MAX);
            let value = rng.gen_range(1..1_000_000);
            let utxo = Utxo {
                outpoint: Some(OutPoint { txid, vout }),
                output: rng.gen::<u32>(),
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
        let compressor = Compressor::new();
        let prost_serialized_compressed = compressor.compress(&prost_serialized).await.unwrap();
        println!("Compressed to bytes: {:?}", prost_serialized_compressed);

        // now decompress the prost message
        let prost_serialized_decompressed =
            compressor.decompress(&prost_serialized_compressed).await.unwrap();
        println!("Decompressed to bytes: {:?}", prost_serialized_decompressed);

        let prost_deserialized = ProstMessageSerdelizer::<GetAllUtxosResponse>::deserialize(
            prost_serialized_decompressed,
        )
        .unwrap();
        println!("Deserialized to bytes: {:?}", prost_deserialized);

        assert!(prost_utxos == prost_deserialized);
    }
}
