use bitcoin::{key::TweakedPublicKey, secp256k1::PublicKey, Address, Network, ScriptBuf};
use frost_secp256k1_tr::{self as frost, keys::Tweak, SigningParameters};

use crate::wallet::util::{VerifyingKeyExt, VerifyingKeyExtError};

pub trait EthAddress {
    fn as_slice(&self) -> &[u8];
}

impl EthAddress for Vec<u8> {
    fn as_slice(&self) -> &[u8] {
        self
    }
}

pub fn generate_tweaked_public_key(
    verifying_key: &frost::VerifyingKey,
    eth_address: &[u8; 20],
) -> Result<PublicKey, VerifyingKeyExtError> {
    let signing_parameters = SigningParameters {
        tapscript_merkle_root: None,
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
    // This is commented out for now b/c the frost library only supports empty merkel root
    // let taproot_spend_info =
    //     generate_taproot_spend_info(secp, public_key).expect("Valid spend info");

    // TODO: secp context should be a global variable or passed down
    let secp = bitcoin::secp256k1::Secp256k1::new();

    bitcoin::ScriptBuf::new_p2tr(&secp, public_key.x_only_public_key().0, None)
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
