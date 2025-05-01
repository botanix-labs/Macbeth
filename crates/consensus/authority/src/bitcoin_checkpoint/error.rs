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
    RpcError { error: reth_btc_wallet::bitcoind::JsonRPCError, procedure_name: String },
    #[error("Strong confirmation depth must be greater than zero")]
    ZeroStrongConfirmationDepth,
    #[error("Weak checkpoints count {weak_checkpoints_count} is greater than strong confirmation depth {strong_confirmation_depth}")]
    WeakCheckpointsCountTooBig { weak_checkpoints_count: usize, strong_confirmation_depth: usize },
    #[error("Chain configuration values are too big: strong_confirmation_depth={strong_confirmation_depth}, historical_checkpoints_count={historical_checkpoints_count}, weak_checkpoints_count={weak_checkpoints_count}")]
    ChainParamsTooLarge {
        strong_confirmation_depth: usize,
        historical_checkpoints_count: usize,
        weak_checkpoints_count: usize,
    },
}
