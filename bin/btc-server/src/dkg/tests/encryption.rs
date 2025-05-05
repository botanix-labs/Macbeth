use crate::dkg::encryption::{DkgHandshakeManager, Error};
use bitcoin::secp256k1;
use frost::keys::dkg::{round1, round2};
use frost_secp256k1_tr as frost;
use std::collections::BTreeMap;

const SESSION_ID: &[u8] = b"test_session_id";

const ROUND1_DKG: &[u8] = &[
    0, 35, 15, 138, 179, 2, 2, 120, 88, 85, 71, 235, 157, 87, 39, 38, 125, 191, 226, 130, 130, 109,
    33, 101, 203, 186, 92, 8, 192, 49, 14, 162, 200, 99, 210, 81, 193, 116, 35, 3, 3, 106, 54, 33,
    158, 157, 204, 101, 31, 134, 240, 213, 83, 120, 7, 193, 132, 135, 1, 209, 27, 29, 108, 85, 16,
    2, 41, 11, 129, 48, 199, 108, 64, 82, 233, 151, 145, 38, 39, 23, 230, 84, 196, 216, 128, 145,
    22, 182, 69, 191, 243, 11, 111, 220, 94, 34, 101, 66, 1, 34, 206, 187, 151, 84, 248, 127, 11,
    173, 110, 104, 72, 32, 73, 170, 148, 211, 170, 108, 244, 232, 37, 117, 104, 172, 111, 16, 249,
    70, 33, 22, 18, 156, 178, 255, 134, 99, 134,
];

const ROUND2_DKG: &[u8] = &[
    0, 35, 15, 138, 179, 40, 187, 129, 5, 45, 179, 241, 143, 13, 134, 171, 27, 4, 5, 204, 10, 175,
    209, 21, 55, 121, 141, 147, 7, 163, 101, 73, 65, 34, 191, 210, 117,
];

const ROUND3_DKG: &[u8] = &[
    0, 35, 15, 138, 179, 2, 52, 39, 160, 118, 176, 185, 222, 25, 54, 213, 2, 171, 233, 2, 75, 75,
    154, 59, 199, 10, 16, 208, 24, 249, 238, 56, 171, 146, 37, 245, 114, 93, 3, 108, 137, 0, 253,
    154, 146, 199, 112, 182, 12, 44, 60, 29, 106, 29, 65, 98, 57, 254, 90, 246, 117, 140, 167, 102,
    167, 190, 31, 63, 156, 97, 50, 172, 197, 159, 249, 40, 76, 205, 49, 208, 14, 123, 169, 145,
    252, 96, 128, 142, 96, 26, 2, 128, 79, 6, 59, 90, 29, 133, 161, 26, 217, 244, 230, 2, 53, 176,
    41, 176, 195, 54, 36, 48, 175, 186, 41, 235, 166, 101, 112, 138, 136, 202, 139, 71, 6, 214,
    117, 180, 231, 56, 198, 84, 253, 75, 98, 30, 3, 251, 161, 29, 35, 218, 147, 21, 208, 170, 36,
    42, 217, 240, 26, 207, 45, 101, 228, 130, 212, 56, 168, 46, 66, 162, 96, 179, 91, 235, 140,
    237, 190,
];

const ROUND1_DKG_OTHER: &[u8] = &[
    0, 35, 15, 138, 179, 2, 2, 241, 0, 5, 99, 19, 3, 65, 116, 28, 137, 104, 66, 18, 136, 196, 87,
    23, 4, 170, 132, 129, 12, 15, 77, 228, 92, 60, 55, 177, 30, 224, 20, 2, 21, 190, 88, 2, 105,
    239, 203, 36, 135, 4, 145, 177, 90, 189, 216, 103, 13, 21, 167, 173, 4, 162, 95, 32, 170, 127,
    252, 86, 206, 102, 29, 221, 64, 190, 4, 108, 204, 161, 170, 31, 62, 245, 58, 165, 25, 62, 126,
    65, 119, 239, 227, 103, 185, 208, 48, 169, 212, 161, 99, 37, 51, 53, 130, 144, 184, 163, 164,
    249, 10, 64, 63, 28, 126, 242, 38, 158, 186, 210, 149, 111, 15, 105, 45, 78, 144, 91, 186, 182,
    154, 249, 211, 39, 141, 129, 75, 33, 199,
];

/*
TODO: Additional test:
* Different session IDs
* Incremental `.commit_*` calls
* Incremental `.validate_*` calls
*/

fn setup() -> (DkgHandshakeManager, DkgHandshakeManager, DkgHandshakeManager) {
    let secp = secp256k1::Secp256k1::new();

    let alice_sec = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let alice_pub = secp256k1::PublicKey::from_secret_key(&secp, &alice_sec);

    let bob_sec = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let bob_pub = secp256k1::PublicKey::from_secret_key(&secp, &bob_sec);

    let eve_sec = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let eve_pub = secp256k1::PublicKey::from_secret_key(&secp, &eve_sec);

    let alice_addr = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();

    let mut fed_members = BTreeMap::new();
    fed_members.insert(alice_addr, alice_pub);
    fed_members.insert(bob_addr, bob_pub);
    fed_members.insert(eve_addr, eve_pub);

    // Setup encryption layer for round one.
    let alice =
        DkgHandshakeManager::new(SESSION_ID, alice_addr, alice_sec, fed_members.clone()).unwrap();
    let bob = DkgHandshakeManager::new(SESSION_ID, bob_addr, bob_sec, fed_members.clone()).unwrap();
    let eve = DkgHandshakeManager::new(SESSION_ID, eve_addr, eve_sec, fed_members.clone()).unwrap();

    (alice, bob, eve)
}

#[test]
fn encryption_complete_all_rounds() {
    let alice_addr = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();

    let (mut alice, mut bob, mut eve) = setup();

    // NOTE: We use the same packages for all three members; we're just testing
    // the encryption layer and don't bother finalizing the actual DKG process.

    {
        let round1_dkg = round1::Package::deserialize(ROUND1_DKG).unwrap();

        let (alice_eph, alice_sig) = alice.commit_round1(&round1_dkg).unwrap();
        let (bob_eph, bob_sig) = bob.commit_round1(&round1_dkg).unwrap();
        let (eve_eph, eve_sig) = eve.commit_round1(&round1_dkg).unwrap();

        alice.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
        alice.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        bob.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        bob.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        eve.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        eve.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
    }

    // Transition to round two.
    let mut alice = alice.finalize().unwrap();
    let mut bob = bob.finalize().unwrap();
    let mut eve = eve.finalize().unwrap();

    {
        let round2_dkg = round2::Package::deserialize(ROUND2_DKG).unwrap();

        let alice_to_bob = alice.commit_round2(&bob_addr.into(), &round2_dkg).unwrap();
        let alice_to_eve = alice.commit_round2(&eve_addr.into(), &round2_dkg).unwrap();

        let bob_to_alice = bob.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        let bob_to_eve = bob.commit_round2(&eve_addr.into(), &round2_dkg).unwrap();

        let eve_to_alice = eve.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        let eve_to_bob = eve.commit_round2(&bob_addr.into(), &round2_dkg).unwrap();

        let res1 = alice.validate_round2(bob_addr.into(), bob_to_alice.1, &bob_to_alice.0).unwrap();
        let res2 = alice.validate_round2(eve_addr.into(), eve_to_alice.1, &eve_to_alice.0).unwrap();

        let res3 = bob.validate_round2(alice_addr.into(), alice_to_bob.1, &alice_to_bob.0).unwrap();
        let res4 = bob.validate_round2(eve_addr.into(), eve_to_bob.1, &eve_to_bob.0).unwrap();

        let res5 = eve.validate_round2(alice_addr.into(), alice_to_eve.1, &alice_to_eve.0).unwrap();
        let res6 = eve.validate_round2(bob_addr.into(), bob_to_eve.1, &bob_to_eve.0).unwrap();

        assert_eq!(res1, round2_dkg);
        assert_eq!(res2, round2_dkg);
        assert_eq!(res3, round2_dkg);
        assert_eq!(res4, round2_dkg);
        assert_eq!(res5, round2_dkg);
        assert_eq!(res6, round2_dkg);
    }

    // Transition to round three.
    let mut alice = alice.finalize().unwrap();
    let mut bob = bob.finalize().unwrap();
    let mut eve = eve.finalize().unwrap();

    {
        let round3_dkg = frost::keys::PublicKeyPackage::deserialize(ROUND3_DKG).unwrap();

        let alice_sig = alice.commit_round3(&round3_dkg).unwrap();
        let bob_sig = bob.commit_round3(&round3_dkg).unwrap();
        let eve_sig = eve.commit_round3(&round3_dkg).unwrap();

        alice.validate_round3(bob_addr.into(), bob_sig).unwrap();
        alice.validate_round3(eve_addr.into(), eve_sig).unwrap();

        bob.validate_round3(alice_addr.into(), alice_sig).unwrap();
        bob.validate_round3(eve_addr.into(), eve_sig).unwrap();

        eve.validate_round3(alice_addr.into(), alice_sig).unwrap();
        eve.validate_round3(bob_addr.into(), bob_sig).unwrap();
    }

    // Finalize the DKG process, resulting in the final commit which is equal
    // for all members.
    let alice_final_commit = alice.finalize().unwrap();
    let bob_final_commit = bob.finalize().unwrap();
    let eve_final_commit = eve.finalize().unwrap();

    assert_eq!(alice_final_commit, bob_final_commit);
    assert_eq!(alice_final_commit, eve_final_commit);
    assert_eq!(bob_final_commit, eve_final_commit);
}

#[test]
fn encryption_validate_round1_properties() {
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();
    //
    let invalid_addr = frost::Identifier::derive(100u16.to_le_bytes().as_slice()).unwrap();

    let (mut alice, mut bob, mut eve) = setup();

    {
        let round1_dkg = round1::Package::deserialize(ROUND1_DKG).unwrap();
        let round1_dkg_other = round1::Package::deserialize(ROUND1_DKG_OTHER).unwrap();

        let (bob_eph, bob_sig) = bob.commit_round1(&round1_dkg).unwrap();
        let (eve_eph, eve_sig) = eve.commit_round1(&round1_dkg).unwrap();

        // VALIDATE: None-member address!
        let res = alice.validate_round1(invalid_addr.into(), bob_eph, bob_sig, &round1_dkg);
        assert_eq!(res.unwrap_err(), Error::NotAFedMember);

        // VALIDATE: Wrong address!
        let res = alice.validate_round1(eve_addr.into(), bob_eph, bob_sig, &round1_dkg);
        assert_eq!(res.unwrap_err(), Error::SignatureVerificationFailed);

        // VALIDATE: Wrong ephemeral key!
        let res = alice.validate_round1(bob_addr.into(), eve_eph, bob_sig, &round1_dkg);
        assert_eq!(res.unwrap_err(), Error::SignatureVerificationFailed);

        // VALIDATE: Wrong signature!
        let res = alice.validate_round1(bob_addr.into(), bob_eph, eve_sig, &round1_dkg);
        assert_eq!(res.unwrap_err(), Error::SignatureVerificationFailed);

        // VALIDATE: Wrong round1 package!
        let res = alice.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg_other);
        assert_eq!(res.unwrap_err(), Error::SignatureVerificationFailed);

        // VALIDATE: Original package is valid.
        let res = alice.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg);
        assert!(res.is_ok());
    }

    // VALIDATE: Insufficient number of packages processed.
    let res = alice.finalize();
    assert_eq!(res.unwrap_err(), Error::InsufficientSamples);
}

#[test]
fn encryption_validate_round2_properties() {
    let alice_addr = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();
    //
    let invalid_addr = frost::Identifier::derive(100u16.to_le_bytes().as_slice()).unwrap();

    let (mut alice, mut bob, mut eve) = setup();

    // NOTE: We use the same packages for all three members; we're just testing
    // the encryption layer and don't bother finalizing the actual DKG process.

    {
        let round1_dkg = round1::Package::deserialize(ROUND1_DKG).unwrap();

        let (alice_eph, alice_sig) = alice.commit_round1(&round1_dkg).unwrap();
        let (bob_eph, bob_sig) = bob.commit_round1(&round1_dkg).unwrap();
        let (eve_eph, eve_sig) = eve.commit_round1(&round1_dkg).unwrap();

        alice.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
        alice.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        bob.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        bob.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        eve.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        eve.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
    }

    // Transition to round two.
    let mut alice = alice.finalize().unwrap();
    let mut bob = bob.finalize().unwrap();
    let mut eve = eve.finalize().unwrap();

    {
        let round2_dkg = round2::Package::deserialize(ROUND2_DKG).unwrap();

        let bob_to_alice = bob.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        let eve_to_alice = eve.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();

        assert_eq!(bob_to_alice.1, 0);
        assert_eq!(eve_to_alice.1, 0);

        // VALIDATE: None-member address!
        let res = alice.validate_round2(invalid_addr.into(), bob_to_alice.1, &bob_to_alice.0);
        assert_eq!(res.unwrap_err(), Error::NotAFedMember);

        // VALIDATE: Wrong address!
        let res = alice.validate_round2(eve_addr.into(), bob_to_alice.1, &bob_to_alice.0);
        assert_eq!(res.unwrap_err(), Error::DecryptionFailed);

        // VALIDATE: Wrong nonce!
        let res = alice.validate_round2(bob_addr.into(), 1, &bob_to_alice.0);
        assert_eq!(res.unwrap_err(), Error::DecryptionFailed);

        // VALIDATE: Wrong package!
        let res = alice.validate_round2(bob_addr.into(), eve_to_alice.1, &eve_to_alice.0);
        assert_eq!(res.unwrap_err(), Error::DecryptionFailed);

        // VALIDATE: Original package is valid.
        let res = alice.validate_round2(bob_addr.into(), bob_to_alice.1, &bob_to_alice.0);
        assert!(res.is_ok());
    }

    // VALIDATE: Insufficient number of packages processed.
    let res = alice.finalize();
    assert_eq!(res.unwrap_err(), Error::InsufficientSamples);
}

#[test]
fn encryption_validate_round2_nonce_increments() {
    let alice_addr = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();

    let (mut alice, mut bob, mut eve) = setup();

    // NOTE: We use the same packages for all three members; we're just testing
    // the encryption layer and don't bother finalizing the actual DKG process.

    {
        let round1_dkg = round1::Package::deserialize(ROUND1_DKG).unwrap();

        let (alice_eph, alice_sig) = alice.commit_round1(&round1_dkg).unwrap();
        let (bob_eph, bob_sig) = bob.commit_round1(&round1_dkg).unwrap();
        let (eve_eph, eve_sig) = eve.commit_round1(&round1_dkg).unwrap();

        alice.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
        alice.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        bob.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        bob.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        eve.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        eve.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
    }

    // Transition to round two.
    let _alice = alice.finalize().unwrap();
    let mut bob = bob.finalize().unwrap();
    let _eve = eve.finalize().unwrap();

    {
        let round2_dkg = round2::Package::deserialize(ROUND2_DKG).unwrap();

        let (_, nonce) = bob.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        assert_eq!(nonce, 0);

        let (_, nonce) = bob.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        assert_eq!(nonce, 1);

        let (_, nonce) = bob.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        assert_eq!(nonce, 2);

        // Different nonce for different member.
        let (_, nonce) = bob.commit_round2(&eve_addr.into(), &round2_dkg).unwrap();
        assert_eq!(nonce, 0);
    }
}

#[test]
fn encryption_validate_round3_properties() {
    let alice_addr = frost::Identifier::derive(0u16.to_le_bytes().as_slice()).unwrap();
    let bob_addr = frost::Identifier::derive(1u16.to_le_bytes().as_slice()).unwrap();
    let eve_addr = frost::Identifier::derive(2u16.to_le_bytes().as_slice()).unwrap();
    //
    let invalid_addr = frost::Identifier::derive(100u16.to_le_bytes().as_slice()).unwrap();

    let (mut alice, mut bob, mut eve) = setup();

    // NOTE: We use the same packages for all three members; we're just testing
    // the encryption layer and don't bother finalizing the actual DKG process.

    {
        let round1_dkg = round1::Package::deserialize(ROUND1_DKG).unwrap();

        let (alice_eph, alice_sig) = alice.commit_round1(&round1_dkg).unwrap();
        let (bob_eph, bob_sig) = bob.commit_round1(&round1_dkg).unwrap();
        let (eve_eph, eve_sig) = eve.commit_round1(&round1_dkg).unwrap();

        alice.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
        alice.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        bob.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        bob.validate_round1(eve_addr.into(), eve_eph, eve_sig, &round1_dkg).unwrap();

        eve.validate_round1(alice_addr.into(), alice_eph, alice_sig, &round1_dkg).unwrap();
        eve.validate_round1(bob_addr.into(), bob_eph, bob_sig, &round1_dkg).unwrap();
    }

    // Transition to round two.
    let mut alice = alice.finalize().unwrap();
    let mut bob = bob.finalize().unwrap();
    let mut eve = eve.finalize().unwrap();

    {
        let round2_dkg = round2::Package::deserialize(ROUND2_DKG).unwrap();

        let alice_to_bob = alice.commit_round2(&bob_addr.into(), &round2_dkg).unwrap();
        let alice_to_eve = alice.commit_round2(&eve_addr.into(), &round2_dkg).unwrap();

        let bob_to_alice = bob.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        let bob_to_eve = bob.commit_round2(&eve_addr.into(), &round2_dkg).unwrap();

        let eve_to_alice = eve.commit_round2(&alice_addr.into(), &round2_dkg).unwrap();
        let eve_to_bob = eve.commit_round2(&bob_addr.into(), &round2_dkg).unwrap();

        let res1 = alice.validate_round2(bob_addr.into(), bob_to_alice.1, &bob_to_alice.0).unwrap();
        let res2 = alice.validate_round2(eve_addr.into(), eve_to_alice.1, &eve_to_alice.0).unwrap();

        let res3 = bob.validate_round2(alice_addr.into(), alice_to_bob.1, &alice_to_bob.0).unwrap();
        let res4 = bob.validate_round2(eve_addr.into(), eve_to_bob.1, &eve_to_bob.0).unwrap();

        let res5 = eve.validate_round2(alice_addr.into(), alice_to_eve.1, &alice_to_eve.0).unwrap();
        let res6 = eve.validate_round2(bob_addr.into(), bob_to_eve.1, &bob_to_eve.0).unwrap();

        assert_eq!(res1, round2_dkg);
        assert_eq!(res2, round2_dkg);
        assert_eq!(res3, round2_dkg);
        assert_eq!(res4, round2_dkg);
        assert_eq!(res5, round2_dkg);
        assert_eq!(res6, round2_dkg);
    }

    // Transition to round three.
    let mut alice = alice.finalize().unwrap();
    let mut bob = bob.finalize().unwrap();
    let mut eve = eve.finalize().unwrap();

    {
        let round3_dkg = frost::keys::PublicKeyPackage::deserialize(ROUND3_DKG).unwrap();

        let bob_sig = bob.commit_round3(&round3_dkg).unwrap();
        let eve_sig = eve.commit_round3(&round3_dkg).unwrap();

        // VALIDATE: Awaiting challenge generation
        let res = alice.validate_round3(bob_addr.into(), bob_sig);
        assert_eq!(res.unwrap_err(), Error::AwaitingChallengeGeneration);

        // By committing the round3 package, we generate the challenge bytes and
        // can hence validate incoming signatures.
        let _alice_sig = alice.commit_round3(&round3_dkg).unwrap();

        // VALIDATE: None-member address!
        let res = alice.validate_round3(invalid_addr.into(), bob_sig);
        assert_eq!(res.unwrap_err(), Error::NotAFedMember);

        // VALIDATE: Wrong address!
        let res = alice.validate_round3(eve_addr.into(), bob_sig);
        assert_eq!(res.unwrap_err(), Error::SignatureVerificationFailed);

        // VALIDATE: Wrong signature!
        let res = alice.validate_round3(bob_addr.into(), eve_sig);
        assert_eq!(res.unwrap_err(), Error::SignatureVerificationFailed);

        // VALIDATE: Original package is valid.
        let res = alice.validate_round3(bob_addr.into(), bob_sig);
        assert!(res.is_ok());
    }

    // VALIDATE: Insufficient number of packages processed.
    let res = alice.finalize();
    assert_eq!(res.unwrap_err(), Error::InsufficientSamples);
}
