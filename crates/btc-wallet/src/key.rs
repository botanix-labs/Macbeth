use secp256k1::{
    hashes::{sha256, Hash},
    rand::rngs::OsRng,
    scalar::OutOfRangeError,
    KeyPair, PublicKey, Scalar, SecretKey,
};

use frost_secp256k1_tr as frost;

lazy_static::lazy_static! {
    static ref SECP: secp256k1::Secp256k1<secp256k1::All> = secp256k1::Secp256k1::new();
}

#[derive(Debug)]
pub enum KeyError {
    OutOfRange,
    SecpError,
}

impl From<OutOfRangeError> for KeyError {
    fn from(_err: OutOfRangeError) -> Self {
        KeyError::OutOfRange
    }
}

impl From<secp256k1::Error> for KeyError {
    fn from(_err: secp256k1::Error) -> Self {
        KeyError::SecpError
    }
}

pub fn generate_new_secret_key() -> SecretKey {
    let (secret_key, _) = SECP.generate_keypair(&mut OsRng);
    secret_key
}

pub fn generate_bip340_keypair() -> KeyPair {
    KeyPair::new(&SECP, &mut OsRng)
}

fn generate_tweak_scalar(tweak: &[u8; 32], pk: &PublicKey) -> Result<Scalar, KeyError> {
    let pk_bytes = pk.serialize();
    let bytes_to_hash = {
        let mut buf = Vec::with_capacity(pk_bytes.len() + tweak.len());
        buf.extend(pk_bytes);
        buf.extend(tweak);
        buf
    };

    let hash = sha256::Hash::hash(bytes_to_hash.as_slice());
    let scalar = Scalar::from_be_bytes(hash.to_byte_array())?;

    Ok(scalar)
}

// Deprecated
pub fn tweak_private_key(tweak: &[u8; 32], prv: &SecretKey) -> Result<SecretKey, KeyError> {
    let scalar = generate_tweak_scalar(tweak, &prv.public_key(&SECP))?;
    let tweaked_prv = prv.add_tweak(&scalar)?;

    Ok(tweaked_prv)
}

// Deprecated
pub fn tweak_public_key(
    tweak: &[u8; 32],
    pk: secp256k1::PublicKey,
) -> Result<secp256k1::PublicKey, KeyError> {
    let scalar = generate_tweak_scalar(tweak, &pk)?;
    let tweaked_pk = pk.add_exp_tweak(&SECP, &scalar)?;

    Ok(tweaked_pk)
}

pub fn tweak_frost_verifying_key(
    pk: &secp256k1::PublicKey,
    tweak: &[u8; 20],
) -> Result<secp256k1::PublicKey, KeyError> {
    let pk_slice: [u8; 33] = pk.serialize().try_into().unwrap();
    let vk = frost::VerifyingKey::deserialize(pk_slice).unwrap().get_tweaked(Some(tweak));

    let tweaked_pk = secp256k1::PublicKey::from_slice(&vk.serialize()).unwrap();
    Ok(tweaked_pk)
}

#[cfg(test)]
mod tests {
    use secp256k1::{Message, Secp256k1};

    use super::*;
    const ETH_ADDRESS: [u8; 32] = [
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde,
        0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
        0xde, 0xf0,
    ];
    #[test]
    fn is_should_tweak_pk() {
        let eth_tweak = [0u8; 20];
        let kp = generate_bip340_keypair();
        let pk = kp.public_key();
        let tpk = tweak_frost_verifying_key(&pk, &eth_tweak).expect("valid tweak");

        assert_ne!(pk, tpk);
    }

    #[test]
    fn it_should_create_key_of_correct_length() {
        let secret_key = generate_new_secret_key();
        assert!(secret_key[..].len() == 32);
    }

    #[test]
    fn verify_signed_message_with_tweaked_key() {
        let secp = Secp256k1::new();
        let key_pair = generate_bip340_keypair();
        let pk = key_pair.public_key();

        let message = Message::from_hashed_data::<sha256::Hash>("foobar".as_bytes());
        let tweaked_pk = tweak_public_key(&ETH_ADDRESS, pk).unwrap();
        let tweaked_prv = tweak_private_key(&ETH_ADDRESS, &key_pair.secret_key()).unwrap();

        let sig = secp.sign_schnorr(&message, &tweaked_prv.keypair(&secp));

        secp.verify_schnorr(&sig, &message, &tweaked_pk.x_only_public_key().0).unwrap();
    }
}
