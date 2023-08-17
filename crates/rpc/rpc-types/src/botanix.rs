use reth_primitives::Address;
use serde::{Serialize, Deserialize};

/// Information about the Botanix Pegin Gateway Address
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayAddress {
    /// Bitcoin Pegin Address
    pub gateway_address: String,
    /// Aggregated public key used as internal taproot key
    pub aggregate_public_key: String,
    /// User provided nonce
    pub nonce: u64,
    /// User Eth account
    pub eth_address: Address,
}