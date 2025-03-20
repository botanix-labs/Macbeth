use crate::activation_manager::{
    tests::utils::{Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE},
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests that validators tracking different upgrade versions don't count each
/// other's votes.
///
/// This test verifies that:
/// 1. Two validators (ALICE, BOB) vote for and track upgrade version 2.0
/// 2. A third validator (EVE) votes for a different upgrade version (3.0)
/// 3. The validators only count votes for their specific tracked version
/// 4. Even though all validators are voting "Aye", the upgrade doesn't activate because from each
///    perspective, the minimum quorum requirement is not met
/// 5. This protects the network from confusion when multiple potential upgrades are being discussed
#[test]
fn activation_manager_reject_mismatched_vote() {
    let upgrade_height = 3;
    // NOTE: All validators are required to reach the approval rate!
    let required_approval_rate = 100;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye)
        // NOTE: Eve is using the same active version as Alice and Bob, but
        // tracks a different upgrade; Alice and Bob vote for version 2.0, while
        // Eve votes for 3.0.
        .setup_INVALID_compliant_validator(EVE, Vote::Aye);

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // All validators are do accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false,
                aye_approvals_req: false,
                block_height_req: false,
            },
        )
        // Alice and Bob count Alices' vote.
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 34,
                compliance_approval_rate: 34,
            },
        )
        // Eve does not count Alices' vote (different version).
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
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false,
                aye_approvals_req: false,
                block_height_req: false,
            },
        )
        // Alice and Bob count each others votes.
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 67,
                compliance_approval_rate: 67,
            },
        )
        // Eve does not count Bobs' vote (different version).
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
    // Block 2: Eve proposes and votes.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false,
                aye_approvals_req: false,
                block_height_req: false,
            },
        )
        // Alice and Bob DO NOT count Eves' vote (different version).
        .expectations(
            &[ALICE, BOB],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 67,
                compliance_approval_rate: 67,
            },
        )
        // Eve counts his own vote
        .expectations(
            &[EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 34,
                compliance_approval_rate: 34,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 3);
    assert_eq!(f.next_height(), upgrade_height);

    //
    // Block 3-20: Alice keeps building, but the upgrade never happens
    // (100% quorum requirement).
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false,
                aye_approvals_req: false,
                block_height_req: true, // IS met!
            },
        )
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
                aye_approval_rate: 34,
                compliance_approval_rate: 34,
            },
        )
        .build_blocks_until(21);

    assert_eq!(f.next_height(), 21);
}
