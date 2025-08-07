use bitcoin::{address::NetworkUnchecked, Address, Network};
use bytes::Buf;
use reth_db_api::table::{Compress, Decompress};
use reth_primitives::{Bytes, B256};
use serde::{Deserialize, Serialize};
// TODO: TBD

pub type WalletSweepSessionId = B256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSweepSession {
    psbt_bytes: Bytes,
    bitcoin_network: Network,
    bitcoin_destination_address: Address<NetworkUnchecked>,
    created_at: u64,
}

impl Compress for WalletSweepSession {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        // Write PSBT bytes with length prefix
        buf.put_u32_le(self.psbt_bytes.len() as u32);
        buf.put(self.psbt_bytes.as_ref());

        // Write network magic (4 bytes)
        let serialized_network_magic = bitcoin::consensus::serialize(&self.bitcoin_network.magic());
        buf.put(serialized_network_magic.as_slice());

        // Write address with length prefix
        let destination_address_string =
            self.bitcoin_destination_address.assume_checked().to_string();
        let address_bytes = destination_address_string.into_bytes();
        buf.put_u32_le(address_bytes.len() as u32);
        buf.put(address_bytes.as_slice());

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

        // Read PSBT bytes
        if buf.remaining() < 4 {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let psbt_len = buf.get_u32_le() as usize;
        if buf.remaining() < psbt_len {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let psbt_bytes = buf.copy_to_bytes(psbt_len);
        let psbt_bytes = Bytes::from(psbt_bytes);

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

        // Read timestamp
        if buf.remaining() < 8 {
            return Err(reth_storage_errors::db::DatabaseError::Decode);
        }
        let created_at = buf.get_u64_le();

        Ok(Self { psbt_bytes, bitcoin_network, bitcoin_destination_address, created_at })
    }
}

impl WalletSweepSession {
    pub fn calculate_id(&self) -> WalletSweepSessionId {
        todo!()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UtxoOrdering {
    Lexicographic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusParameters {
    fee_rate_sat_vb: u64,
    utxo_ordering: UtxoOrdering,
    threshold_percent: u8,
    reachable_members: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utxo {}

impl TryFrom<btc_server_client::Utxo> for Utxo {
    type Error = eyre::Error;

    fn try_from(_value: btc_server_client::Utxo) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{address::NetworkUnchecked, Address, Network};
    use reth_db_api::table::{Compress, Decompress};
    use reth_primitives::Bytes;
    use std::str::FromStr;

    fn create_test_session() -> WalletSweepSession {
        let psbt_bytes = Bytes::from(vec![0x01, 0x02, 0x03, 0x04]);
        let bitcoin_network = Network::Bitcoin;
        let bitcoin_destination_address =
            Address::from_str("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2").unwrap().as_unchecked().clone();
        let created_at = 1234567890;

        WalletSweepSession { psbt_bytes, bitcoin_network, bitcoin_destination_address, created_at }
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
        assert_eq!(original.psbt_bytes, decompressed.psbt_bytes);
        assert_eq!(original.bitcoin_network, decompressed.bitcoin_network);
        assert_eq!(original.bitcoin_destination_address, decompressed.bitcoin_destination_address);
        assert_eq!(original.created_at, decompressed.created_at);
    }

    #[test]
    fn test_compress_decompress_empty_psbt() {
        let psbt_bytes = Bytes::new();
        let bitcoin_network = Network::Testnet;
        let bitcoin_destination_address =
            Address::from_str("bc1qwqdg6squsna38e46795at95yu9atm8azzmyvckulcc7kytlcckxswvvzej")
                .unwrap()
                .as_unchecked()
                .clone();
        let created_at = 0;

        let session = WalletSweepSession {
            psbt_bytes,
            bitcoin_network,
            bitcoin_destination_address,
            created_at,
        };

        let mut buffer = Vec::new();
        session.clone().compress_to_buf(&mut buffer);
        let decompressed = WalletSweepSession::decompress(&buffer).unwrap();

        assert_eq!(session.psbt_bytes, decompressed.psbt_bytes);
        assert_eq!(session.bitcoin_network, decompressed.bitcoin_network);
        assert_eq!(session.bitcoin_destination_address, decompressed.bitcoin_destination_address);
        assert_eq!(session.created_at, decompressed.created_at);
    }

    #[test]
    fn test_compress_decompress_large_psbt() {
        let psbt_bytes = Bytes::from(vec![0xFF; 1000]);
        let bitcoin_network = Network::Regtest;
        let bitcoin_destination_address =
            Address::from_str("bc1qwqdg6squsna38e46795at95yu9atm8azzmyvckulcc7kytlcckxswvvzej")
                .unwrap()
                .as_unchecked()
                .clone();
        let created_at = u64::MAX;

        let session = WalletSweepSession {
            psbt_bytes,
            bitcoin_network,
            bitcoin_destination_address,
            created_at,
        };

        let mut buffer = Vec::new();
        session.clone().compress_to_buf(&mut buffer);
        let decompressed = WalletSweepSession::decompress(&buffer).unwrap();

        assert_eq!(session.psbt_bytes, decompressed.psbt_bytes);
        assert_eq!(session.bitcoin_network, decompressed.bitcoin_network);
        assert_eq!(session.bitcoin_destination_address, decompressed.bitcoin_destination_address);
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
            let psbt_bytes = Bytes::from(vec![0x42]);
            let bitcoin_destination_address =
                Address::from_str(address_str).unwrap().as_unchecked().clone();
            let created_at = 1234567890;

            let session = WalletSweepSession {
                psbt_bytes,
                bitcoin_network: *network,
                bitcoin_destination_address,
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
        let psbt_bytes = Bytes::from(vec![0x01, 0x02]);
        let bitcoin_network = Network::Bitcoin;
        let created_at: u64 = 1234567890;

        let mut buffer = Vec::new();

        // Write PSBT
        buffer.extend_from_slice(&(psbt_bytes.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&psbt_bytes);

        // Write network
        let network_magic = bitcoin::consensus::serialize(&bitcoin_network.magic());
        buffer.extend_from_slice(&network_magic);

        // Write invalid address
        let invalid_address = b"not-a-valid-address";
        buffer.extend_from_slice(&(invalid_address.len() as u32).to_le_bytes());
        buffer.extend_from_slice(invalid_address);

        // Write timestamp
        buffer.extend_from_slice(&created_at.to_le_bytes());

        // This should fail due to invalid address
        assert!(WalletSweepSession::decompress(&buffer).is_err());
    }
}
