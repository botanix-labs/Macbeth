use crate::activation_manager::{
    tests::utils::{
        Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE, UPGRADE_VERSION,
    },
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests that a validator can vote Nay and reject an upgrade.
///
/// This test verifies that:
/// 1. Two validators (ALICE, BOB) vote Aye and are willing to accept an upgrade
/// 2. One validator (EVE) votes Nay and is configured to reject the upgrade (is_compliant = false)
/// 3. When conditions are met, ALICE proposes upgraded blocks
/// 4. EVE consistently rejects these upgraded blocks in both process_proposal and finalize_block
///    phases
/// 5. This leads to a consensus split where ALICE and BOB operate on the upgraded chain while EVE
///    rejects those blocks as dead ends
#[test]
fn activation_manager_vote_nay_and_reject() {
    let upgrade_height = 3;
    let required_approval_rate = 67;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye)
        // NOTE: Eve votes Nay, and rejects the upgrade if all conditions are met.
        .setup_signaling_validator(EVE, Vote::Nay);

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes Aye.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // Alice and Bob do accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false,
                aye_approval_req: false,
                block_height_req: false,
            },
        )
        // Eve DOES NOT accept the upgrade.
        .upgrade_conditions(
            &[EVE],
            ConditionList {
                compliant_req: false, // IS NOT met!
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
                compliance_approval_rate: 34,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 1);

    //
    // Block 1: Bob proposes and votes Aye.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false, // not finalized yet
                aye_approval_req: false,  // not finalized yet
                block_height_req: false,
            },
        )
        // Eve DOES NOT accept the upgrade.
        .upgrade_conditions(
            &[EVE],
            ConditionList {
                compliant_req: false,
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
                compliance_approval_rate: 67,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 2);

    //
    // Block 2: Eve proposes and votes NAY, and rejects the upgrade.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                compliant_req: true,
                comp_approval_req: true, // IS met!
                aye_approval_req: true,  // IS met!
                block_height_req: false,
            },
        )
        .upgrade_conditions(
            &[EVE],
            ConditionList {
                compliant_req: false,
                // NOTE: rejecting validators simply use `u64::MAX` as the
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
                // NOTE: Eve: votes Nay and rejects.
                aye_approval_rate: 67,
                compliance_approval_rate: 67,
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
                aye_approval_req: true,  // IS met!
                block_height_req: true,  // IS met!
            },
        )
        // Eve DOES NOT accept the upgrade.
        //
        // NOTE: rejecting validators simply use `u64::MAX` as the minimum
        // validator requirement and target height.
        .upgrade_conditions(
            &[EVE],
            ConditionList {
                compliant_req: false,
                comp_approval_req: false, // IS NOT met!
                aye_approval_req: false,  // IS NOT met!
                block_height_req: false,  // IS NOT met!
            },
        )
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
                // Votes pruned after upgrade
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
        .upgrade_conditions_empty(&[ALICE, BOB, EVE])
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
