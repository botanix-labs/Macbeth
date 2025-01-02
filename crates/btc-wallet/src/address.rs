use bitcoin::{key::TweakedPublicKey, Address, Network, ScriptBuf};
use secp256k1::PublicKey;

pub trait EthAddress {
    fn as_slice(&self) -> &[u8];
}

impl EthAddress for ethers::types::Address {
    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl EthAddress for Vec<u8> {
    fn as_slice(&self) -> &[u8] {
        self
    }
}

/// Generate a taproot address from a given tweaked public key
/// Note this includes both the eth address tweak and the taproot merkel root tweak
pub fn generate_taproot_address(tweaked_public_key: &PublicKey, network: Network) -> Address {
    let tweaked_pk =
        TweakedPublicKey::dangerous_assume_tweaked(tweaked_public_key.x_only_public_key().0);
    let p2tr_script = bitcoin::ScriptBuf::new_p2tr_tweaked(tweaked_pk);
    Address::from_script(&p2tr_script, network).expect("valid address")
}

pub fn generate_taproot_change_scriptpubkey(public_key: &PublicKey) -> ScriptBuf {
    // This is commented out for now b/c the frost library only supports empty merkel root
    // let taproot_spend_info =
    //     generate_taproot_spend_info(secp, public_key).expect("Valid spend info");

    bitcoin::ScriptBuf::new_p2tr(
        bitcoin::secp256k1::SECP256K1,
        public_key.x_only_public_key().0,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use secp256k1::{rand::rngs::OsRng, Keypair};
    fn generate_key_pair() -> Keypair {
        let (secret_key, _) = secp256k1::SECP256K1.generate_keypair(&mut OsRng);
        let keypair = Keypair::from_secret_key(secp256k1::SECP256K1, &secret_key);

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
