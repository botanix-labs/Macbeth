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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::TransactionSigned;
    use std::io::Cursor;
    use tendermint_rpc::endpoint::tx;

    #[test]
    fn test_transactions_signed_from_bytes() {
        let mut tx1 = TransactionSigned::default();
        tx1.transaction.set_nonce(1);
        let mut tx2 = TransactionSigned::default();
        tx2.transaction.set_nonce(2);
        let mut tx3 = TransactionSigned::default();
        tx3.transaction.set_nonce(3);

        let mut buf1 = Vec::new();
        tx1.encode_enveloped(&mut buf1);
        let signed_tx1 = TransactionSigned::decode_enveloped(&mut buf1.as_slice()).unwrap();
        let bytes1 = prost::bytes::Bytes::copy_from_slice(buf1.as_slice());

        let mut buf2 = Vec::new();
        tx2.encode_enveloped(&mut buf2);
        let signed_tx2 = TransactionSigned::decode_enveloped(&mut buf2.as_slice()).unwrap();
        let bytes2 = prost::bytes::Bytes::copy_from_slice(buf2.as_slice());

        let mut buf3 = Vec::new();
        tx3.encode_enveloped(&mut buf3);
        let signed_tx3 = TransactionSigned::decode_enveloped(&mut buf3.as_slice()).unwrap();
        let bytes3 = prost::bytes::Bytes::copy_from_slice(buf3.as_slice());

        let vec_bytes = vec![bytes1, bytes2, bytes3];
        let txs = transactions_signed_from_bytes(vec_bytes.iter().cloned()).unwrap();

        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0], signed_tx1);
        assert_eq!(txs[1], signed_tx2);
        assert_eq!(txs[2], signed_tx3);
    }
}
