use frost_secp256k1_tr as frost;

#[derive(Debug)]
pub enum KeyError {
    OutOfRange,
    SecpError,
}

impl From<secp256k1::Error> for KeyError {
    fn from(_err: secp256k1::Error) -> Self {
        KeyError::SecpError
    }
}

/// Generate a tweaked public key from a given public key and tweak.
pub fn tweak_frost_verifying_key(
    pk: &secp256k1::PublicKey,
    tweak: &[u8; 20],
) -> Result<secp256k1::PublicKey, KeyError> {
    let pk_slice: [u8; 33] = pk.serialize();
    let vk = frost::VerifyingKey::deserialize(pk_slice).unwrap().get_tweaked(Some(tweak));

    let tweaked_pk = secp256k1::PublicKey::from_slice(&vk.serialize()).unwrap();
    Ok(tweaked_pk)
}
