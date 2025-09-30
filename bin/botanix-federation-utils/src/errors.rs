use ethers::providers::ProviderError;
use std::io;
use tendermint_rpc::Error as TenderMintError;
use thiserror::Error;

/// wallet error struct
#[derive(Debug, Error)]
pub enum WalletError {
    /// Error when an RPC call fails.
    #[error("RPC Error: {0}")]
    RpcError(String),

    /// Error related to input/output operations.
    #[error("I/O Error: {0}")]
    IoError(String),

    /// Error in the command-line interface.
    #[error("CLI Error: {0}")]
    CliError(String),

    /// Custom error for generic use.
    #[error("{0}")]
    CustomError(String),

    /// Error during gas calculation.
    #[error("Gas calculation error: {0}")]
    GasError(String),

    /// Error related to balance operations.
    #[error("Balance error: {0}")]
    BalanceError(String),

    /// Error when an invalid address is used.
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    /// Error when an config file is invalid.
    #[error("Config.toml error: {0}")]
    ConfigLoadError(String),

    /// Error when an config file is invalid.
    #[error("Config.toml error: {0}")]
    TransactionNotFound(String),

    /// Tendermint RPC client error.
    #[error("Tendermint RPC client error: {0}")]
    Tendermint(#[from] TenderMintError),
}

impl From<io::Error> for WalletError {
    fn from(err: io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

impl From<ProviderError> for WalletError {
    fn from(err: ProviderError) -> Self {
        Self::RpcError(err.to_string())
    }
}

impl From<&str> for WalletError {
    fn from(message: &str) -> Self {
        Self::CustomError(message.to_string())
    }
}
