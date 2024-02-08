use std::str::FromStr;

use ethers::abi::decode;
use reth_primitives::{keccak256, Address, B256};
use revm::primitives::Log;
use secp256k1::{self, PublicKey};

use crate::{
    peg_contract::{PeginData, PeginError, PeginMeta, PegoutData, PegoutError},
    utils::AmountExt,
};

lazy_static::lazy_static! {
    pub static ref MINT_TOPIC: B256 = keccak256("Mint(address,uint256,uint32,bytes)");
    pub static ref BURN_TOPIC: B256 = keccak256("Burn(address,uint256,bytes,bytes)");
    pub static ref MINT_CONTRACT_ADDRESS: Address = Address::from_str("0x0Ea320990B44236A0cEd0ecC0Fd2b2df33071e78").unwrap();
    static ref FROST_PUB_KEY: PublicKey = PublicKey::from_str("02d0a67d0b49551c6edfa7f00737b8139a28de6eb7102131c02704f3ad1cf579cd").unwrap();
}

#[derive(Debug)]
pub enum GenesisContractEvents {
    MintingEvent,
    BurnEvent,
}

impl TryFrom<B256> for GenesisContractEvents {
    type Error = &'static str;
    fn try_from(value: B256) -> Result<Self, Self::Error> {
        if value == *MINT_TOPIC {
            return Ok(GenesisContractEvents::MintingEvent);
        } else if value == *BURN_TOPIC {
            return Ok(GenesisContractEvents::BurnEvent);
        }
        Err("Invalid topic")
    }
}

#[derive(Debug)]
pub enum MintConsensusError {
    UnexpectedLog(&'static str),
    InvalidMetadata(PeginError),
    MintContractDidNotEmitRelevantTopic(),
    ValidationFailed(PeginError),
    PegoutValidationFailed(PegoutError),
    RecentBlocksCannotBeEmpty(),
    LogParsingError(&'static str),
    UintParsingError(&'static str),
    FailedToConvertMetadataToBytes(),
    MintContractDidNotEmitMintTopic(),
    PegoutAmountIsInvalid(),
    InvalidPayloadFromLog(),
}

fn topic_to_address(t: B256) -> Result<Address, MintConsensusError> {
    // topics are 32 byte values that padd the actual value within,
    // so for addresses we have 12 zero bytes of padding in front
    let decoded_params: Vec<ethers::abi::Token> =
        decode(&[ethers::abi::param_type::ParamType::Address], &t.0)
            // TODO (armins) this should be a custom error
            .unwrap();

    let word = decoded_params
        .get(0)
        .ok_or(MintConsensusError::LogParsingError("Failed to parse destination address"))?
        .clone()
        .into_address()
        .ok_or(MintConsensusError::LogParsingError("Failed to parse destination address"))?;

    let address_slice = word.0.as_slice();
    let address = Address::from_slice(&address_slice);

    // Convert ethers address to reth address
    Ok(address)
}

pub fn parse_pegin_reth_log_topic(
    log: &reth_primitives::Log,
) -> Result<PeginData, MintConsensusError> {
    let revm_log = log.into();

    parse_pegin_topic(&revm_log)
}

pub fn parse_pegout_reth_log_topic(
    log: &reth_primitives::Log,
) -> Result<PegoutData, MintConsensusError> {
    let revm_log = log.into();

    parse_pegout_topic(&revm_log)
}

pub fn parse_pegin_topic(log: &revm::primitives::Log) -> Result<PeginData, MintConsensusError> {
    if log.address != *MINT_CONTRACT_ADDRESS {
        return Err(MintConsensusError::MintContractDidNotEmitMintTopic());
    }

    for topic in log.topics() {
        if *topic == *MINT_TOPIC {
            if log.topics().len() != 2 {
                return Err(MintConsensusError::UnexpectedLog("wrong number of topics"));
            }

            let destination = topic_to_address(log.topics()[1])?;
            let data = &log.data;

            let decoded_params: Vec<ethers::abi::Token> = decode(
                &[
                    ethers::abi::param_type::ParamType::Uint(256 as usize),
                    ethers::abi::param_type::ParamType::Uint(32 as usize),
                    ethers::abi::param_type::ParamType::Bytes,
                ],
                &data.data,
            )
            .map_err(|_e| MintConsensusError::InvalidPayloadFromLog())?;

            let amount = decoded_params
                .get(0)
                .ok_or(MintConsensusError::LogParsingError("Failed to parse amount"))?
                .clone()
                .into_uint()
                .ok_or(MintConsensusError::UintParsingError("Failed to parse amount"))?;

            let bitcoin_block_height = decoded_params
                .get(1)
                .ok_or(MintConsensusError::LogParsingError("Failed to parse bitcoin block height"))?
                .clone()
                .into_uint()
                .ok_or(MintConsensusError::UintParsingError(
                    "Failed to parse bitcoin block height",
                ))?
                .as_u32();

            let meta_bytes = decoded_params
                .get(2)
                .ok_or(MintConsensusError::LogParsingError("Failed to parse metadata"))?
                .clone()
                .into_bytes()
                .ok_or(MintConsensusError::FailedToConvertMetadataToBytes())?;

            let meta = {
                let mut proofs = Vec::new();
                let mut offset = 0;
                while offset < meta_bytes.len() {
                    let (proof, proof_size) = PeginMeta::deserialize(&meta_bytes[offset..])
                        .map_err(MintConsensusError::InvalidMetadata)?;
                    proofs.push(proof);
                    offset += proof_size;
                }
                proofs
            };

            let pegin = PeginData { account: destination, amount, bitcoin_block_height, meta };
            return Ok(pegin);
        }
    }
    Err(MintConsensusError::MintContractDidNotEmitRelevantTopic())
}

pub fn parse_pegout_topic(log: &revm::primitives::Log) -> Result<PegoutData, MintConsensusError> {
    if log.address != *MINT_CONTRACT_ADDRESS {
        return Err(MintConsensusError::MintContractDidNotEmitMintTopic());
    }

    for topic in log.topics() {
        if *topic == *BURN_TOPIC {
            if log.topics().len() != 2 {
                return Err(MintConsensusError::UnexpectedLog("wrong number of topics"));
            }

            let data = &log.data;

            let decoded_params: Vec<ethers::abi::Token> = decode(
                &[
                    ethers::abi::param_type::ParamType::Uint(256 as usize),
                    ethers::abi::param_type::ParamType::String,
                ],
                &data.data,
            )
            .map_err(|_e| MintConsensusError::InvalidPayloadFromLog())?;

            let amount = decoded_params
                .get(0)
                .ok_or(MintConsensusError::LogParsingError("Failed to parse pegout amount"))?
                .clone()
                .into_uint()
                .ok_or(MintConsensusError::UintParsingError("Failed to pegout amount"))?;

            let destination = decoded_params
                .get(1)
                .ok_or(MintConsensusError::LogParsingError("Failed to parse pegout destination"))?
                .clone()
                .into_string()
                .ok_or(MintConsensusError::UintParsingError("Failed to parse pegout destination"))?
                .as_str()
                .to_string();

            let btc_amount = bitcoin::Amount::from_wei_floor(amount)
                .ok_or(MintConsensusError::PegoutAmountIsInvalid())?;

            let pegout = PegoutData::new(btc_amount, destination)
                .map_err(|e| MintConsensusError::PegoutValidationFailed(e))?;

            return Ok(pegout);
        }
    }

    Err(MintConsensusError::MintContractDidNotEmitRelevantTopic())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn mint_topic() {
        let topic =
            B256::from_str("0x9de7365c663dc09a824437fcfe283fde0349736c62570a07a36e47f9a5dcaf0f")
                .unwrap();
        assert!(topic == *MINT_TOPIC);
    }

    #[test]
    fn burn_topic_sanity_check() {
        let topic =
            B256::from_str("0x17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe1715")
                .unwrap();
        assert!(topic == *BURN_TOPIC);
    }

    #[test]
    fn decode_log_payload() {
        let payload = "000000000000000000000000000000000000000000000000000000000000006400000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000219000000002e5523bcd1b329e8a1a66b7d31719e94a33483eae77f5a677e6634d84ce55f470000000014194f42f33a9b3d5fe9e7ba8501be24d00b07b50376698beebe8ee5c74d8cc50ab84ac301ee8f10af6f28d0ffd6adf4d6d3b9b762010080732aa97865f6b4be36ba861d397401e956d23e129940bb8a03000000000000000000b0d5ec7a0f49793b896db8f4a2cb4ec37e6b2dbd8e90e23d23f860abc9a76b70f1d2ba6494380517307685f91900000005eff8dff8847822b4e01e67fa9677090a2e245da5b9e9cc14252f1ef4f92f4f3e17016fd863ab8fdcc80288d0ae8bf36f723c231f54ce5466f04052efd68b74ed4cdc9c656a3e4fb891fe119493f394890c75b71311fe418d685ddb1af6db28ff672579da4f13ecd7e7b5aa7f2e487156ba1b315369e5324b012387e273484fa149105de78edaea707d3a9a5dba2b5e3ae72413465184871523aad56d2144bf4204001000000100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac0000000000000000000000";
        let bytes = hex::decode(payload).unwrap();

        let decoded_params: Vec<ethers::abi::Token> = decode(
            &[
                ethers::abi::param_type::ParamType::Uint(256 as usize),
                ethers::abi::param_type::ParamType::Uint(64 as usize),
                ethers::abi::param_type::ParamType::Bytes,
            ],
            &bytes.as_slice(),
        )
        .unwrap();

        let amount = decoded_params.get(0).unwrap().clone().into_uint().unwrap();
        assert_eq!(amount, ethers::types::U256::from_str_radix("100", 10).unwrap());

        let nonce = decoded_params.get(1).unwrap().clone().into_uint().unwrap().as_u64();
        assert_eq!(nonce, 1000u64);

        let meta_bytes = decoded_params.get(2).unwrap().clone().into_bytes().unwrap();
        let meta = PeginMeta::deserialize(&meta_bytes.as_slice());
        assert!(meta.is_ok());
    }

    #[test]
    fn decode_address_topic() {
        let topic = "000000000000000000000000a65812bac44dadb79c3e4930dbd98d5a75376b2a";
        let decoded = topic_to_address(B256::from_str(topic).unwrap());

        assert!(decoded.is_ok());
        assert_eq!(
            decoded.unwrap(),
            Address::from_str("0xa65812bac44dadb79c3e4930dbd98d5a75376b2a").unwrap()
        );
    }
}
