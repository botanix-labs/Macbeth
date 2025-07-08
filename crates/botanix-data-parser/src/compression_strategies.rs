pub use async_compression::{
    tokio::write as compression_encoders_and_decoders, Level as CompressionLevel,
};
use tokio::io::AsyncWriteExt;

use crate::CompressionError;

/// A private module used to seal the `CompressionStrategy` trait. This ensures
/// that the trait cannot be implemented outside of this module.
mod private {
    pub trait Sealed {}
}

/// The `CompressionStrategy` trait defines the interface for compression and decompression
/// strategies. It is sealed to restrict external implementations.
#[async_trait::async_trait]
pub trait CompressionStrategy: private::Sealed + Sync + Send {
    /// Returns the name of the compression strategy.
    fn name(&self) -> &'static str;

    /// Compresses the provided data asynchronously.
    ///
    /// # Arguments
    ///
    /// * `uncompressed` - A slice of bytes representing the data to be compressed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `Vec<u8>` of the compressed data or a `CompressionError` if
    /// compression fails.
    async fn compress(&self, uncompressed: &[u8]) -> Result<Vec<u8>, CompressionError>;

    /// Decompresses the provided data asynchronously.
    ///
    /// # Arguments
    ///
    /// * `compressed` - A slice of bytes representing the data to be decompressed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `Vec<u8>` of the decompressed data or a `CompressionError` if
    /// decompression fails.
    async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>, CompressionError>;
}

/// A macro to define a new compression strategy by implementing the `CompressionStrategy` trait.
///
/// # Parameters
/// - `$name`: The name of the compression strategy (typically a unit struct).
/// - `$compression_type`: The compression type (e.g., ZLib, Gzip, Brotli, Bz, Lzma, Deflate, Zstd).
/// - `$compression_level`: The compression level to be used (using `CompressionLevel` enum).
///
/// # Example
/// struct TestCompressionStrategy;
/// define_compression_strategy!(TestCompressionStrategy, Zlib, CompressionLevel::Fastest);
macro_rules! define_compression_strategy {
    ($name:ident, $compression_type:ident, $compression_level:ty) => {
        impl private::Sealed for $name {}

        #[async_trait::async_trait]
        impl CompressionStrategy for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            async fn compress(&self, uncompressed: &[u8]) -> Result<Vec<u8>, CompressionError> {
                paste::paste! {
                    let mut encoder = compression_encoders_and_decoders::[<$compression_type Encoder>]::with_quality(Vec::new(), $compression_level);
                    encoder
                        .write_all(uncompressed)
                        .await
                        .map_err(|e| CompressionError::[<$compression_type>](e))?;
                    encoder
                        .shutdown()
                        .await
                        .map_err(|e| CompressionError::[<$compression_type>](e))?;
                    Ok(encoder.into_inner())
                }
            }

            async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>, CompressionError> {
                paste::paste! {
                    let mut decoder = compression_encoders_and_decoders::[<$compression_type Decoder>]::new(Vec::new());
                    decoder
                        .write_all(compressed)
                        .await
                        .map_err(|e| CompressionError::[<$compression_type>](e))?;
                    decoder
                        .shutdown()
                        .await
                        .map_err(|e| CompressionError::[<$compression_type>](e))?;
                    Ok(decoder.into_inner())
                }
            }
        }
    };
}

#[derive(Clone)]
pub struct ZLibCompressionStrategy;
define_compression_strategy!(ZLibCompressionStrategy, Zlib, CompressionLevel::Fastest);

#[derive(Clone)]
pub struct GzipCompressionStrategy;

define_compression_strategy!(GzipCompressionStrategy, Gzip, CompressionLevel::Fastest);

#[derive(Clone)]
pub struct BrotliCompressionStrategy;

define_compression_strategy!(BrotliCompressionStrategy, Brotli, CompressionLevel::Fastest);

#[derive(Clone)]
pub struct BzCompressionStrategy;

define_compression_strategy!(BzCompressionStrategy, Bz, CompressionLevel::Fastest);

#[derive(Clone)]
pub struct LzmaCompressionStrategy;

define_compression_strategy!(LzmaCompressionStrategy, Lzma, CompressionLevel::Fastest);

#[derive(Clone)]
pub struct DeflateCompressionStrategy;

define_compression_strategy!(DeflateCompressionStrategy, Deflate, CompressionLevel::Fastest);

#[derive(Clone)]
pub struct ZstdCompressionStrategy;

define_compression_strategy!(ZstdCompressionStrategy, Zstd, CompressionLevel::Fastest);

use std::sync::Arc;

lazy_static::lazy_static! {
    pub static ref DEFAULT_COMPRESSION_STRATEGY: Arc<dyn CompressionStrategy> =  Arc::new(ZLibCompressionStrategy);
}

lazy_static::lazy_static! {
    pub static ref ALL_COMPRESSION_STRATEGIES: [Arc<dyn CompressionStrategy>; 7] = [
        Arc::new(ZLibCompressionStrategy),
        Arc::new(GzipCompressionStrategy),
        Arc::new(BrotliCompressionStrategy),
        Arc::new(BzCompressionStrategy),
        Arc::new(LzmaCompressionStrategy),
        Arc::new(DeflateCompressionStrategy),
        Arc::new(ZstdCompressionStrategy),
    ];

}
