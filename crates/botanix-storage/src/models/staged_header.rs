//! Models for staged headers with their associated pegins and pegouts.

use reth_codecs::{add_arbitrary_tests, Compact};
use reth_primitives::Header;
use serde::{Deserialize, Serialize};

/// A header with associated pegins and pegouts.
///
/// This structure represents a blockchain header along with its associated
/// Bitcoin pegin and pegout transaction data. It is used in the staged header
/// system to persist pegin/pegout data after finalizing a block, ensuring
/// that no Bitcoin bridge transactions are lost during block processing.
///
/// ## Usage
///
/// Staged headers are created when a block is finalized and contains Bitcoin
/// pegin or pegout operations. The header along with the extracted pegin/pegout
/// data is staged for later processing, allowing the system to handle Bitcoin
/// bridge operations reliably.
///
/// ## Fields
///
/// - `pegins`: Bitcoin-to-Botanix bridge operations found in this block
/// - `pegouts`: Botanix-to-Bitcoin bridge operations found in this block
/// - `header`: The original blockchain header containing these operations
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct HeaderWithPegs {
    /// The pegins associated with this header.
    ///
    /// Contains all Bitcoin pegin operations detected in this block.
    /// Each pegin represents Bitcoin being locked on the Bitcoin network
    /// to mint equivalent tokens on the Botanix network.
    pub pegins: Vec<PeginData>,

    /// The pegouts associated with this header.
    ///
    /// Contains all Botanix pegout operations detected in this block.
    /// Each pegout represents tokens being burned on the Botanix network
    /// to unlock equivalent Bitcoin on the Bitcoin network.
    pub pegouts: Vec<PegoutData>,

    /// The header to which these pegins and pegouts are associated.
    ///
    /// This is the original blockchain header that contained the transactions
    /// from which the pegin and pegout data was extracted.
    pub header: Header,
}

/// Pegin data associated with a header.
///
/// Represents a Bitcoin-to-Botanix bridge operation where Bitcoin is locked
/// on the Bitcoin network to mint equivalent tokens on the Botanix network.
/// This structure contains all the necessary information to process and verify
/// a pegin operation.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct PeginData {
    /// The Bitcoin transaction ID that contains the pegin output.
    ///
    /// This uniquely identifies the Bitcoin transaction that locked the Bitcoin
    /// for the pegin operation. Used for verification against the Bitcoin network.
    pub txid: Vec<u8>,

    /// The output index of the pegin output in the Bitcoin transaction.
    ///
    /// Specifies which output within the Bitcoin transaction contains the locked
    /// Bitcoin. Bitcoin transactions can have multiple outputs, so this index
    /// identifies the specific output used for the pegin.
    pub vout: u64,

    /// The value of the pegin output in satoshis.
    ///
    /// The amount of Bitcoin (in satoshis) that was locked on the Bitcoin network.
    /// This determines how many equivalent tokens should be minted on Botanix.
    /// Note: 1 Bitcoin = 100,000,000 satoshis.
    pub value: u64,

    /// The script that must be satisfied to claim the output.
    ///
    /// The Bitcoin script (scriptPubKey) that locks the Bitcoin. This script
    /// typically requires specific conditions to be met for the Bitcoin to be
    /// unlocked, ensuring proper bridge operation security.
    pub script_pubkey: Vec<u8>,

    /// Final destination address of the pegin (non-hex encoded).
    ///
    /// The Botanix network address where the equivalent tokens should be minted.
    /// This address is specified by the user when initiating the pegin operation.
    /// Stored as raw bytes rather than hex-encoded string for efficiency.
    pub eth_address: Vec<u8>,
}

/// Pegout data associated with a header.
///
/// Represents a Botanix-to-Bitcoin bridge operation where tokens are burned
/// on the Botanix network to unlock equivalent Bitcoin on the Bitcoin network.
/// This structure contains all the necessary information to process and execute
/// a pegout operation.
#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct PegoutData {
    /// The pegout identifier.
    ///
    /// A unique identifier for this pegout operation, used to track the pegout
    /// through its lifecycle from initiation to completion. This ID correlates
    /// the token burning on Botanix with the Bitcoin unlocking on Bitcoin network.
    pub pegout_id: Vec<u8>,

    /// The script that must be satisfied to claim the output.
    ///
    /// The Bitcoin script (scriptPubKey) that will control the unlocked Bitcoin.
    /// This typically encodes the destination Bitcoin address where the user
    /// wants to receive their Bitcoin after the pegout completes.
    pub script_pubkey: Vec<u8>,

    /// Amount to be pegged out.
    ///
    /// The amount of tokens (in the smallest unit) that were burned on Botanix
    /// and should be unlocked as Bitcoin. This amount determines how much Bitcoin
    /// will be released on the Bitcoin network.
    pub amount: u64,

    /// Height at which the pegout was requested.
    ///
    /// The block height at which this pegout operation was initiated.
    /// Used for ordering pegout operations and ensuring proper processing
    /// sequence, especially important for coordination with Bitcoin network.
    pub height: u64,
}
