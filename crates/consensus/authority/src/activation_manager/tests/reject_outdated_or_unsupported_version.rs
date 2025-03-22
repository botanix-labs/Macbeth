use crate::activation_manager::{
    tests::utils::{
        Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE, UPGRADE_VERSION,
    },
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests that validators reject blocks with outdated or unsupported versions.
///
/// This test verifies that:
/// 1. All validators successfully activate an upgrade to version 2.0
/// 2. EVE is then reset to use the previous version (1.0)
/// 3. When EVE proposes blocks with the outdated version:
///    - ALICE and BOB reject these blocks during process_proposal but accept them during
///      finalize_block
///    - This behavior allows historical sync while preventing outdated block production
/// 4. When ALICE proposes blocks with the upgraded version:
///    - EVE rejects these blocks during both process_proposal and finalize_block
///    - This creates a consensus split where EVE cannot follow the upgraded chain
#[test]
fn activation_manager_reject_outdated_or_unsupported_version() {
    let upgrade_height = 3;
    let required_approval_rate = 67;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye)
        .setup_compliant_validator(EVE, Vote::Aye);

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // All validators do accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false,
                aye_approval_req: false,
                block_height_req: false,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 34,
                comp_approval_rate: 34,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 1);

    //
    // Block 1: Bob proposes and votes.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false, // not finalized yet
                aye_approval_req: false,  // not finalized yet
                block_height_req: false,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 67,
                comp_approval_rate: 67,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 2);

    //
    // Block 2: Eve proposes and votes.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: true, // IS met!
                aye_approval_req: true,  // IS met!
                block_height_req: false,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 100,
                comp_approval_rate: 100,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 3);
    assert_eq!(f.next_height(), upgrade_height);

    //
    // Block 3: Alice proposes the UPGRADE, all conditions are met!
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: true, // IS met!
                aye_approval_req: true,  // IS met!
                block_height_req: true,  // IS met!
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                // Votes pruned after upgrade
                aye_approval_rate: 0,
                comp_approval_rate: 0,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 4);

    //
    // RESET Eve with the previous, outdated version. Do note that Alice and Bob
    // do still accept the outdated blocks during finalization, but they do not
    // back outdated blocks.
    //
    assert!(ACTIVE_VERSION < UPGRADE_VERSION);
    let mut f = f.setup_ignoring_validator(EVE);

    //
    // Block 4-20: Eve keeps building OUTDATED bocks, Alice and Bob keep
    // finalizing.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        // No upgrade ongoing.
        .upgrade_conditions_empty(&[ALICE, BOB, EVE])
        // Alice and Bob REJECT the outdated block during the backing phase, but
        // will accept it during the finalization phase.
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: false, // Reject!
                finalize_pass: true, // Accept
                aye_approval_rate: 0,
                comp_approval_rate: 0,
            },
        )
        // Eve (producer) accepts the outdated block.
        .expectations(
            &[EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                comp_approval_rate: 0,
            },
        )
        .build_blocks_until(21);

    assert_eq!(f.next_height(), 21);

    //
    // Block 21-40: Alice keeps building UPGRADED blocks, Eve keeps rejecting.
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        .upgrade_conditions_empty(&[ALICE, BOB, EVE])
        // Alice and Bob accept the upgraded blocks.
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                comp_approval_rate: 0,
            },
        )
        // Eve REJECTS the upgraded blocks, both during the backing and
        // finalization phase. Note that Eve is not configured to accept the
        // upgrade.
        .expectations(
            &[EVE],
            Expectations {
                process_pass: false,  // Reject!
                finalize_pass: false, // Reject!
                aye_approval_rate: 0,
                comp_approval_rate: 0,
            },
        )
        .build_blocks_until(41);

    assert_eq!(f.next_height(), 41);
}
