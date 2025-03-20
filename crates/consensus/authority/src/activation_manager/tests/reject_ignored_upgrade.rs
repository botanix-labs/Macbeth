use crate::activation_manager::{
    tests::utils::{
        Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE, UPGRADE_VERSION,
    },
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests that a validator can reject an upgrade that it ignores.
///
/// This test verifies that:
/// 1. Two validators (ALICE, BOB) vote for and are compliant with the upgrade
/// 2. A third validator (EVE) ignores the upgrade entirely
/// 3. When conditions are met (2 validators required, height reached), ALICE proposes upgraded
///    blocks
/// 4. EVE consistently rejects these upgraded blocks in both the process_proposal and
///    finalize_block phases
/// 5. This leads to a consensus split where ALICE and BOB operate on the upgraded chain while EVE
///    rejects those blocks as dead ends
#[test]
fn activation_manager_reject_ignored_upgrade() {
    let upgrade_height = 3;
    let required_approval_rate = 67;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye)
        // NOTE: Eve ignores the upgrade.
        .setup_ignoring_validator(EVE);

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // Alice and Bob do accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false,
                aye_approvals_req: false,
                block_height_req: false,
            },
        )
        // Eve DOES NOT accept the upgrade.
        .upgrade_conditions_empty(&[EVE])
        // Alice and Bob do participate in voting.
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 34,
                compliance_approval_rate: 34,
            },
        )
        // Eve DOES NOT participate in voting.
        .expectations(
            &[EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 1);

    //
    // Block 1: Bob proposes and votes.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false, // not finalized yet
                aye_approvals_req: false, // not finalized yet
                block_height_req: false,
            },
        )
        .upgrade_conditions_empty(&[EVE])
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 67,
                compliance_approval_rate: 67,
            },
        )
        .expectations(
            &[EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 2);

    //
    // Block 2: Eve proposes, but DOES NOT participate in voting.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: true, // IS met!
                aye_approvals_req: true, // IS met!
                block_height_req: false,
            },
        )
        .upgrade_conditions_empty(&[EVE])
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 67,
                compliance_approval_rate: 67,
            },
        )
        .expectations(
            &[EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
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
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: true, // IS met!
                aye_approvals_req: true, // IS met!
                block_height_req: true,  // IS met!
            },
        )
        .upgrade_conditions_empty(&[EVE])
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                // Votes pruned after upgrade
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
            },
        )
        // Eve REJECTS the upgrade.
        .expectations(
            &[EVE],
            Expectations {
                process_pass: false,  // Reject!
                finalize_pass: false, // Reject!
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 4);

    //
    // Block 4-20: Alice keeps building upgraded bocks, Eve keeps rejecting.
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        // No upgrade ongoing.
        .upgrade_conditions_empty(&[ALICE, BOB])
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
            },
        )
        // Eve REJECTS the upgrade.
        .expectations(
            &[EVE],
            Expectations {
                process_pass: false,  // Reject!
                finalize_pass: false, // Reject!
                aye_approval_rate: 0,
                compliance_approval_rate: 0,
            },
        )
        .build_blocks_until(21);

    assert_eq!(f.next_height(), 21);
}
