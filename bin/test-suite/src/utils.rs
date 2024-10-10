use crate::it_info_print;
use bitcoincore_rpc::RpcApi;
use std::time::Duration;

/// Generate `num_blocks` blocks on the given bitcoind instance
pub async fn generate_blocks(bitcoind: &impl RpcApi, num_blocks: u32) {
    let address = bitcoind.get_new_address(None, None).unwrap().assume_checked();
    for _ in 0..num_blocks {
        // You could generate many blocks at once here but occassionally
        // We get a `SocketError`
        match bitcoind.generate_to_address(1, &address) {
            Ok(_) => {}
            Err(e) => {
                it_info_print!("Error generating blocks: {:?}", e);
                panic!("generate to address failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
