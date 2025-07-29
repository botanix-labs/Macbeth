pub mod address;
pub(crate) mod coin_selection;
pub mod psbt;
pub mod util;

use bitcoin::Weight;

/// The weight needed to satisfy a taproot output using keyspend.
pub const TAPROOT_KEYSPEND_SATISFACTION_WEIGHT: Weight = Weight::from_wu(66);
pub const TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT: Weight = Weight::from_wu(65);

// Two base weights for segwit transactions
pub const SEGWIT_FLAG_WEIGHT: Weight = Weight::from_wu(1);
pub const SEGWIT_MARKER_WEIGHT: Weight = Weight::from_wu(1);

#[cfg(test)]
mod test {
    use super::*;

    use bitcoin::secp256k1::{self, rand, Keypair};
    use miniscript::{self, Descriptor};

    #[test]
    fn taproot_keyspend_satisfaction_weights() {
        let secp = secp256k1::Secp256k1::new();
        let key_pair = Keypair::new(&secp, &mut rand::thread_rng());

        let desc =
            Descriptor::Tr(miniscript::descriptor::Tr::new(key_pair.public_key(), None).unwrap());
        let weight = desc.max_weight_to_satisfy().unwrap();
        assert_eq!(weight, TAPROOT_KEYSPEND_SATISFACTION_WEIGHT);
    }

    #[test]
    fn taproot_keyspend_sighash_default_weights() {
        // Sighash default is 1 less weight unit as the sighash type is omitted from the signature
        assert_eq!(
            TAPROOT_KEYSPEND_SATISFACTION_WEIGHT - Weight::from_wu(1),
            TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT
        );
    }
}
