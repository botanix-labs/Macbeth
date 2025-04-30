use bitcoin::block::BlockHash as BitcoinBlockHash;

#[derive(thiserror::Error, Debug)]
pub enum BitcoinCheckpointError {
    #[error(
        "Block from other chain added: expected previous block hash {expected_prev_block_hash}, found {added_prev_block_hash}"
    )]
    StaleBlockAdded {
        expected_prev_block_hash: BitcoinBlockHash,
        added_prev_block_hash: BitcoinBlockHash,
    },
    #[error("RPC call {procedure_name} error: {error}")]
    RpcError { error: reth_btc_wallet::bitcoincore_rpc::Error, procedure_name: String },
}
