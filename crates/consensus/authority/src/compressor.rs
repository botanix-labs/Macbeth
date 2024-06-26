use async_compression::{
    tokio::write::{
        BrotliDecoder, BrotliEncoder, BzDecoder, BzEncoder, DeflateDecoder, DeflateEncoder,
        GzipDecoder, GzipEncoder, LzmaDecoder, LzmaEncoder, ZlibDecoder, ZlibEncoder, ZstdDecoder,
        ZstdEncoder,
    },
    Level,
};
use serde::Deserialize;
use strum::{AsRefStr, EnumIter, EnumString};
use tokio::io::AsyncWriteExt as _; // for `write_all` and `shutdown`

use displaydoc::Display as DisplayDoc;
use thiserror::Error;

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
    use crate::compressor::Compressor;
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
}
