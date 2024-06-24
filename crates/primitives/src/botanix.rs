//! Botanix Specific primitives

/// Helper type for the recent header
/// Second field is the height of the header
pub type RecentHeader = (bitcoin::block::Header, u32);

#[derive(Debug, Clone)]
/// Series of botanix specific consensus data
pub struct BotanixConsensusPackage {
    /// Deeply confirmed bitcoin header.
    pub bitcoin_checkpoint: RecentHeader,
    /// Aggregate public key
    pub aggregate_public_key: secp256k1::PublicKey,
    /// Bitcoin network
    pub btc_network: bitcoin::Network,
}
