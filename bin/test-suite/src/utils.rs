use crate::it_info_print;
use bitcoin::hash_types::BlockHash;
use bitcoincore_rpc::RpcApi;
use std::time::Duration;

/// Generate `num_blocks` blocks on the given bitcoind instance
pub async fn generate_blocks(bitcoind: &impl RpcApi, num_blocks: u32) -> Vec<BlockHash> {
    let address = bitcoind.get_new_address(None, None).unwrap().assume_checked();
    let mut block_hashes = vec![];
    for _ in 0..num_blocks {
        // You could generate many blocks at once here but occassionally
        // We get a `SocketError`
        match bitcoind.generate_to_address(1, &address) {
            Ok(hashes) => {
                block_hashes.push(hashes);
            }
            Err(e) => {
                it_info_print!("Error generating blocks: {:?}", e);
                panic!("generate to address failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    block_hashes.into_iter().flatten().collect::<Vec<_>>()
}
