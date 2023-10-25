use botanix_lib::extra_data_header::ExtraDataHeader;
use reth_primitives::{Header, H256};

pub enum CreateSigHashError {
    
}

/// Create sighash for authority to sign
pub fn create_authority_sighash(header: &Header, extra_data: &ExtraDataHeader) -> Result<H256, CreateSigHashError> {
    header.extra_data = extra_data.serialize_without_signature().as_slice();

    Ok(header.hash_slow())
}
