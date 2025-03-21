use crate::activation_manager::{
    tests::utils::{Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE},
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests that an upgrade is rejected when most validators vote Nay despite
/// sufficient compliance.
///
/// This test verifies that:
/// 1. All validators are configured to accept an upgrade (is_compliant = true)
/// 2. ALICE votes Aye, but BOB and EVE vote Nay
/// 3. Despite having 100% of validators being compliant with the upgrade, it does not activate
///    because the Aye vote approval_rate (only 34%) is below the required quorum
/// 4. This demonstrates that both acceptance and explicit Aye votes are required for an upgrade to
///    activate
#[test]
fn activation_manager_vote_majority_nay_compliant_and_reject() {
    let upgrade_height = 3;
    let required_approval_rate = 67;

    // NOTE: While the majority of validators are willing to upgrade, the
    // upgrade does not happen since the explicit Aye-votes are are still in the
    // minority.
    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Nay)
        .setup_compliant_validator(EVE, Vote::Nay);

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes Aye.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // All validators do accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
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
    // Block 1: Bob proposes and votes Nay, but is willing to upgrade.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: false, // not finalized yet
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
                compliance_approval_rate: 67,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 2);

    //
    // Block 2: Eve proposes and votes Nay, but is willing to upgrade.
    //

    f.start_proposal(EVE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: true, // IS met!
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
                compliance_approval_rate: 100,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 3);
    assert_eq!(f.next_height(), upgrade_height);

    //
    // Block 3-20: Alice keeps building, but the upgrade never happens.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                compliant_req: true,
                comp_approval_req: true,
                aye_approval_req: false, // IS NOT met!
                block_height_req: true,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 34,
                compliance_approval_rate: 100,
            },
        )
        .build_blocks_until(21);

    assert_eq!(f.next_height(), 21);
}
