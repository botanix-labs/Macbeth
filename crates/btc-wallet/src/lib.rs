extern crate anyhow;
extern crate hex;

pub mod address;
pub mod block_source;
pub mod key;
pub mod transaction;

use bitcoin::Weight;

/// The weight needed to satisfy a taproot output using keyspend.
pub const TAPROOT_KEYSPEND_SATISFACTION_WEIGHT: Weight = Weight::from_wu(71);

#[cfg(test)]
mod test {
    use super::*;

    use bitcoin::secp256k1::{self, rand, KeyPair};
    use miniscript::{self, Descriptor};

    #[test]
    fn taproot_keyspend_satisfaction_weights() {
        let secp = secp256k1::Secp256k1::new();
        let key_pair = KeyPair::new(&secp, &mut rand::thread_rng());

        let desc =
            Descriptor::Tr(miniscript::descriptor::Tr::new(key_pair.public_key(), None).unwrap());
        let weight = Weight::from_wu(desc.max_satisfaction_weight().unwrap() as u64);
        assert_eq!(weight, TAPROOT_KEYSPEND_SATISFACTION_WEIGHT);
    }
}
