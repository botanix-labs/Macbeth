use crate::activation_manager::{
    tests::utils::{Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE},
    ConditionList,
};
use reth_db::models::activation_manager::Vote;

/// Tests that votes expire after the retention period and are no longer counted.
///
/// This test verifies that:
/// 1. All validators vote for and are willing to accept an upgrade
/// 2. With a reduced vote retention period of 10 blocks, votes begin to expire
/// 3. By block 12, BOB's vote (cast in block 1) expires and is no longer counted
/// 4. By block 13, EVE's vote (cast in block 2) also expires
/// 5. Even when the target height is reached, the upgrade doesn't activate because expired votes
///    reduce the validator count below the minimum requirement
/// 6. This ensures that upgrade decisions reflect recent consensus rather than outdated votes
#[test]
fn activation_manager_vote_expiration() {
    let upgrade_height = 20;
    let required_approval_rate = 67;
    let vote_retention_period = 10;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        // NOTE: Setup the vote retention period.
        .vote_retention_period(vote_retention_period)
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
                aye_votes: 3,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 3,
                total_votes: 3,
            },
        )
        .build_block();

    assert_eq!(f.next_height(), 3);
    assert_eq!(vote_retention_period, 10);
    assert_eq!(upgrade_height, 20);

    //
    // Block 3-11: Alice keeps building, meanwhile Bob's and Eve's votes
    // approach the expiration target.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: true, // IS met!
                aye_approval_req: true,  // IS met!
                block_height_req: false, // IS NOT met!
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                aye_approval_rate: 100,
                comp_approval_rate: 100,
                aye_votes: 3,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 3,
                total_votes: 3,
            },
        )
        .build_blocks_until(12);

    assert_eq!(f.next_height(), 12);

    //
    // Block 12: Alice builds the block; Bob voted last in block 2, his vote
    // EXPIRED!
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: true,
                aye_approval_req: true,
                block_height_req: false,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                // NOTE: Bob's vote expired!
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

    assert_eq!(f.next_height(), 13);

    //
    // Block 13: Alice builds the block, Eve voted last in block 3, his vote
    // EXPIRED!
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: true, // not finalized yet
                aye_approval_req: true,  // not finalized yet
                block_height_req: false,
            },
        )
        .expectations(
            &[ALICE, BOB, EVE],
            Expectations {
                process_pass: true,
                finalize_pass: true,
                // NOTE: Eve's vote expired!
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

    assert_eq!(f.next_height(), 14);

    //
    // Block 14-19: Alice keeps building right before the height of the upgrade.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false, // IS NOT me!
                aye_approval_req: false,  // IS NOT met!
                block_height_req: false,  // IS NOT met!
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
        .build_blocks_until(20);

    assert_eq!(f.next_height(), 20);
    assert_eq!(upgrade_height, 20);

    //
    // Block 20-30: Alice keeps building, but the upgrade never happens.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB, EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false,
                aye_approval_req: false,
                block_height_req: true, // IS met!
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
        .build_blocks_until(31);

    assert_eq!(f.next_height(), 31);
}
