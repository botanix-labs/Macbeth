use bitcoin::consensus::encode::{self, Decodable, Encodable};
use reth_primitives::Address;
use std::io::{self, Write};
use thiserror::Error;

/// Errors that can occur when deserializing NonDeterministicData
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NonDeterministicDataDeserializeError {
    #[error("I/O error")]
    /// I/O error
    Io(#[from] io::Error),
    #[error("invalid data format")]
    /// Invalid data format
    Decoding(#[from] encode::Error),
    /// Invalid inclusion indicator, equivalent to Some/None
    #[error("invalid inclusion indicator")]
    InclusionIndicator,
    /// // Invalid network upgrade vote
    #[error("invalid network upgrade vote")]
    NetworkUpgradePayloadVote,
}

/// The implied Botanix runtime version at mainnet launch, created
/// retroactively.
pub const GENESIS_RUNTIME_VERSION: (u16, u16) = (0, 1);

/// Represents a validator's vote on a network upgrade proposal.
///
/// Validators can explicitly vote in favor of an upgrade (`Aye`),
/// against an upgrade (`Nay`), or can abstain from voting (`Absent`).
///
/// Votes are included in block proposals via the `NetworkUpgradePayload`
/// in the Non-Deterministic Data (NDD) transaction. These votes are then
/// tracked by the activation manager to calculate support thresholds.
///
/// The default vote is `Nay`, indicating that validators must explicitly
/// opt-in to upgrades rather than being opted-in by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote {
    /// Vote in favor of the upgrade. An `Aye` vote contributes to the signaling
    /// threshold calculations.
    Aye = 2,

    /// Vote against the upgrade. A `Nay` vote allows validators to signal
    /// opposition while still being counted in voting statistics.
    Nay = 1,

    /// Explicit abstention from voting. An `Absent` vote functions the same as
    /// `Nay` in quorum calculations, but communicates the validator's intent to
    /// abstain rather than actively oppose the upgrade. It still counts as
    /// participation in the voting process.
    Absent = 0,
}

/// Represents a validator's stance on a network upgrade proposal.
///
/// This payload is included in each block's non-deterministic data when a node is
/// configured to participate in the network upgrade voting process. It communicates
/// the validator's current position on a specific upgrade version.
///
/// # Fields
///
/// * `version` - The specific runtime version that this vote applies to.
///
/// * `vote` - The validator's explicit opinion on the upgrade (Aye/Nay/Absent).
///
/// * `is_compliant` - Indicates whether the validator is technically ready to process blocks with
///   the upgrade version. When `true`, the validator has the necessary software version and
///   configuration to handle the upgrade. This can be independent of the vote - a validator may
///   vote `Nay` but still be prepared to follow the network if the upgrade is adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkUpgradePayload {
    /// The runtime version that this vote applies to.
    pub version: (u16, u16),
    /// The validator's explicit opinion on the upgrade (Aye/Nay/Absent).
    pub vote: Vote,
    /// Indicates whether the validator is technically ready to process blocks with the upgrade
    /// version.
    pub is_compliant: bool,
}

pub const VERSION_1: u16 = 1;
pub const VERSION_2: u16 = 2;

/// Type that encapsulates non-deterministic data needed for consensus.
#[derive(Debug, Clone, PartialEq)]
pub struct NonDeterministicData {
    pub version: u16,
    pub bitcoin_block_hash: bitcoin::hash_types::BlockHash,
    pub aggregated_public_key: secp256k1::PublicKey,
    pub block_fee_recipient_address: Address,
    pub runtime_version: (u16, u16),
    pub network_upgrade_payload: Option<NetworkUpgradePayload>,
}

impl NonDeterministicData {
    /// Returns the version of the non-deterministic data structure.
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Returns the version of the Botanix runtime logic.
    pub fn runtime_version(&self) -> (u16, u16) {
        self.runtime_version
    }

    /// Constructor for the non-deterministic data (version 2).
    pub fn new(
        bitcoin_block_hash: bitcoin::hash_types::BlockHash,
        aggregated_public_key: secp256k1::PublicKey,
        block_fee_recipient_address: Address,
        runtime_version: (u16, u16),
        network_upgrade_payload: Option<NetworkUpgradePayload>,
    ) -> Self {
        Self {
            version: VERSION_2,
            bitcoin_block_hash,
            aggregated_public_key,
            block_fee_recipient_address,
            runtime_version,
            network_upgrade_payload,
        }
    }

    /// Serializes the non-deterministic data.
    pub fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        let mut writer = Vec::new();
        self.bitcoin_block_hash.consensus_encode(&mut writer)?;
        self.aggregated_public_key.serialize().consensus_encode(&mut writer)?;
        self.version().consensus_encode(&mut writer)?;
        writer.write_all(self.block_fee_recipient_address.as_slice())?;

        // Serialize runtime version.
        self.runtime_version.consensus_encode(&mut writer)?;

        // Serialize network upgrade payload, if available.
        if let Some(p) = &self.network_upgrade_payload {
            1u8.consensus_encode(&mut writer)?; // ~= Some(_)
            p.version.consensus_encode(&mut writer)?;
            (p.vote as u8).consensus_encode(&mut writer)?;
            p.is_compliant.consensus_encode(&mut writer)?;
        } else {
            0u8.consensus_encode(&mut writer)?; // ~= None
        }

        Ok(writer)
    }

    /// Deserializes the non-deterministic data.
    pub fn deserialize(
        reader: &mut impl bitcoin::io::Read,
    ) -> Result<Self, NonDeterministicDataDeserializeError> {
        // Read the bitcoin block hash.
        let bitcoin_block_hash = Decodable::consensus_decode(reader)?;

        // Read the aggregated public key.
        let pk_bytes = <[u8; 33]>::consensus_decode(reader)?;
        let aggregated_public_key = secp256k1::PublicKey::from_slice(&pk_bytes)
            .map_err(|_e| encode::Error::ParseFailed("malformed aggregate public key"))?;

        // Read the version and conditionally read the address.
        let version = u16::consensus_decode(reader)?;

        // Read the block fee recipient address.
        let mut address_bytes = [0u8; 20];
        reader
            .read_exact(&mut address_bytes)
            .map_err(|_e| encode::Error::ParseFailed("malformed block fee recipient address"))?;
        let block_fee_recipient_address = Address::from(address_bytes);

        let mut this = Self {
            version,
            bitcoin_block_hash,
            aggregated_public_key,
            block_fee_recipient_address,
            runtime_version: GENESIS_RUNTIME_VERSION,
            network_upgrade_payload: None,
        };

        match version {
            VERSION_2 => {
                // Decode runtime version.
                this.runtime_version = <(u16, u16)>::consensus_decode(reader)?;

                // Check whether the network upgrade payload is included.
                match u8::consensus_decode(reader)? {
                    0 => {
                        // Is NOT included, return.
                        return Ok(this)
                    }
                    1 => {
                        // Is included, proceed...
                    }
                    _ => return Err(NonDeterministicDataDeserializeError::InclusionIndicator),
                }

                // Decode payload information
                let version = <(u16, u16)>::consensus_decode(reader)?;
                let vote = match u8::consensus_decode(reader)? {
                    0 => Vote::Absent,
                    1 => Vote::Nay,
                    2 => Vote::Aye,
                    _ => {
                        return Err(NonDeterministicDataDeserializeError::NetworkUpgradePayloadVote)
                    }
                };
                let is_compliant = bool::consensus_decode(reader)?;

                this.network_upgrade_payload =
                    Some(NetworkUpgradePayload { version, vote, is_compliant });

                Ok(this)
            }
            _ => {
                // IMPORTANT: This is a versioning mechanism designed for
                // forward compatibility. We want to support new,
                // backwards-compatible versions with enhanced functionality
                // without requiring coordinated upgrades or hard-forks.
                //
                // When an older node encounters a newer, unknown version:
                // 1. It extracts the data it can understand based on the known structure
                // 2. It ignores any trailing data that may be present in newer versions
                // 3. It continues to function normally despite version differences
                //
                // By design, encountering an unknown version alone MUST NEVER
                // cause an error. The caller is responsible for validating
                // whether the returned NDD is appropriate for their specific
                // business logic requirements.
                Ok(this)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{hashes::Hash, BlockHash};

    #[test]
    fn test_non_deterministic_data_new() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        let runtime_version = (1, 2);

        let ndd = NonDeterministicData::new(
            bitcoin_block_hash,
            pk,
            block_fee_recipient_address,
            runtime_version,
            None,
        );

        assert_eq!(ndd.version, VERSION_2);
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
        assert_eq!(ndd.runtime_version, runtime_version);
        assert_eq!(ndd.network_upgrade_payload, None);
    }

    #[test]
    fn test_non_deterministic_data_serde() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        let runtime_version = (1, 2);

        let ndd = NonDeterministicData::new(
            bitcoin_block_hash,
            pk,
            block_fee_recipient_address,
            runtime_version,
            None,
        );
        let mut reader = io::Cursor::new(ndd.serialize().unwrap());

        let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
        assert_eq!(deserialized.version, ndd.version);
        assert_eq!(deserialized.bitcoin_block_hash, ndd.bitcoin_block_hash);
        assert_eq!(deserialized.aggregated_public_key, ndd.aggregated_public_key);
        assert_eq!(deserialized.block_fee_recipient_address, ndd.block_fee_recipient_address);
        assert_eq!(deserialized.runtime_version, ndd.runtime_version);
        assert_eq!(deserialized.network_upgrade_payload, ndd.network_upgrade_payload);
    }

    #[test]
    fn test_non_deterministic_data_serde_with_network_upgrade_payload() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        let runtime_version = (1, 2);

        let check_ndd = |network_upgrade_payload: NetworkUpgradePayload| {
            // Create NDD with network upgrade payload
            let ndd = NonDeterministicData::new(
                bitcoin_block_hash,
                pk,
                block_fee_recipient_address,
                runtime_version,
                Some(network_upgrade_payload.clone()),
            );

            let res = ndd.serialize().unwrap();
            let mut reader = io::Cursor::new(res);

            let deserialized = NonDeterministicData::deserialize(&mut reader).unwrap();
            assert_eq!(deserialized.version, ndd.version);
            assert_eq!(deserialized.bitcoin_block_hash, ndd.bitcoin_block_hash);
            assert_eq!(deserialized.aggregated_public_key, ndd.aggregated_public_key);
            assert_eq!(deserialized.block_fee_recipient_address, ndd.block_fee_recipient_address);
            assert_eq!(deserialized.runtime_version, runtime_version);
            // Network upgrade payload must be deserialized correctly!
            assert_eq!(deserialized.network_upgrade_payload, ndd.network_upgrade_payload);
            assert_eq!(deserialized.network_upgrade_payload, Some(network_upgrade_payload));
        };

        #[rustfmt::skip]
        check_ndd(NetworkUpgradePayload {
            version: (0, 0),
            vote: Vote::Absent,
            is_compliant: true,
        });

        #[rustfmt::skip]
        check_ndd(NetworkUpgradePayload {
            version: (5, 4),
            vote: Vote::Nay,
            is_compliant: false
        });

        #[rustfmt::skip]
        check_ndd(NetworkUpgradePayload {
            version: (50, 40),
            vote: Vote::Aye,
            is_compliant: true
        });
    }

    #[test]
    /// Attempts to deserialize a NDD with an unknown version containing some
    /// tailing data.
    fn test_non_deterministic_data_serde_unknown_version() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk: secp256k1::PublicKey = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        // NOTE: The last 32 bytes (all 1's) are ignored.
        let bytes = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000000\
             039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d\
             6300\
             43c8bdcb9afebb1d834a7de18cc214a6fd1632d9\
             1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();

        let mut reader = io::Cursor::new(bytes);
        let ndd = NonDeterministicData::deserialize(&mut reader).unwrap();

        assert_eq!(ndd.version, 99); // Version 99 is unknown
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
        assert_eq!(ndd.runtime_version, GENESIS_RUNTIME_VERSION);
        assert_eq!(ndd.network_upgrade_payload, None);
    }

    #[test]
    /// Attempts to deserialize a raw NDD using version 1.
    ///
    /// This is primarily intended for future NDD versions to ensure backwards
    /// compatibility.
    fn test_non_deterministic_data_serde_version_1() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk: secp256k1::PublicKey = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        let bytes = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000000\
             039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d\
             0100\
             43c8bdcb9afebb1d834a7de18cc214a6fd1632d9",
        )
        .unwrap();

        let mut reader = io::Cursor::new(bytes);
        let ndd = NonDeterministicData::deserialize(&mut reader).unwrap();

        assert_eq!(ndd.version, VERSION_1);
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
        assert_eq!(ndd.runtime_version, GENESIS_RUNTIME_VERSION);
        assert_eq!(ndd.network_upgrade_payload, None);
    }

    #[test]
    /// Attempts to deserialize a raw NDD using version 2, without network
    /// upgrade payload.
    ///
    /// This is primarily intended for future NDD versions to ensure backwards
    /// compatibility.
    fn test_non_deterministic_data_serde_version_2() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        let runtime_version = (1, 2);

        let bytes = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000000\
            039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d\
            0200\
            43c8bdcb9afebb1d834a7de18cc214a6fd1632d9\
            01000200\
            00",
        )
        .unwrap();

        let mut reader = io::Cursor::new(bytes);
        let ndd = NonDeterministicData::deserialize(&mut reader).unwrap();

        assert_eq!(ndd.version, VERSION_2);
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
        assert_eq!(ndd.runtime_version, runtime_version);
        assert_eq!(ndd.network_upgrade_payload, None);
    }

    #[test]
    /// Attempts to deserialize a raw NDD using version 2, WITH a network
    /// upgrade payload.
    ///
    /// This is primarily intended for future NDD versions to ensure backwards
    /// compatibility.
    fn test_non_deterministic_data_serde_version_2_with_network_upgrade_payload() {
        let bitcoin_block_hash = BlockHash::all_zeros();
        let pk = secp256k1::PublicKey::from_slice(
            hex::decode("039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d")
                .unwrap()
                .as_slice(),
        )
        .unwrap();

        let block_fee_recipient_address =
            Address::parse_checksummed("0x43C8bDCb9AFeBB1D834A7de18CC214a6FD1632d9", None)
                .expect("valid address");

        let runtime_version = (1, 2);

        let bytes = hex::decode(
            "0000000000000000000000000000000000000000000000000000000000000000\
            039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d\
            0200\
            43c8bdcb9afebb1d834a7de18cc214a6fd1632d9\
            01000200\
            01\
            01000500\
            01\
            00",
        )
        .unwrap();

        let mut reader = io::Cursor::new(bytes);
        let ndd = NonDeterministicData::deserialize(&mut reader).unwrap();

        let network_upgrade_payload =
            NetworkUpgradePayload { version: (1, 5), vote: Vote::Nay, is_compliant: false };

        assert_eq!(ndd.version, VERSION_2);
        assert_eq!(ndd.bitcoin_block_hash, bitcoin_block_hash);
        assert_eq!(ndd.aggregated_public_key, pk);
        assert_eq!(ndd.block_fee_recipient_address, block_fee_recipient_address);
        assert_eq!(ndd.runtime_version, runtime_version);
        assert_eq!(ndd.network_upgrade_payload, Some(network_upgrade_payload));
    }
}
