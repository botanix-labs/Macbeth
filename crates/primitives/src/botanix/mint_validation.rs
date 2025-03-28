use std::str::FromStr;

use ethers::abi::decode;

use thiserror::Error;

use crate::{
    botanix::{
        peg_contract::{PeginData, PeginDataError, PeginMeta, PegoutData, PegoutDataError},
        utils::AmountExt,
    },
    keccak256, Address, B256,
};

use tracing::{error, info};

lazy_static::lazy_static! {
    /// hash of the mint topic as it appears in the log
    pub static ref MINT_TOPIC: B256 = keccak256("Mint(address,uint256,uint32,bytes)");
    /// hash of the burn topic as it appears in the log
    pub static ref BURN_TOPIC: B256 = keccak256("Burn(address,uint256,bytes,bytes)");
    /// address of the mint contract
    pub static ref MINT_CONTRACT_ADDRESS: Address = Address::from_str("0x0Ea320990B44236A0cEd0ecC0Fd2b2df33071e78").unwrap();
}

/// Two types of events that can be emitted by the mint contract.
#[derive(Debug)]
pub enum GenesisContractEventType {
    /// Minting event
    MintingEvent,
    /// Burn event
    BurnEvent,
}

impl TryFrom<B256> for GenesisContractEventType {
    type Error = &'static str;
    fn try_from(value: B256) -> Result<Self, Self::Error> {
        if value == *MINT_TOPIC {
            return Ok(Self::MintingEvent);
        } else if value == *BURN_TOPIC {
            return Ok(Self::BurnEvent);
        }
        Err("Invalid topic")
    }
}

/// Error type for parsing mint event
#[derive(Debug, Error)]
pub enum ParseMintEventError {
    /// Error parsing mint event log from minting contract
    #[error("error parsing Mint log from Minting contract")]
    InvalidLog(&'static str),
    /// Invalid pegin metadata
    #[error("invalid pegin metadata")]
    InvalidPeginData {
        /// Error parsing pegin data
        #[source]
        error: PeginDataError,
        /// Address the pegin is sent from
        revert_address: Address,
        /// Amount of the pegin to be reverted
        revert_amount: ethers::types::U256,
    },
}

/// Error type for parsing burn event
#[derive(Debug, Error)]
pub enum ParseBurnEventError {
    /// Error parsing burn event log from minting contract
    #[error("error parsing Burn log from Minting contract")]
    InvalidLog(&'static str),
    /// Invalid pegout metadata
    #[error("invalid pegout metadata")]
    InvalidPegoutData(#[from] PegoutDataError),
}

/// Combined type of [`ParseMintEventError`] and [`ParseBurnEventError`].
#[derive(Debug, Error)]
pub enum MintContractError {
    /// Error parsing event log from Minting contract
    #[error("error parsing event log from Minting contract")]
    InvalidLog {
        /// Event type
        event: &'static str,
        /// Error message
        error: String,
    },
    /// Invalid pegin metadata
    #[error("invalid pegin metadata")]
    InvalidPeginData {
        /// Error message
        error: String,
        /// Address the pegin is sent from
        revert_address: Address,
        /// Amount of the pegin to be reverted
        revert_amount: ethers::types::U256,
    },
    /// Invalid pegout metadata
    #[error("invalid pegout metadata")]
    InvalidPegoutData(#[from] PegoutDataError),
}

impl From<ParseMintEventError> for MintContractError {
    fn from(e: ParseMintEventError) -> Self {
        match e {
            ParseMintEventError::InvalidLog(e) => {
                Self::InvalidLog { event: "Mint", error: e.into() }
            }
            ParseMintEventError::InvalidPeginData { error, revert_address, revert_amount } => {
                Self::InvalidPeginData { error: error.to_string(), revert_address, revert_amount }
            }
        }
    }
}

impl From<ParseBurnEventError> for MintContractError {
    fn from(e: ParseBurnEventError) -> Self {
        match e {
            ParseBurnEventError::InvalidLog(e) => {
                Self::InvalidLog { event: "Burn", error: e.into() }
            }
            ParseBurnEventError::InvalidPegoutData(e) => Self::InvalidPegoutData(e),
        }
    }
}

fn topic_to_address(t: B256) -> Option<Address> {
    // topics are 32 byte values that pad the actual value within,
    // so for addresses we have 12 zero bytes of padding in front
    let tokens = decode(&[ethers::abi::param_type::ParamType::Address], &t.0).ok()?;
    let bytes = tokens.first()?.clone().into_address()?;
    Some(Address::from_slice(bytes.0.as_slice()))
}

/// Parse the given log for a [Mint] event.
///
/// It returns an error if it's a mint event with problems, but
/// returns [Ok(None)] if it's not a [Mint] event.
pub fn try_parse_mint_event(
    log: &revm_primitives::Log,
) -> Result<Option<PeginData>, ParseMintEventError> {
    if log.address != *MINT_CONTRACT_ADDRESS {
        info!("Log address is not mint contract address");
        return Ok(None);
    }

    let topics = log.topics();
    if topics.is_empty() {
        // NB I don't think this is possible but just be safe.
        info!("Log has no topics");
        return Ok(None);
    }
    if topics[0] != *MINT_TOPIC {
        info!("Log topic is not mint topic");
        return Ok(None);
    }

    // So we have a mint event
    if topics.len() != 2 {
        return Err(ParseMintEventError::InvalidLog("wrong number of topics"));
    }

    let destination = topic_to_address(log.topics()[1])
        .ok_or(ParseMintEventError::InvalidLog("invalid destination encoding"))?;

    let params = decode(
        &[
            ethers::abi::param_type::ParamType::Uint(256_usize),
            ethers::abi::param_type::ParamType::Uint(32_usize),
            ethers::abi::param_type::ParamType::Bytes,
        ],
        &log.data.data,
    )
    .map_err(|_| ParseMintEventError::InvalidLog("invalid payload"))?;

    if params.len() != 3 {
        return Err(ParseMintEventError::InvalidLog("wrong number of params"));
    }

    let amount = params[0]
        .clone()
        .into_uint()
        .ok_or(ParseMintEventError::InvalidLog("invalid mint amount params"))?;

    let bitcoin_block_height = params[1]
        .clone()
        .into_uint()
        .ok_or(ParseMintEventError::InvalidLog("parsing bitcoin block height param"))?
        .as_u32();

    let meta_bytes = params[2]
        .clone()
        .into_bytes()
        .ok_or(ParseMintEventError::InvalidLog("converting metadata param to bytes"))?;

    let meta = {
        let mut proofs = Vec::new();
        let mut offset = 0;
        while offset < meta_bytes.len() {
            let (proof, proof_size) =
                PeginMeta::deserialize(&meta_bytes[offset..]).map_err(|e| {
                    let err = ParseMintEventError::InvalidPeginData {
                        error: e,
                        revert_address: destination,
                        revert_amount: amount,
                    };
                    error!("Failed to parse pegin meta: {:?}", err);
                    err
                })?;
            proofs.push(proof);
            offset += proof_size;
        }
        proofs
    };

    Ok(Some(PeginData { account: destination, amount, bitcoin_block_height, meta }))
}

/// Parse the given log for a [Burn] event.
///
/// It returns an error if it's a burn event with problems, but
/// returns [Ok(None)] if it's not a [Burn] event.
pub fn try_parse_burn_event(
    log: &revm_primitives::Log,
    btc_network: bitcoin::Network,
) -> Result<Option<PegoutData>, ParseBurnEventError> {
    if log.address != *MINT_CONTRACT_ADDRESS {
        return Ok(None);
    }

    let topics = log.topics();
    if topics.is_empty() {
        // NB I don't think this is possible but just be safe.
        return Ok(None);
    }
    if topics[0] != *BURN_TOPIC {
        return Ok(None);
    }

    if topics.len() != 2 {
        return Err(ParseBurnEventError::InvalidLog("wrong number of topics"));
    }

    let params = decode(
        &[
            ethers::abi::param_type::ParamType::Uint(256_usize),
            ethers::abi::param_type::ParamType::String,
            ethers::abi::param_type::ParamType::Bytes,
        ],
        &log.data.data,
    )
    .map_err(|_| ParseBurnEventError::InvalidLog("invalid payload"))?;

    if params.len() != 3 {
        return Err(ParseBurnEventError::InvalidLog("wrong number of params"));
    }
    let amount =
        params[0].clone().into_uint().ok_or(ParseBurnEventError::InvalidLog("pegout amount"))?;
    let btc_amount = bitcoin::Amount::from_wei_floor(amount)
        .ok_or(ParseBurnEventError::InvalidLog("invalid amount"))?;

    let destination = params[1]
        .clone()
        .into_string()
        .ok_or(ParseBurnEventError::InvalidLog("pegout destination"))?;

    // should be the pegout version which is a single byte
    let metadata = params[2].clone().into_bytes().ok_or(ParseBurnEventError::InvalidPegoutData(
        PegoutDataError::Invalid("invalid metadata", amount),
    ))?;

    if metadata.len() != 1 {
        return Err(ParseBurnEventError::InvalidPegoutData(PegoutDataError::Invalid(
            "invalid metadata length",
            amount,
        )));
    }

    if metadata[0] != PegoutData::version() {
        info!("unexpected pegout version submitted, version: {}", metadata[0].to_string());
        // Add support for legacy pegout versions
    }

    Ok(Some(
        PegoutData::new(btc_amount, amount, destination, btc_network)
            .map_err(ParseBurnEventError::InvalidPegoutData)?,
    ))
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use revm_primitives::{hex, Bytes, Log, LogData};

    use super::*;

    #[test]
    fn mint_topic() {
        let topic =
            B256::from_str("0x922344dc04648c0ce028ecdf9b2c9eed9a6794dbb47b777b54b0cfe069f128aa")
                .unwrap();
        assert_eq!(topic, *MINT_TOPIC);
    }

    #[test]
    fn burn_topic_sanity_check() {
        let topic =
            B256::from_str("0x17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe1715")
                .unwrap();
        assert_eq!(topic, *BURN_TOPIC);
    }

    #[test]
    fn decode_log_payload() {
        let payload = "000000000000000000000000000000000000000000000000000000000000006400000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000219000000002e5523bcd1b329e8a1a66b7d31719e94a33483eae77f5a677e6634d84ce55f470000000014194f42f33a9b3d5fe9e7ba8501be24d00b07b50376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762010080732aa97865f6b4be36ba861d397401e956d23e129940bb8a03000000000000000000b0d5ec7a0f49793b896db8f4a2cb4ec37e6b2dbd8e90e23d23f860abc9a76b70f1d2ba6494380517307685f91900000005eff8dff8847822b4e01e67fa9677090a2e245da5b9e9cc14252f1ef4f92f4f3e17016fd863ab8fdcc80288d0ae8bf36f723c231f54ce5466f04052efd68b74ed4cdc9c656a3e4fb891fe119493f394890c75b71311fe418d685ddb1af6db28ff672579da4f13ecd7e7b5aa7f2e487156ba1b315369e5324b012387e273484fa149105de78edaea707d3a9a5dba2b5e3ae72413465184871523aad56d2144bf4204001000000100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac0000000000000000000000";
        let bytes = hex::decode(payload).unwrap();

        let decoded_params: Vec<ethers::abi::Token> = decode(
            &[
                ethers::abi::param_type::ParamType::Uint(256_usize),
                ethers::abi::param_type::ParamType::Uint(64_usize),
                ethers::abi::param_type::ParamType::Bytes,
            ],
            bytes.as_slice(),
        )
        .unwrap();

        let amount = decoded_params.first().unwrap().clone().into_uint().unwrap();
        assert_eq!(amount, ethers::types::U256::from_str_radix("100", 10).unwrap());

        let nonce = decoded_params.get(1).unwrap().clone().into_uint().unwrap().as_u64();
        assert_eq!(nonce, 1000u64);

        let meta_bytes = decoded_params.get(2).unwrap().clone().into_bytes().unwrap();
        let meta = PeginMeta::deserialize(meta_bytes.as_slice());
        assert!(meta.is_ok());
    }

    #[test]
    fn decode_address_topic() {
        let topic = "000000000000000000000000a65812bac44dadb79c3e4930dbd98d5a75376b2a";
        let decoded = topic_to_address(B256::from_str(topic).unwrap());

        assert!(decoded.is_some());
        assert_eq!(
            decoded.unwrap(),
            Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap()
        );
    }

    #[test]
    fn decode_burn_log_payload() {
        // create log data from burn event
        // encoded values (amount, destination, version)
        let amount = ethabi::Token::Uint(ethabi::ethereum_types::U256::from(100));
        let destination = ethabi::Token::String("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string());
        let version = ethabi::Token::Bytes(vec![0]);
        let payload = ethabi::encode(&[amount, destination, version]);

        let decoded_params: Vec<ethers::abi::Token> = decode(
            &[
                ethers::abi::param_type::ParamType::Uint(256_usize),
                ethers::abi::param_type::ParamType::String,
                ethers::abi::param_type::ParamType::Bytes,
            ],
            payload.as_slice(),
        )
        .expect("params are decoded");

        let amount = decoded_params
            .first()
            .expect("first param exists")
            .clone()
            .into_uint()
            .expect("valid uint");
        assert_eq!(amount, ethers::types::U256::from_str_radix("100", 10).unwrap());

        let destination = decoded_params
            .get(1)
            .expect("second param exists")
            .clone()
            .into_string()
            .expect("valid string");
        assert_eq!(destination, "mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string());

        let version = decoded_params
            .get(2)
            .expect("third param exists")
            .clone()
            .into_bytes()
            .expect("valid bytes");
        assert_eq!(version, vec![0]);
    }

    #[test]
    fn try_parse_burn_event_should_parse_successfully() {
        // create log generated from burn event
        // encoded values (amount, destination, version)
        let amount =
            ethabi::Token::Uint(ethabi::ethereum_types::U256::from(10_000_000_000_000_u64));
        let destination = ethabi::Token::String("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string());
        let version = ethabi::Token::Bytes(vec![0]);
        let payload = ethabi::encode(&[amount, destination, version]);

        let log = Log {
            address: *MINT_CONTRACT_ADDRESS,
            data: LogData::new(
                vec![
                    *BURN_TOPIC,
                    // msg.sender
                    B256::from(hex!(
                        "000000000000000000000000a65812bac44dadb79c3e4930dbd98d5a75376b2a"
                    )),
                ],
                Bytes::copy_from_slice(payload.as_slice()),
            )
            .expect("log data is created"),
        };

        let result = try_parse_burn_event(&log, bitcoin::Network::Regtest);
        assert!(result.is_ok());

        let pegout_data = result.expect("result is ok").expect("pegout data exists");
        assert_eq!(pegout_data.amount, bitcoin::Amount::from_sat(1000));
        assert_eq!(
            pegout_data.destination,
            bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
                .expect("valid address")
                .assume_checked()
        );
        assert_eq!(pegout_data.network, bitcoin::Network::Regtest);
    }
    
    #[test]
    fn try_parse_burn_event_failure_error_should_contain_amount() {
        // Create a burn event payload with an invalid metadata (empty bytes) to simulate invalid pegout metadata.
        let amount = ethabi::Token::Uint(ethabi::ethereum_types::U256::from(10_000_000_000_000_u64));
        let destination = ethabi::Token::String("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh".to_string());
        let invalid_metadata = ethabi::Token::Bytes(vec![]); // invalid: length != 1
        let payload = ethabi::encode(&[amount, destination, invalid_metadata]);

        let sender = B256::from_str("0x000000000000000000000000a65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap();
        let log = Log { 
            address: *MINT_CONTRACT_ADDRESS,
            data: LogData::new(
                vec![*BURN_TOPIC, sender],
                Bytes::copy_from_slice(payload.as_slice())
            ).expect("LogData creation should succeed")
        };

        let result = try_parse_burn_event(&log, bitcoin::Network::Regtest);
        let expected_amount = ethers::types::U256::from(10_000_000_000_000_u64);
        assert_matches!(result, Err(ParseBurnEventError::InvalidPegoutData(PegoutDataError::Invalid("invalid metadata length", amt))) if amt == expected_amount);
    }
}
