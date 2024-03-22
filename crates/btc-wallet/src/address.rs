use std::io::Write;

use bitcoin::{
    absolute::LockTime,
    hashes::{sha256, Hash},
    key::TweakedPublicKey,
    opcodes,
    script::Builder,
    taproot::{TaprootBuilder, TaprootError, TaprootSpendInfo},
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

// Unused, spend safe path only requires a single signer for now
// Keep commented out till we use a multisig
// const SAFE_SPEND_PATH_QUORUM: i64 = 3;
const SAFE_SPEND_TIMELOCK_SECOND: u32 = 1653195600;

lazy_static::lazy_static! {
    /// Compressed 33 byte public key of the recovery signer
    /// After SAFE_SPEND_TIMELOCK_SECOND blocks this recovery signer can recover funds for the user
    /// Keep in mind this is temporary solution for the POC testnet.alloc
    /// Note this key is not derived from proper entropy nor is it dervied from a BIP-32 path
    /// Mainnet funds should not be spendable via this path
    static ref RECOVERY_PK: PublicKey = PublicKey::from_slice(
        hex::decode("02e2af4a49570e224fdddc6443863281ff9d96e6311547943a7628ed925e767a7a")
            .expect("decode hex")
            .as_slice(),
    ).expect("Public key conversion");
}

#[derive(Debug)]
enum SafeSpendPathError {
    #[allow(dead_code)]
    InvalidLengthOfPublicKeys,
    #[allow(dead_code)]
    QuorumCannotBeLessThanPublicKeys,
}

/// TODO function desc
/// Timelocks are relative
fn _build_safe_spend_path_script_check_sig_add(
    lock_time: LockTime,
    public_keys: &Vec<PublicKey>,
    quorum: i64,
) -> Result<ScriptBuf, SafeSpendPathError> {
    if public_keys.len() < 2 {
        return Err(SafeSpendPathError::InvalidLengthOfPublicKeys);
    }

    if public_keys.len() > usize::try_from(quorum).expect("Quorum should always be a valid usize") {
        return Err(SafeSpendPathError::QuorumCannotBeLessThanPublicKeys);
    }

    let mut script = Builder::new()
        .push_lock_time(lock_time)
        .push_key(&bitcoin::PublicKey::new(
            *public_keys.first().expect("There is always a 0th public key"),
        ))
        .push_opcode(opcodes::all::OP_CHECKSIG);

    for i in 1..public_keys.len() {
        script = script
            .push_key(&bitcoin::PublicKey::new(
                *(public_keys.get(i).unwrap_or_else(|| panic!("should find pubkey at {}", i))),
            ))
            .push_opcode(opcodes::all::OP_CHECKSIGADD);
    }

    script = script.push_int(quorum);
    script = script.push_opcode(opcodes::all::OP_EQUALVERIFY);
    Ok(script.into_script())
}

fn build_safe_spend_path_script_check_sig_verify(
    lock_time: LockTime,
    public_key: PublicKey,
) -> Result<ScriptBuf, SafeSpendPathError> {
    let script = Builder::new()
        .push_lock_time(lock_time)
        .push_key(&bitcoin::PublicKey::new(public_key))
        .push_opcode(opcodes::all::OP_CHECKSIGVERIFY);

    Ok(script.into_script())
}

pub fn generate_taproot_spend_info(
    secp: &Secp256k1<impl Verification>,
    tweaked_public_key: &PublicKey,
) -> Result<TaprootSpendInfo, TaprootError> {
    let lock_time = LockTime::from_time(SAFE_SPEND_TIMELOCK_SECOND).expect("valid time");
    let builder = TaprootBuilder::new()
        .add_leaf(
            0u8,
            build_safe_spend_path_script_check_sig_verify(lock_time, *RECOVERY_PK).unwrap(),
        )
        .expect("Couldn't add timelock leaf");

    let finalized_taproot =
        builder.finalize(secp, tweaked_public_key.x_only_public_key().0).unwrap();

    Ok(finalized_taproot)
}

pub fn generate_taproot_address(tweaked_public_key: &PublicKey, network: Network) -> Address {
    let script = generate_taproot_scriptpubkey(tweaked_public_key);
    Address::from_script(&script, network).expect("valid address")
}

/// Deprecated
fn generate_tweak<T>(eth_address: &T, aggregate_key: &PublicKey) -> Scalar
where
    T: EthAddress,
{
    let eth = eth_address.as_slice();
    let eth_address_tweak = sha256::Hash::hash(eth);
    let tweak = {
        let mut eng = sha256::Hash::engine();
        eng.write_all(&aggregate_key.serialize()).unwrap();
        eng.write_all(&eth_address_tweak[..]).unwrap();
        let hash = sha256::Hash::from_engine(eng);
        secp256k1::Scalar::from_be_bytes(hash.to_byte_array())
            .expect("safe hash values should be under the curve order")
    };

    tweak
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

pub fn generate_taproot_scriptpubkey(public_key: &secp256k1::PublicKey) -> ScriptBuf {
    // This is commented out for now b/c the frost library only supports empty merkel root
    // let taproot_spend_info =
    //     generate_taproot_spend_info(secp, public_key).expect("Valid spend info");

    // Note that the public key is already tweaked with the eth address and the taptree merkel root
    // so we can use the dangerous_assume_tweaked method to create the script
    // In the case of a change output being created no eth address tweak is provided
    let tweaked_pk = TweakedPublicKey::dangerous_assume_tweaked(public_key.x_only_public_key().0);
    bitcoin::ScriptBuf::new_v1_p2tr_tweaked(tweaked_pk)
}

pub fn generate_taproot_change_scriptpubkey(
    secp: &Secp256k1<impl Verification>,
    public_key: &PublicKey,
) -> ScriptBuf {
    // This is commented out for now b/c the frost library only supports empty merkel root
    // let taproot_spend_info =
    //     generate_taproot_spend_info(secp, public_key).expect("Valid spend info");

    bitcoin::ScriptBuf::new_v1_p2tr(&secp, public_key.x_only_public_key().0, None)
}

/// Note: pk provided to this address is the frost public key already tweaked
/// with the eth address and the taptree merkel root.
pub fn gateway_address(pk: &PublicKey, network: Network) -> anyhow::Result<Address> {
    Ok(generate_taproot_address(&pk, network))
}

#[cfg(test)]
mod tests {
    lazy_static::lazy_static! {
        static ref SECP: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
    }

    use super::*;
    use crate::key::generate_bip340_keypair;
    use hex;
    use secp256k1::KeyPair;

    #[test]
    fn correct_eth_address() {
        let network: Network = Network::Testnet;
        let key_pair = KeyPair::from_seckey_str(
            &SECP,
            "fe66aac784520af747e36ef4cd99320f2d5003ba05aafd05feea115ae79c9b65",
        )
        .unwrap();

        let gateway = gateway_address(&key_pair.public_key(), network).unwrap();
        assert_eq!(
            gateway.to_string(),
            "tb1ptn6uxgn7euat3hkqdxx2x0h9ynhvzkjkwnee2smlzx6fvnyn2ejqjyman0"
        );
    }

    #[test]
    fn it_should_produce_a_testnet_taproot_address() {
        let network: Network = Network::Testnet;
        let key_pair = generate_bip340_keypair();
        // Here we use a untweaked key, but that is fine, generate address doesn't know any better
        let address = generate_taproot_address(&key_pair.public_key(), network);
        assert!(address.to_string().starts_with("tb1p"));
        assert!(Address::is_spend_standard(&address));
    }

    #[test]
    fn it_should_produce_a_mainnet_taproot_address() {
        let network = Network::Bitcoin;
        let key_pair = generate_bip340_keypair();
        // Here we use a untweaked key, but that is fine, generate address doesn't know any better
        let address = generate_taproot_address(&key_pair.public_key(), network);

        assert!(address.to_string().starts_with("bc1p"));
        assert!(Address::is_spend_standard(&address));
    }

    #[test]
    fn it_should_produce_34_byte_script_pubkey() {
        let network = Network::Bitcoin;
        let key_pair = generate_bip340_keypair();
        // Here we use a untweaked key, but that is fine, generate address doesn't know any better
        let address = generate_taproot_address(&key_pair.public_key(), network);

        assert_eq!(address.script_pubkey().len(), 34);
    }
}
