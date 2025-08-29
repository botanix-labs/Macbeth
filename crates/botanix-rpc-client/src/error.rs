use reth_rpc_server_types::result::internal_rpc_err;

/// Errors that can occur when interacting with the `eth_` namespace
#[derive(Debug, thiserror::Error)]
pub enum BotanixEthApiError {
    /// Errors related to invalid transactions
    #[error("error getting the gateway address")]
    GatewayAddress,
    /// Errors when getting the merkle proof of all utxos from the btc-server
    #[error("error getting the merkle root of all utxos")]
    GetMerkleProof,
    /// Error when getting the btc fee from the btc-server
    #[error("error getting the btc fee")]
    GetBtcFee,
    /// Error when getting the aggregate public key from Frost
    #[error("error getting aggregate public key")]
    GetAggregatePublicKey,
}

impl From<BotanixEthApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(error: BotanixEthApiError) -> Self {
        match error {
            BotanixEthApiError::GatewayAddress |
            BotanixEthApiError::GetMerkleProof |
            BotanixEthApiError::GetBtcFee |
            BotanixEthApiError::GetAggregatePublicKey => internal_rpc_err(error.to_string()),
        }
    }
}
