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

// The standardness limit for a transaction is 400,000 weight units
pub const MAX_TX_WEIGHT: u64 = 400_000;

// version = 4 bytes * 4 weight units
// marker = 1 byte * 1 weight unit
// flag = 1 byte * 1 weight unit
// ninput = 3 bytes to encode values between 253 to 65,535 * 4 weight units
// noutput = 3 bytes to encode values between 253 to 65,535 * 4 weight units
// locktime = 4 bytes * 4 weight units
pub const MAX_BASE_TX_WEIGHT: u64 = 16 + 1 + 1 + 12 + 12 + 16;

// txid = 32 bytes * 4 weight units
// vout = 4 bytes * 4 weight units
// empty_script_len = 1 byte * 4 weight units
// sequence = 4 bytes * 4 weight units
// witness item count = 1 byte * 1 weight units
// signature = TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT
pub const PER_P2TR_KEYSPEND_WEIGHT: u64 =
    32 * 4 + 4 * 4 + 1 * 4 + 4 * 4 + 1 + TAPROOT_KEYSPEND_SIGHASH_DEFAULT_WEIGHT.to_wu();

// output_value = 8 bytes * 4 weight units
// output_script_len = 1 byte * 4 weight units
// output_script = 34 bytes * 1 weight unit (assuming the longest standard script (p2wsh or p2tr),
// others are shorter))
pub const PER_OUTPUT_MAX_WEIGHT: u64 = 8 * 4 + 1 * 4 + 34 * 4;

// Bitcoin's maximum transaction weight is 400,000 weight units.
// Conservatively setting this to 50,000 to allow for ~100s of pegouts in a single tx
pub const MAX_PEGOUT_TX_WEIGHT: u64 = 50_000;

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
