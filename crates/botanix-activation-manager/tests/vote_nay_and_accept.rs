use botanix_activation_manager::{
    test_utils::{
        Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE, UPGRADE_VERSION,
    },
    ConditionList,
};
use botanix_storage::models::Vote;

/// Tests that an upgrade activates even with a validator voting Nay but
/// accepting.
///
/// This test verifies that:
/// 1. Two validators (ALICE, BOB) vote Aye for an upgrade
/// 2. One validator (EVE) votes Nay but is configured to accept the upgrade
/// 3. The upgrade still activates because:
///    - All validators are accepting the upgrade (100% acceptance)
///    - Aye votes reach 67%, meeting the quorum requirement
/// 4. After activation, all validators, including EVE, process blocks with the new version
/// 5. This shows that validators can signal disagreement while still accepting majority decisions
#[test]
fn activation_manager_vote_nay_and_accept() {
    let upgrade_height = 3;
    let required_approval_rate = 67;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye)
        // NOTE: Eve votes Nay, but accepts the upgrade.
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
                aye_votes: 1,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 1,
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
                aye_votes: 2,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 2,
                total_votes: 2,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 2);

    //
    // Block 2: Eve proposes and votes NAY, but accepts the upgrade.
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
                // NOTE: Eve: votes Nay, but is compliant
                aye_approval_rate: 67,
                comp_approval_rate: 100,
                aye_votes: 2,
                nay_votes: 1,
                abstained_votes: 0,
                compliant_count: 3,
                total_votes: 3,
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
                aye_votes: 0,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0,
                total_votes: 0,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 4);

    //
    // Block 4-20: Alice keeps building UPGRADED blocks, everyone accepts.
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
