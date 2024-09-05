use reth_primitives::TransactionSigned;
use tracing::error;

/// Convert bytes to [TransactionSigned] using an iterator
/// Iterator is passed in case some bytes need to be skipped
/// For example, if the first Bytes are non-deterministic data
pub fn transactions_signed_from_bytes(
    bytes: impl Iterator<Item = prost::bytes::Bytes>,
) -> Result<Vec<TransactionSigned>, Box<dyn std::error::Error>> {
    let mut txs = Vec::new();
    for tx in bytes {
        match TransactionSigned::decode_enveloped(&mut tx.to_vec().as_slice()) {
            Ok(signed_tx) => txs.push(signed_tx),
            Err(e) => {
                error!("Error decoding signed transaction: {:?}", e);
                return Err(Box::new(e));
            }
        }
    }

    Ok(txs)
}
