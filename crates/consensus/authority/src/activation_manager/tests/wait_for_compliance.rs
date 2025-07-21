use crate::activation_manager::{
    tests::utils::{
        Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE, UPGRADE_VERSION,
    },
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests the upgrade flow where validators first signal support, then later accept.
///
/// This test verifies that:
/// 1. All validators initially signal Aye votes but are not ready to accept the upgrade
/// 2. Despite unanimous Aye votes, the upgrade doesn't activate because no validators are compliant
///    (is_compliant = false)
/// 3. After all validators update to accept the upgrade and additional voting occurs:
///    - By block 5, all conditions are met (100% Aye votes, 100% acceptance, target height reached)
///    - EVE proposes the first upgraded block which is accepted by all
/// 4. This demonstrates the two-phase upgrade process where validators can signal support before
///    they're technically ready to handle the upgrade
#[test]
fn activation_manager_wait_for_compliance() {
    let upgrade_height = 3;
    let required_approval_rate = 67;

    // NOTE: All validators signal their support for the upgrade, but none are
    // ready to handle/accept it yet.
    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_signaling_validator(ALICE, Vote::Aye)
        .setup_signaling_validator(BOB, Vote::Aye)
        .setup_signaling_validator(EVE, Vote::Aye);

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes Aye.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // None of the validators are ready to accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: false,
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
                comp_approval_rate: 0,
                aye_votes: 1,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0, // Not compliant!
                total_votes: 1,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 1);

    //
    // Block 1: Bob proposes and votes Aye.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: false,
                // NOTE: signaling validators simply use `u64::MAX` as the
                // approval_rate.
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
                aye_approval_rate: 67,
                comp_approval_rate: 0,
                aye_votes: 2,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0,
                total_votes: 2,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 2);

    //
    // Block 2: Eve proposes and votes Aye.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: false,
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
                aye_approval_rate: 100,
                comp_approval_rate: 0,
                aye_votes: 3,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0,
                total_votes: 3,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 3);
    assert_eq!(f.next_height(), upgrade_height);

    //
    // All validators update their software and are now ready to ACCEPT the upgrade.
    //

    let mut f = f
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye)
        .setup_compliant_validator(EVE, Vote::Aye);

    //
    // Block 3: Alice proposes and votes Aye.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false,
                aye_approval_req: true, // IS met!
                block_height_req: true, // IS met!
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 100,
                comp_approval_rate: 34,
                aye_votes: 3,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 1, // Compliance count starts!
                total_votes: 3,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 4);

    //
    // Block 4: Bob proposes and votes Aye.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false, // not finalized yet
                aye_approval_req: true,
                block_height_req: true,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 100,
                comp_approval_rate: 67,
                aye_votes: 3,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 2,
                total_votes: 3,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 5);

    //
    // Block 5: Eve proposes the UPGRADE, all conditions are met!
    //

    f.start_proposal(EVE, UPGRADE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: true, // IS met!
                aye_approval_req: true,  // IS met!
                block_height_req: true,
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
                aye_votes: 0,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0,
                total_votes: 0,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 6);

    //
    // Block 6-20: Alice keeps building upgraded bocks, all validators accept.
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        // No upgrade ongoing.
        .upgrade_conditions_empty(&[ALICE, BOB, EVE])
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                comp_approval_rate: 0,
                aye_votes: 0,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0,
                total_votes: 0,
            },
        )
        .build_blocks_until(21);

    assert_eq!(f.next_height(), 21);
}
