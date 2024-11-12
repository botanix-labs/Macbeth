use std::io::Write;

use bitcoin::{
    hashes::{sha256, Hash},
    key::TweakedPublicKey,
    Address, Network, ScriptBuf,
};
use secp256k1::{PublicKey, Scalar, Secp256k1, SecretKey, Verification};

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

/// Deprecated
fn generate_tweak<T>(eth_address: &T, aggregate_key: &PublicKey) -> Scalar
where
    T: EthAddress,
{
    let eth = eth_address.as_slice();
    let eth_address_tweak = sha256::Hash::hash(eth);
    {
        let mut eng = sha256::Hash::engine();
        eng.write_all(&aggregate_key.serialize()).unwrap();
        eng.write_all(&eth_address_tweak[..]).unwrap();
        let hash = sha256::Hash::from_engine(eng);
        secp256k1::Scalar::from_be_bytes(hash.to_byte_array())
            .expect("safe hash values should be under the curve order")
    }
}

/// Deprecated
pub fn generate_tweaked_public_key<T>(
    secp: &Secp256k1<impl Verification>,
    eth_address: &T,
    aggregate_key: &PublicKey,
) -> secp256k1::PublicKey
where
    T: EthAddress,
{
    let tweak = generate_tweak(eth_address, aggregate_key);
    aggregate_key
        .add_exp_tweak(secp, &tweak)
        .expect("if you hash the point into the tweak, this can't really happen")
}

/// Deprecated
pub fn generate_tweaked_secret_key<T>(
    eth_address: &T,
    aggregate_key: &PublicKey,
    secret_key: &SecretKey,
) -> SecretKey
where
    T: EthAddress,
{
    let tweak = generate_tweak(eth_address, aggregate_key);
    secret_key.add_tweak(&tweak).expect("legal tweak")
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
