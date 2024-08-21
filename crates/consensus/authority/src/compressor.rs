/// The api provides utilities for serializing and deserializing, as well as compression and
/// decompression of messages. It is being used by the utxo set syncing mechanism as well as
/// inside the blockcfetcher.
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
pub(crate) enum CompressionError {
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
pub(crate) enum SerdeError {
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
pub(crate) enum Error {
    /// compression error: {0}
    Compression(#[from] CompressionError),
    /// serde error: {0}
    Serde(#[from] SerdeError),
}

/// Compression types
#[derive(
    Debug, Copy, Clone, Deserialize, EnumString, AsRefStr, EnumIter, strum_macros::Display,
)]
pub(crate) enum CompressionType {
    /// No compression to be applied
    #[strum(serialize = "none")]
    None,
    /// Zlib compression
    #[strum(serialize = "zlib")]
    Zlib,
    /// Gzip compression
    #[strum(serialize = "gzip")]
    Gzip,
    /// Brotli compression
    #[strum(serialize = "brotli")]
    Brotli,
    /// Bz compression
    #[strum(serialize = "bz")]
    Bz,
    #[strum(serialize = "lzma")]
    /// Lzma compression
    Lzma,
    #[strum(serialize = "deflate")]
    /// Deflate compression
    Deflate,
    #[strum(serialize = "zstd")]
    /// Zstd compression
    Zstd,
}

/// Serialization types
#[derive(
    Debug, Copy, Clone, Deserialize, EnumString, AsRefStr, EnumIter, strum_macros::Display,
)]
pub(crate) enum SerializationType {
    /// Bincode serialization
    #[strum(serialize = "bincode")]
    Bincode,
    /// Postcard serialization
    #[strum(serialize = "postcard")]
    Postcard,
}

/// Prost Message Wrapper allowing serialization/deserialization
pub(crate) struct ProstMessageSerdelizer<T: prost::Message>(pub(crate) T);

impl<T> ProstMessageSerdelizer<T>
where
    T: prost::Message + std::default::Default,
{
    /// Method to serialize
    pub(crate) fn serialize(&self) -> Result<Vec<u8>, Error> {
        let mut buf = Vec::new();
        self.0.encode(&mut buf).map_err(|e| Error::Serde(SerdeError::ProstEncode(e)))?;
        Ok(buf)
    }

    /// Method to deserialize
    pub(crate) fn deserialize(buf: Vec<u8>) -> Result<T, Error> {
        //let x = Bytes::from(buf);
        T::decode(Bytes::from(buf)).map_err(|e| Error::Serde(SerdeError::ProstDecode(e)))
    }
}

macro_rules! define_compression_methods {
    ($($name:ident),+) => {
        paste::item! {
            $(
                pub(crate) async fn [<compress_ $name:lower>](&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
                    let mut encoder = [<$name Encoder>]::with_quality(Vec::new(), self.compression_level);
                    encoder.write_all(in_data).await.map_err(|e| Error::Compression(CompressionError::[<$name>](e)))?;
                    encoder.shutdown().await.map_err(|e| Error::Compression(CompressionError::[<$name>](e)))?;
                    Ok(encoder.into_inner())
                }

                pub(crate) async fn [<decompress_ $name:lower>](&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
                    let mut decoder = [<$name Decoder>]::new(Vec::new());
                    decoder.write_all(in_data).await.map_err(|e| Error::Compression(CompressionError::[<$name>](e)))?;
                    decoder.shutdown().await.map_err(|e| Error::Compression(CompressionError::[<$name>](e)))?;
                    Ok(decoder.into_inner())
                }
            )*
        }
    };
}

/// Compressor implementation
#[derive(Debug, Clone)]
pub(crate) struct Compressor {
    compression_type: CompressionType,
    compression_level: Level,
    serialization_type: SerializationType,
}

impl Compressor {
    /// Constructor for a new compressor
    pub(crate) fn new() -> Self {
        Self {
            compression_type: CompressionType::Zlib,
            compression_level: Level::Best,
            serialization_type: SerializationType::Bincode,
        }
    }

    // Macro invocation to generate methods
    define_compression_methods!(Zlib, Gzip, Brotli, Bz, Lzma, Deflate, Zstd);

    /// Sets the compression type
    pub(crate) fn set_compression_type(&mut self, compression_type: CompressionType) {
        self.compression_type = compression_type;
    }

    /// Sets the compression level
    pub(crate) fn set_compression_level(&mut self, compression_level: Level) {
        self.compression_level = compression_level;
    }

    /// Sets the serialization type
    pub(crate) fn set_serialization_type(&mut self, serialization_type: SerializationType) {
        self.serialization_type = serialization_type;
    }

    /// Serializes and compresses the data
    pub(crate) async fn serialize_and_compress(
        &self,
        in_data: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let serialized_data = self.serialize(in_data).await?;
        let compressed_data = self.compress(&serialized_data[..]).await?;
        Ok(compressed_data)
    }

    /// Decompresses and deserializes the data
    pub(crate) async fn decompress_and_deserialize(
        &self,
        in_data: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let decompressed_data = self.decompress(in_data).await?;
        let deserialized_data = self.deserialize(&decompressed_data[..]).await?;
        Ok(deserialized_data)
    }

    /// Serializes the data
    pub(crate) async fn serialize(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.serialization_type {
            SerializationType::Bincode => {
                bincode::serialize(in_data).map_err(|e| Error::Serde(SerdeError::Bincode(*e)))
            }
            SerializationType::Postcard => {
                postcard::to_allocvec(in_data).map_err(|e| Error::Serde(SerdeError::Postcard(e)))
            }
        }
    }

    /// Deserializes the data
    pub(crate) async fn deserialize(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.serialization_type {
            SerializationType::Bincode => {
                bincode::deserialize(in_data).map_err(|e| Error::Serde(SerdeError::Bincode(*e)))
            }
            SerializationType::Postcard => {
                postcard::from_bytes(in_data).map_err(|e| Error::Serde(SerdeError::Postcard(e)))
            }
        }
    }

    /// Compresses the data
    pub(crate) async fn compress(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.compression_type {
            CompressionType::None => Ok(in_data.to_vec()),
            CompressionType::Zlib => self.compress_zlib(in_data).await,
            CompressionType::Gzip => self.compress_gzip(in_data).await,
            CompressionType::Brotli => self.compress_brotli(in_data).await,
            CompressionType::Bz => self.compress_bz(in_data).await,
            CompressionType::Lzma => self.compress_lzma(in_data).await,
            CompressionType::Deflate => self.compress_deflate(in_data).await,
            CompressionType::Zstd => self.compress_zstd(in_data).await,
        }
    }

    /// Decompresses the data
    pub(crate) async fn decompress(&self, in_data: &[u8]) -> Result<Vec<u8>, Error> {
        match self.compression_type {
            CompressionType::None => Ok(in_data.to_vec()),
            CompressionType::Zlib => self.decompress_zlib(in_data).await,
            CompressionType::Gzip => self.decompress_gzip(in_data).await,
            CompressionType::Brotli => self.decompress_brotli(in_data).await,
            CompressionType::Bz => self.decompress_bz(in_data).await,
            CompressionType::Lzma => self.decompress_lzma(in_data).await,
            CompressionType::Deflate => self.decompress_deflate(in_data).await,
            CompressionType::Zstd => self.decompress_zstd(in_data).await,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::compressor::{Compressor, ProstMessageSerdelizer};
    use bitcoin::{hashes::Hash, Txid};
    use client::{GetAllUtxosResponse, TxOut, Utxo};
    use rand::{thread_rng, Rng};
    use serde_json::Value;

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
            let script_pubkey = rng.gen::<[u8; 32]>().to_vec();
            let vout = rng.gen_range(0..u32::MAX);
            let utxo = Utxo {
                outpoint: Some(client::OutPoint { txid: txid.clone(), vout }),
                output: Some(TxOut {
                    script_pubkey: Some(client::ScriptBuf { script: script_pubkey }),
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
            let script_pubkey = rng.gen::<[u8; 32]>().to_vec();
            let vout = rng.gen_range(0..u32::MAX);
            let utxo = Utxo {
                outpoint: Some(client::OutPoint { txid: txid.clone(), vout }),
                output: Some(TxOut {
                    script_pubkey: Some(client::ScriptBuf { script: script_pubkey }),
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
        let compressor = Compressor::new();
        let prost_serialized_compressed = compressor.compress(&prost_serialized).await.unwrap();
        println!(
            "Compressed to bytes: serialized: {:?} bytes, ser+compressed {:?} bytes",
            prost_serialized.len(),
            prost_serialized_compressed.len()
        );

        assert!(
            prost_serialized.len() > prost_serialized_compressed.len(),
            "serialzied message length is greater than the compressed length"
        );

        // now decompress the prost message
        let prost_serialized_decompressed =
            compressor.decompress(&prost_serialized_compressed).await.unwrap();
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
            prost_deserialized.utxos.len() > 0,
            "deserialized message length is greater than 0"
        );

        assert!(prost_utxos == prost_deserialized);
    }
}
