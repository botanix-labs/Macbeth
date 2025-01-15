pub(crate) mod coin_selection;
pub(crate) mod psbt;
pub(crate) mod util;
pub mod address;

use bitcoin::Weight;

/// The weight needed to satisfy a taproot output using keyspend.
pub const TAPROOT_KEYSPEND_SATISFACTION_WEIGHT: Weight = Weight::from_wu(66);

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
}
