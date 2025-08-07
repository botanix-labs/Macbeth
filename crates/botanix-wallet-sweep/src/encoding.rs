use botanix_data_parser::{DataParser, SerializationType, DEFAULT_COMPRESSION_STRATEGY};
use std::{
    cell::LazyCell,
    sync::{Arc, LazyLock},
};

pub static PARSER: LazyLock<DataParser> = LazyLock::new(|| {
    DataParser::default()
        .with_compression_strategy(&DEFAULT_COMPRESSION_STRATEGY)
        .with_serialization_type(SerializationType::Postcard)
});
