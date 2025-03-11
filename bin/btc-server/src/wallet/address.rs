use std::str::FromStr;

use bitcoin::{
    key::TweakedPublicKey,
    opcodes::all::{OP_CHECKSIG, OP_CHECKSIGADD, OP_EQUAL},
    secp256k1::PublicKey,
    taproot::TaprootSpendInfo,
    Address, Network, ScriptBuf, TapNodeHash,
};
use bitcoin_hashes::Hash;
use frost_secp256k1_tr::{self as frost, keys::Tweak, SigningParameters};
use miniscript::ToPublicKey;

use crate::wallet::util::{VerifyingKeyExt, VerifyingKeyExtError};

// TODO should be iter of something that can be sorted and into'd into a publickey
pub fn generate_ssp_script(pks: Vec<bitcoin::PublicKey>) -> ScriptBuf {
    assert_eq!(pks.len(), 3);
    let threshold = 2;
    // Lets sort the pks
    let mut pks = pks;
    pks.sort_by(|a, b| a.cmp(b));

    let script = ScriptBuf::builder()
        .push_key(&pks[0])
        .push_opcode(OP_CHECKSIG)
        .push_key(&pks[1])
        .push_opcode(OP_CHECKSIGADD)
        .push_key(&pks[2])
        .push_opcode(OP_CHECKSIGADD)
        .push_int(threshold)
        .push_opcode(OP_EQUAL)
        .into_script();

    script
}

// TODO fill this out, comment listed pubkeys
// Or this lives somehwere else maybe a config.yaml
pub(crate) const SSP_PKS: &str = "";

pub trait EthAddress {
    fn as_slice(&self) -> &[u8];
}

impl EthAddress for Vec<u8> {
    fn as_slice(&self) -> &[u8] {
        self
    }
}

pub fn generate_taproot_spend_info(
    pks: Vec<bitcoin::PublicKey>,
    agg_pk: bitcoin::PublicKey,
) -> TaprootSpendInfo {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let script = generate_ssp_script(pks);
    let builder = bitcoin::taproot::TaprootBuilder::new()
        .add_leaf(0u8, script.clone())
        .expect("Couldn't add ssp leaf");

    let taproot_spend_info =
        builder.finalize(&secp, agg_pk.to_x_only_pubkey()).expect("Couldn't finalize taproot");

    taproot_spend_info
}

pub(crate) fn taproot_merkle_root(agg_pk: bitcoin::PublicKey) -> TapNodeHash {
    assert!(!SSP_PKS.is_empty());
    let pks: Vec<bitcoin::PublicKey> = SSP_PKS
        .split(",")
        .map(|s| bitcoin::PublicKey::from_str(s).expect("Valid public key"))
        .collect();
    let taproot_spend_info = generate_taproot_spend_info(pks, agg_pk);
    let merkle_root = taproot_spend_info.merkle_root().expect("Couldn't get merkle root");

    merkle_root
}

pub fn generate_tweaked_public_key(
    verifying_key: &frost::VerifyingKey,
    eth_address: &[u8; 20],
) -> Result<PublicKey, VerifyingKeyExtError> {
    let pk = bitcoin::PublicKey::new(verifying_key.to_secp_pk()?);
    let merkle_root = taproot_merkle_root(pk);
    let signing_parameters = SigningParameters {
        tapscript_merkle_root: Some(merkle_root.to_byte_array().to_vec()),
        additional_tweak: Some(eth_address.as_slice().to_vec()),
    };
    let tweaked_pk = verifying_key.tweak(&signing_parameters).to_secp_pk()?;
    Ok(tweaked_pk)
}

pub fn generate_taproot_scriptpubkey(tweaked_public_key: &PublicKey) -> ScriptBuf {
    let tap_tweaked_key =
        TweakedPublicKey::dangerous_assume_tweaked(tweaked_public_key.x_only_public_key().0);
    bitcoin::ScriptBuf::new_p2tr_tweaked(tap_tweaked_key)
}

/// Generate a taproot address from a given tweaked public key
/// Note this includes both the eth address tweak and the taproot merkel root tweak
pub fn generate_taproot_address(tweaked_public_key: &PublicKey, network: Network) -> Address {
    let p2tr_script = generate_taproot_scriptpubkey(tweaked_public_key);
    Address::from_script(&p2tr_script, network).expect("valid address")
}

pub fn generate_taproot_change_scriptpubkey(public_key: &PublicKey) -> ScriptBuf {
    // TODO: secp context should be a global variable or passed down
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let public_key = bitcoin::PublicKey::new(*public_key);
    bitcoin::ScriptBuf::new_p2tr(
        &secp,
        public_key.to_x_only_pubkey(),
        Some(taproot_merkle_root(public_key)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::secp256k1::{rand::rngs::OsRng, Keypair};
    fn generate_key_pair() -> Keypair {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let (secret_key, _) = secp.generate_keypair(&mut OsRng);
        let keypair = Keypair::from_secret_key(&secp, &secret_key);

        keypair
    }

    #[test]
    fn it_should_produce_a_testnet_taproot_address() {
        let network: Network = Network::Testnet;
        let key_pair = generate_key_pair();
        // Here we use a untweaked key, but that is fine, generate address doesn't know any better
        let address = generate_taproot_address(&key_pair.public_key(), network);
        assert!(address.to_string().starts_with("tb1p"));
        assert!(Address::is_spend_standard(&address));
    }

    #[test]
    fn it_should_produce_a_mainnet_taproot_address() {
        let network = Network::Bitcoin;
        let key_pair = generate_key_pair();
        // Here we use a untweaked key, but that is fine, generate address doesn't know any better
        let address = generate_taproot_address(&key_pair.public_key(), network);

        assert!(address.to_string().starts_with("bc1p"));
        assert!(Address::is_spend_standard(&address));
    }

    #[test]
    fn it_should_produce_34_byte_script_pubkey() {
        let network = Network::Bitcoin;
        let key_pair = generate_key_pair();
        // Here we use a untweaked key, but that is fine, generate address doesn't know any better
        let address = generate_taproot_address(&key_pair.public_key(), network);

        assert_eq!(address.script_pubkey().len(), 34);
    }
}
