use bitcoin::{key::TweakedPublicKey, ScriptBuf};
use ethabi::ethereum_types::U256;
use frost_secp256k1_tr::{self as frost, keys::Tweak, SigningParameters};
use reth_btc_wallet::psbt::EthAddress;

/// One satoshi expressed in wei.
///
/// This equals 10^10.
const SATOSHI_IN_WEI: U256 = U256([10_000_000_000, 0, 0, 0]);

/// The maximum [`bitcoin::Amount`] satoshi value.
///
/// This equals `u64::max_value()`.
const MAX_SATOSHI: U256 = U256([u64::MAX, 0, 0, 0]);

/// An extension trait for [`bitcoin::Amount`].
pub trait AmountExt: Copy + From<bitcoin::Amount> + Into<bitcoin::Amount> {
    /// Convert this amount to the representation in wei.
    fn to_wei(self) -> U256 {
        U256::from(self.into().to_sat()) * SATOSHI_IN_WEI
    }

    /// Convert the amount represented in wei into an [`bitcoin::Amount`], by
    /// dropping the value that is smaller than one satoshi (rounding down).
    ///
    /// Returns [None] if the wei amount exceeds the maximum value of
    /// [`bitcoin::Amount::max_value()`].
    fn from_wei_floor(wei: U256) -> Option<Self> {
        let sat = wei / SATOSHI_IN_WEI;
        if sat <= MAX_SATOSHI {
            Some(bitcoin::Amount::from_sat(sat.low_u64()).into())
        } else {
            None
        }
    }

    /// Convert the amount represented in wei into an [`bitcoin::Amount`].
    ///
    /// Returns [None] if the wei amount exceeds the maximum value of
    /// [`bitcoin::Amount::max_value()`] or if the given wei amount is not an
    /// exact multiple of one satoshi.
    fn from_wei(wei: U256) -> Option<Self> {
        let ret = Self::from_wei_floor(wei)?;
        if ret.to_wei() == wei {
            Some(ret)
        } else {
            None
        }
    }
}
impl AmountExt for bitcoin::Amount {}

/// Error type for key operations.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The key is out of range.
    #[error("The key is out of range")]
    OutOfRange,
    /// The key is invalid.
    #[error("The key is invalid: {0}")]
    SecpError(#[from] secp256k1::Error),
    /// Frost error
    #[error("Frost error: {0}")]
    FrostError(#[from] frost::Error),
}

// TODO write tests for this
/// Generate a tweaked public key from a given public key and tweak.
pub fn tweak_frost_verifying_key(
    pk: &secp256k1::PublicKey,
    tweak: &EthAddress,
) -> Result<secp256k1::PublicKey, KeyError> {
    let signing_parameters =
        SigningParameters { tapscript_merkle_root: None, additional_tweak: Some(tweak.to_vec()) };
    let pk_slice: [u8; 33] = pk.serialize();
    let vk = frost::VerifyingKey::deserialize(&pk_slice).map_err(KeyError::from)?;
    let tweaked_vk = vk.tweak(&signing_parameters);
    let tweaked_pk = secp256k1::PublicKey::from_slice(&tweaked_vk.serialize()?)?;

    Ok(tweaked_pk)
}

/// Generate a taproot scriptpubkey from a given tweaked public key
/// This includes both the eth address tweak and taproot merkel tweak
pub fn generate_taproot_scriptpubkey(public_key: &secp256k1::PublicKey) -> ScriptBuf {
    // This is commented out for now b/c the frost library only supports empty merkel root
    // let taproot_spend_info =
    //     generate_taproot_spend_info(secp, public_key).expect("Valid spend info");

    // Note that the public key is already tweaked with the eth address and the taptree merkel root
    // so we can use the dangerous_assume_tweaked method to create the script
    // In the case of a change output being created no eth address tweak is provided
    let xonly =
        bitcoin::XOnlyPublicKey::from_slice(&public_key.x_only_public_key().0.serialize()).unwrap();
    let tweaked_pk = TweakedPublicKey::dangerous_assume_tweaked(xonly);
    bitcoin::ScriptBuf::new_p2tr_tweaked(tweaked_pk)
}

#[cfg(test)]
mod test {
    use super::*;

    use bitcoin::Amount;

    #[test]
    fn test_satoshi_in_wei() {
        assert_eq!(SATOSHI_IN_WEI, U256::from(10_i64.pow(10)));
        assert_eq!(Amount::MAX.to_wei(), MAX_SATOSHI * SATOSHI_IN_WEI);
    }

    #[test]
    fn test_amount_wei_conversion() {
        let max = Amount::MAX;
        assert_eq!(max, Amount::from_wei(max.to_wei()).unwrap());
        assert!(Amount::from_wei_floor(max.to_wei() + SATOSHI_IN_WEI).is_none());

        let some_wei = Amount::from_sat(350).to_wei();
        assert_eq!(Amount::from_wei(some_wei).unwrap(), Amount::from_sat(350));
        assert!(Amount::from_wei(some_wei + 1).is_none());
        assert_eq!(Amount::from_wei_floor(some_wei + 1).unwrap(), Amount::from_sat(350));
    }
}
