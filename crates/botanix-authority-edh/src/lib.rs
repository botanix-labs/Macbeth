pub mod extra_data_header;
pub mod header_ext;

/// "nothing up my sleve" NUMS point for the secp256k1 curve.
/// Used as the first aggregate key for the botanix gensis block
/// consensus should check that this key is being used in genesis and post genesis block is not
/// being used
// Pulled from secp256k1 crate `secp256k1::constants::GENERATOR_X`
#[inline]
pub fn nums_secp256k1_pk() -> secp256k1::PublicKey {
    let nums_point = [
        121, 190, 102, 126, 249, 220, 187, 172, 85, 160, 98, 149, 206, 135, 11, 7, 2, 155, 252,
        219, 45, 206, 40, 217, 89, 242, 129, 91, 22, 248, 23, 152,
    ];
    secp256k1::XOnlyPublicKey::from_slice(&nums_point)
        .expect("valid nums point")
        .public_key(secp256k1::Parity::Even)
}
