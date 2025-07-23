use botanix_activation_manager::{
    test_utils::{
        Expectations, UpgradeTestFixture, ACTIVE_VERSION, ALICE, BOB, EVE, UPGRADE_VERSION,
    },
    ConditionList,
};
use botanix_storage::models::Vote;

/// Tests that a lagging validator can join the network after an upgrade has
/// occurred.
///
/// This test verifies that:
/// 1. Two validators (ALICE, BOB) can successfully activate an upgrade when they meet all
///    conditions
/// 2. A third validator (EVE) who joins later and is compliant with the upgrade can successfully
///    process blocks with the new version
/// 3. The lagging validator correctly rejects upgraded blocks during the `process_proposal` phase
///    but accepts them during finalize_block phase
/// 4. After finalizing the first upgraded block, the lagging validator properly updates its active
///    version and can fully participate in the network
#[test]
fn activation_manager_accept_lagging_validator() {
    let upgrade_height = 2;
    let required_approval_rate = 67;

    let mut f = UpgradeTestFixture::new(upgrade_height, required_approval_rate)
        .setup_compliant_validator(ALICE, Vote::Aye)
        .setup_compliant_validator(BOB, Vote::Aye);
    // NOTE: Eve is not in the validator set, but will be added later.

    assert_eq!(f.next_height(), 0);

    //
    // Block 0: Alice proposes and votes.
    //

    f.start_proposal(ALICE, ACTIVE_VERSION)
        // Alice and Bob do accept the upgrade.
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                comp_req: true,
                comp_approval_req: false,
                aye_approval_req: false,
                block_height_req: false,
            },
        )
        // Eve is not configured.
        .upgrade_conditions_empty(&[EVE])
        .expectations(
            &[ALICE, BOB],
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
        // Eve is not configured.
        .expectations_empty(&[EVE])
        .build_block();

    assert_eq!(f.next_height(), 1);

    //
    // Block 1: Bob proposes and votes.
    //

    f.start_proposal(BOB, ACTIVE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                comp_req: true,
                comp_approval_req: false, // not finalized yet
                aye_approval_req: false,  // not finalized yet
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
                comp_approval_rate: 67,
                aye_votes: 2,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 2,
                total_votes: 2,
            },
        )
        .expectations_empty(&[EVE])
        .build_block();

    assert_eq!(f.next_height(), 2);
    assert_eq!(f.next_height(), upgrade_height);

    //
    // Block 2: Alice proposes the UPGRADE, all conditions are met!
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        .upgrade_conditions(
            &[ALICE, BOB],
            ConditionList {
                comp_req: true,
                comp_approval_req: true, // IS met!
                aye_approval_req: true,  // IS met!
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
                comp_approval_rate: 0,
                aye_votes: 0,
                nay_votes: 0,
                abstained_votes: 0,
                compliant_count: 0,
                total_votes: 0,
            },
        )
        // Eve is not configured.
        .expectations_empty(&[EVE])
        .build_block();

    assert_eq!(f.next_height(), 3);

    //
    // Eve joins the validator set with a willingness to upgrade.
    //

    let mut f = f.setup_compliant_validator(EVE, Vote::Aye);

    //
    // Block 3: Alice builds an UPGRADED block, Eve accepts it because it was
    // finalized.
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        // No upgrade is ongoing for Alice and Bob.
        .upgrade_conditions_empty(&[ALICE, BOB])
        // Eves' upgrade tracker is still ongoing.
        .upgrade_conditions(
            &[EVE],
            ConditionList {
                comp_req: true,
                comp_approval_req: false,
                aye_approval_req: false,
                block_height_req: true, // IS met!
            },
        )
        .expectations(
            &[ALICE, BOB],
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
        // Eve REJECTS the upgrade during the backing phase, but ACCEPTS it
        // during the finalization phase.
        .expectations(
            &[EVE],
            Expectations {
                process_pass: false, // Reject!
                finalize_pass: true, // Accept
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
    // Block 4-20: Alice keeps building UPGRADED blocks, Eve keeps accepting.
    //

    f.start_proposal(ALICE, UPGRADE_VERSION)
        // No upgrade ongoing.
        .upgrade_conditions_empty(&[ALICE, BOB, EVE])
        // Eve now accepts the UPGRADED blocks during the backing and
        // finalization phase.
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
