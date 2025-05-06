pub mod address;
pub(crate) mod coin_selection;
pub mod psbt;
pub mod util;

use bitcoin::Weight;

/// The weight needed to satisfy a Segwit WPKH output.
pub const SEGWIT_KEYSPEND_SATISFACTION_WEIGHT: Weight = Weight::from_wu(107);

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
            Descriptor::Wpkh(miniscript::descriptor::Wpkh::new(key_pair.public_key()).unwrap());
        let weight = desc.max_weight_to_satisfy().unwrap();
        assert_eq!(weight, SEGWIT_KEYSPEND_SATISFACTION_WEIGHT);
    }
}
