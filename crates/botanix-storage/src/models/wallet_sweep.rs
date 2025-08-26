use bitcoin::{address::NetworkUnchecked, Address, Network};
use bytes::Buf;
use reth_db_api::table::{Compress, Decompress};
use reth_primitives::{keccak256, B256};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
// TODO: TBD

/// Unique identifier for a wallet sweep session.
///
/// This is a 256-bit identifier used to uniquely identify wallet sweep sessions
/// in the storage system.
pub type WalletSweepSessionId = B256;

/// Represents a wallet sweep session that tracks the state of a Bitcoin wallet sweep operation.
///
/// A wallet sweep session contains information about the Bitcoin network being used,
/// the destination address for the swept funds, the fee rate, and when the session was created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSweepSession {
    /// The Bitcoin network (mainnet, testnet, regtest, or signet) for this sweep operation.
    pub bitcoin_network: Network,
    /// The destination Bitcoin address where swept funds will be sent.
    pub bitcoin_destination_address: Address<NetworkUnchecked>,
    /// Fee rate in satoshis per virtual byte for the sweep transaction.
    pub fee_rate_sat_vb: u64,
    /// Unix timestamp when this session was created.
    pub created_at: u64,
}

impl Compress for WalletSweepSession {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        // Write network magic (4 bytes)
        let serialized_network_magic = bitcoin::consensus::serialize(&self.bitcoin_network.magic());
        buf.put(serialized_network_magic.as_slice());

        // Write address with length prefix
        let destination_address_string =
            self.bitcoin_destination_address.assume_checked().to_string();
        let address_bytes = destination_address_string.into_bytes();
        buf.put_u32_le(address_bytes.len() as u32);
        buf.put(address_bytes.as_slice());

        // Write fee rate (8 bytes)
        buf.put_u64_le(self.fee_rate_sat_vb);

        // Write timestamp
        buf.put_u64_le(self.created_at);
    }
}

impl Decompress for WalletSweepSession {
    fn decompress<B: AsRef<[u8]>>(
        value: B,
    ) -> Result<Self, reth_storage_errors::db::DatabaseError> {
        use std::str::FromStr;

        let mut buf = value.as_ref();

        // Read network magic (4 bytes)
        if buf.remaining() < 4 {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let network_magic_bytes = buf.copy_to_bytes(4);
        let network_magic = bitcoin::consensus::deserialize(&network_magic_bytes)
            .map_err(|_| reth_storage_errors::db::DatabaseError::Decode)?;
        let bitcoin_network = bitcoin::Network::from_magic(network_magic)
            .ok_or(reth_storage_errors::db::DatabaseError::Decode)?;

        // Read address
        if buf.remaining() < 4 {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let address_len = buf.get_u32_le() as usize;
        if buf.remaining() < address_len {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let address_bytes = buf.copy_to_bytes(address_len);
        let address_str = std::str::from_utf8(&address_bytes)
            .map_err(|_| reth_storage_errors::db::DatabaseError::Decode)?;

        let bitcoin_destination_address = Address::from_str(address_str)
            .map_err(|_| reth_storage_errors::db::DatabaseError::Decode)?;

        // Read fee rate (8 bytes)
        if buf.remaining() < 8 {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let fee_rate_sat_vb = buf.get_u64_le();

        // Read timestamp
        if buf.remaining() < 8 {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let created_at = buf.get_u64_le();

        Ok(Self { bitcoin_network, bitcoin_destination_address, fee_rate_sat_vb, created_at })
    }
}

impl WalletSweepSession {
    /// Calculates a unique identifier for this wallet sweep session.
    ///
    /// This method generates a deterministic ID based on the session's properties,
    /// which can be used to uniquely identify this session in storage.
    ///
    /// # Returns
    ///
    /// A unique identifier for this wallet sweep session.
    pub fn calculate_id(&self) -> WalletSweepSessionId {
        // Create a deterministic hash based on session properties
        let mut data = Vec::new();

        // Include network magic
        data.extend_from_slice(&self.bitcoin_network.magic().to_bytes());

        // Include destination address
        let address_str = self.bitcoin_destination_address.clone().assume_checked().to_string();
        data.extend_from_slice(address_str.as_bytes());

        // Include fee rate
        data.extend_from_slice(&self.fee_rate_sat_vb.to_le_bytes());

        // Include timestamp
        data.extend_from_slice(&self.created_at.to_le_bytes());

        // Generate deterministic hash
        keccak256(data)
    }
}

/// Defines the ordering strategy for UTXOs during wallet sweep operations.
///
/// This enum specifies how UTXOs should be ordered when processing them
/// during a wallet sweep operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UtxoOrdering {
    /// Orders UTXOs lexicographically by their identifiers.
    Lexicographic,
}

/// Configuration parameters that must be agreed upon by consensus for wallet sweep operations.
///
/// These parameters define the fee rate, UTXO ordering strategy, threshold requirements,
/// and which federation members are reachable for the sweep operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusParameters {
    /// Fee rate in satoshis per virtual byte for the sweep transaction.
    fee_rate_sat_vb: u64,
    /// The ordering strategy to use for UTXOs during the sweep.
    utxo_ordering: UtxoOrdering,
    /// Percentage threshold required for consensus (0-100).
    threshold_percent: u8,
    /// List of federation member IDs that are reachable for this operation.
    reachable_members: Vec<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Address, Network};
    use reth_db_api::table::{Compress, Decompress};
    use std::str::FromStr;

    fn create_test_session() -> WalletSweepSession {
        let bitcoin_network = Network::Bitcoin;
        let bitcoin_destination_address =
            Address::from_str("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2").unwrap().as_unchecked().clone();
        let fee_rate_sat_vb = 1000;
        let created_at = 1234567890;

        WalletSweepSession {
            bitcoin_network,
            bitcoin_destination_address,
            fee_rate_sat_vb,
            created_at,
        }
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let original = create_test_session();
        let mut buffer = Vec::new();

        // Compress
        original.clone().compress_to_buf(&mut buffer);

        // Ensure we wrote some data
        assert!(!buffer.is_empty());

        // Decompress
        let decompressed = WalletSweepSession::decompress(&buffer).unwrap();

        // Verify all fields match
        assert_eq!(original.bitcoin_network, decompressed.bitcoin_network);
        assert_eq!(original.bitcoin_destination_address, decompressed.bitcoin_destination_address);
        assert_eq!(original.fee_rate_sat_vb, decompressed.fee_rate_sat_vb);
        assert_eq!(original.created_at, decompressed.created_at);
    }

    #[test]
    fn test_compress_decompress_empty_psbt() {
        let bitcoin_network = Network::Testnet;
        let bitcoin_destination_address =
            Address::from_str("bc1qwqdg6squsna38e46795at95yu9atm8azzmyvckulcc7kytlcckxswvvzej")
                .unwrap()
                .as_unchecked()
                .clone();
        let fee_rate_sat_vb = 0;
        let created_at = 0;

        let session = WalletSweepSession {
            bitcoin_network,
            bitcoin_destination_address,
            fee_rate_sat_vb,
            created_at,
        };

        let mut buffer = Vec::new();
        session.clone().compress_to_buf(&mut buffer);
        let decompressed = WalletSweepSession::decompress(&buffer).unwrap();

        assert_eq!(session.bitcoin_network, decompressed.bitcoin_network);
        assert_eq!(session.bitcoin_destination_address, decompressed.bitcoin_destination_address);
        assert_eq!(session.fee_rate_sat_vb, decompressed.fee_rate_sat_vb);
        assert_eq!(session.created_at, decompressed.created_at);
    }

    #[test]
    fn test_compress_decompress_large_psbt() {
        let bitcoin_network = Network::Regtest;
        let bitcoin_destination_address =
            Address::from_str("bc1qwqdg6squsna38e46795at95yu9atm8azzmyvckulcc7kytlcckxswvvzej")
                .unwrap()
                .as_unchecked()
                .clone();
        let fee_rate_sat_vb = u64::MAX;
        let created_at = u64::MAX;

        let session = WalletSweepSession {
            bitcoin_network,
            bitcoin_destination_address,
            fee_rate_sat_vb,
            created_at,
        };

        let mut buffer = Vec::new();
        session.clone().compress_to_buf(&mut buffer);
        let decompressed = WalletSweepSession::decompress(&buffer).unwrap();

        assert_eq!(session.bitcoin_network, decompressed.bitcoin_network);
        assert_eq!(session.bitcoin_destination_address, decompressed.bitcoin_destination_address);
        assert_eq!(session.fee_rate_sat_vb, decompressed.fee_rate_sat_vb);
        assert_eq!(session.created_at, decompressed.created_at);
    }

    #[test]
    fn test_compress_decompress_different_networks() {
        let networks = [Network::Bitcoin, Network::Testnet, Network::Regtest, Network::Signet];
        let addresses = [
            "1QJVDzdqb1VpbDK7uDeyVXy9mR27CJiyhY",
            "33iFwdLuRpW1uK1RTRqsoi8rR4NpDzk66k",
            "bc1qvzvkjn4q3nszqxrv3nraga2r822xjty3ykvkuw",
            "bc1qwqdg6squsna38e46795at95yu9atm8azzmyvckulcc7kytlcckxswvvzej",
        ];

        for (network, address_str) in networks.iter().zip(addresses.iter()) {
            let bitcoin_destination_address =
                Address::from_str(address_str).unwrap().as_unchecked().clone();
            let fee_rate_sat_vb = 1000;
            let created_at = 1234567890;

            let session = WalletSweepSession {
                bitcoin_network: *network,
                bitcoin_destination_address,
                fee_rate_sat_vb,
                created_at,
            };

            let mut buffer = Vec::new();
            session.clone().compress_to_buf(&mut buffer);
            let decompressed = WalletSweepSession::decompress(&buffer).unwrap();

            assert_eq!(session.bitcoin_network, decompressed.bitcoin_network);
            assert_eq!(
                session.bitcoin_destination_address,
                decompressed.bitcoin_destination_address
            );
            assert_eq!(session.fee_rate_sat_vb, decompressed.fee_rate_sat_vb);
        }
    }

    #[test]
    fn test_decompress_insufficient_data() {
        // Test with buffer too small for length prefix
        let short_buffer = vec![0x01, 0x02];
        assert!(WalletSweepSession::decompress(&short_buffer).is_err());

        // Test with length prefix but not enough data
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&10u32.to_le_bytes()); // claim 10 bytes
        buffer.extend_from_slice(&[0x01, 0x02]); // but only provide 2
        assert!(WalletSweepSession::decompress(&buffer).is_err());
    }

    #[test]
    fn test_decompress_invalid_address() {
        let bitcoin_network = Network::Bitcoin;
        let fee_rate_sat_vb: u64 = 1000;
        let created_at: u64 = 1234567890;

        let mut buffer = Vec::new();

        // Write network
        let network_magic = bitcoin::consensus::serialize(&bitcoin_network.magic());
        buffer.extend_from_slice(&network_magic);

        // Write invalid address
        let invalid_address = b"not-a-valid-address";
        buffer.extend_from_slice(&(invalid_address.len() as u32).to_le_bytes());
        buffer.extend_from_slice(invalid_address);

        // Write fee rate
        buffer.extend_from_slice(&fee_rate_sat_vb.to_le_bytes());

        // Write timestamp
        buffer.extend_from_slice(&created_at.to_le_bytes());

        // This should fail due to invalid address
        assert!(WalletSweepSession::decompress(&buffer).is_err());
    }
}
