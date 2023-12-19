use alloy_primitives::Address;

use serde::{Deserialize, Serialize};

/// Information about the Botanix Pegin Gateway Address
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayAddress {
    /// Bitcoin Pegin Address
    pub gateway_address: String,
    /// Aggregated public key used as internal taproot key
    pub aggregate_public_key: String,
    /// User Eth account
    pub eth_address: Address,
}
