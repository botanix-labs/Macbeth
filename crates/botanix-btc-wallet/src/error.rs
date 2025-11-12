use thiserror::Error;

pub type BitcoindAdapterResult<T> = Result<T, BitcoindAdapterError>;

#[derive(Debug, thiserror::Error)]
pub enum BitcoindAdapterError {
    #[error("Bitcoind RPC error: {0}")]
    BitcoindRpc(#[from] BitcoindError),

    #[error("No clients available")]
    NoClientsAvailable,
}

#[derive(Debug, Error)]
pub enum BitcoindError {
    #[error("Client initialization failed")]
    ClientInitFailed(bitcoincore_rpc::Error),
    #[error("Block Header retrieval failed")]
    BlockHeaderRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Block Tip retrieval failed")]
    BlockTipRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Empty block tip")]
    EmptyBlockTip,
    #[error("Block hash retrieval failed")]
    BlockHashRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Tx broadcast failed")]
    TransactionBroadcastFailed(bitcoincore_rpc::Error),
    #[error("Block index failed")]
    BlockIndexStatusFailed(bitcoincore_rpc::Error),
    #[error("Blockchain index failed")]
    BlockchainInfoFailed(bitcoincore_rpc::Error),
    #[error("Best block hash retrieval failed")]
    BestBlockHashRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Block info retrieval failed")]
    BlockInfoRetrievalFailed(bitcoincore_rpc::Error),
    #[error("Smart estimate fee retrieval failed")]
    EstimateSmartFeeFailed(bitcoincore_rpc::Error),
    #[error("Failed to load wallet")]
    WalletLoadingFailed(bitcoincore_rpc::Error),
    #[error("Block count failed")]
    BlockCountFailed(bitcoincore_rpc::Error),
    #[error("Wallet creation failed")]
    WalletCreationFailed(bitcoincore_rpc::Error),
    #[error("Generate to address failed")]
    GenerateToAddressFailed(bitcoincore_rpc::Error),
    #[error("Get new address failed")]
    GetNewAddressFailed(bitcoincore_rpc::Error),
}
