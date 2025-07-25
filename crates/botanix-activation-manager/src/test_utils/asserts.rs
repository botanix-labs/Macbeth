//! This module provides conformance tests that verify the correct behavior of any
//! implementation of the `ActivationManagerReaderWriter` trait. These tests ensure
//! that voting mechanics, approval rate calculations, vote expiration, and data
//! consistency work as expected across different storage backends.

use botanix_storage::{models::Vote, ActivationManagerReaderWriter};

/// Tests approval rate calculations and vote management behavior.
///
/// This function validates that an `ActivationManagerReaderWriter`
/// implementation correctly handles:
/// - Recording votes with different compliance statuses
/// - Calculating approval rates for both "Aye" votes and compliance
/// - Handling minimum validator count thresholds in rate calculations
/// - Updating votes only when block height increases
/// - Removing expired votes based on block height
/// - Using ceiling division for percentage calculations
///
/// The test covers scenarios with varying minimum validator counts (above
/// and below actual vote counts) and verifies that vote removal correctly
/// affects approval rates.
///
/// # Parameters
/// * `alice` - First validator identifier for testing
/// * `bob` - Second validator identifier for testing
/// * `eve` - Third validator identifier for testing
/// * `db` - The empty database instance to test
pub fn assert_threshold_rates<Auth, DB: ActivationManagerReaderWriter<Auth>>(
    alice: Auth,
    bob: Auth,
    eve: Auth,
    db: DB,
) where
    Auth: Clone,
{
    let min_validator_count = 3;

    // Alice votes Aye, is not compliant
    db.update_upgrading_vote(alice, Vote::Aye, false, 1).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));

    // Bob votes Nay, is not compliant
    db.update_upgrading_vote(bob, Vote::Nay, false, 1).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));

    // Eve votes Abstain, is not compliant
    db.update_upgrading_vote(eve.clone(), Vote::Abstain, false, 1).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));

    // Eve votes Aye, IS compliant - NOT counted, reused height
    db.update_upgrading_vote(eve.clone(), Vote::Aye, true, 1).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));

    // Eve votes Aye, IS compliant - IS counted, incremented height
    db.update_upgrading_vote(eve, Vote::Aye, true, 2).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (67, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // Adjust minimum voter count (ABOVE total votes)
    let min_validator_count = 5;
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (40, 5));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (20, 5));

    // Adjust minimum voter count (BELOW total votes)
    let min_validator_count = 1;
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (67, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // Remove votes
    let min_validator_count = 3;

    // > Height 0
    let res = db.remove_upgrading_votes(0).unwrap();
    assert_eq!(res, 0); // None removed
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (67, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // > Height 1
    let res = db.remove_upgrading_votes(1).unwrap();
    assert_eq!(res, 0); // None removed
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (67, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // > Height 2
    let res = db.remove_upgrading_votes(2).unwrap();
    assert_eq!(res, 2); // Alice and Bob removed
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // > Height 3
    let res = db.remove_upgrading_votes(3).unwrap();
    assert_eq!(res, 1); // Eve removed
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));
}

/// Tests raw vote counting and compliance tracking functionality.
///
/// This function validates that an `ActivationManagerReaderWriter`
/// implementation correctly handles:
/// - Counting individual vote types (Aye, Nay, Abstain)
/// - Tracking compliance status independently of vote choice
/// - Maintaining consistent total voter counts across all query methods
/// - Replacing previous votes when block height increases
/// - Removing votes based on block height expiration
/// - Ensuring vote updates only occur with higher block heights
///
/// The test verifies that all vote counting methods return consistent total
/// counts and that vote replacement works correctly when validators change
/// their votes at higher block heights.
///
/// # Parameters
/// * `alice` - First validator identifier for testing
/// * `bob` - Second validator identifier for testing
/// * `eve` - Third validator identifier for testing
/// * `db` - The empty database instance to test
pub fn assert_polling<Auth, DB: ActivationManagerReaderWriter<Auth>>(
    alice: Auth,
    bob: Auth,
    eve: Auth,
    db: DB,
) where
    Auth: Clone,
{
    // Alice votes Aye, is not compliant
    db.update_upgrading_vote(alice, Vote::Aye, false, 1).unwrap();
    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 1); // Alice
    assert_eq!(nays, 0);
    assert_eq!(abstains, 0);
    assert_eq!(compliance, 0);

    assert_eq!(total, 1);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // Bob votes Nay, is not compliant
    db.update_upgrading_vote(bob, Vote::Nay, false, 1).unwrap();
    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 1); // Alice
    assert_eq!(nays, 1); // Bob
    assert_eq!(abstains, 0);
    assert_eq!(compliance, 0);

    assert_eq!(total, 2);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // Eve votes Abstain, is not compliant
    db.update_upgrading_vote(eve.clone(), Vote::Abstain, false, 1).unwrap();
    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 1); // Alice
    assert_eq!(nays, 1); // Bob
    assert_eq!(abstains, 1); // Eve
    assert_eq!(compliance, 0);

    assert_eq!(total, 3);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // Eve votes Aye, is compliant - NOT counted, reused height
    db.update_upgrading_vote(eve.clone(), Vote::Aye, true, 1).unwrap();
    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 1); // Alice
    assert_eq!(nays, 1); // Bob
    assert_eq!(abstains, 1); // Eve
    assert_eq!(compliance, 0);

    assert_eq!(total, 3);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // Eve votes Aye, is compliant - IS counted, incremented height
    db.update_upgrading_vote(eve, Vote::Aye, true, 2).unwrap();
    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 2); // Alice, Eve
    assert_eq!(nays, 1); // Bob
    assert_eq!(abstains, 0); // Eve's previous vote got replaced!
    assert_eq!(compliance, 1); // Eve

    assert_eq!(total, 3);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // Remove votes
    // > Height 0
    let res = db.remove_upgrading_votes(0).unwrap();
    assert_eq!(res, 0); // None removed

    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 2); // Alice, Eve
    assert_eq!(nays, 1); // Bob
    assert_eq!(abstains, 0);
    assert_eq!(compliance, 1); // Eve

    assert_eq!(total, 3);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // > Height 1
    let res = db.remove_upgrading_votes(1).unwrap();
    assert_eq!(res, 0); // None removed

    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 2); // Alice, Eve
    assert_eq!(nays, 1); // Bob
    assert_eq!(abstains, 0);
    assert_eq!(compliance, 1); // Eve

    assert_eq!(total, 3);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // > Height 2
    let res = db.remove_upgrading_votes(2).unwrap();
    assert_eq!(res, 2); // Alice and Bob removed
                        //
    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 1); // Eve
    assert_eq!(nays, 0);
    assert_eq!(abstains, 0);
    assert_eq!(compliance, 1); // Eve

    assert_eq!(total, 1);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);

    // > Height 3
    let res = db.remove_upgrading_votes(3).unwrap();
    assert_eq!(res, 1); // Eve removed

    let (ayes, total) = db.get_aye_votes().unwrap();
    let (nays, t2) = db.get_nay_votes().unwrap();
    let (abstains, t3) = db.get_abstained_votes().unwrap();
    let (compliance, t4) = db.get_compliance_count().unwrap();

    assert_eq!(ayes, 0);
    assert_eq!(nays, 0);
    assert_eq!(abstains, 0);
    assert_eq!(compliance, 0);

    assert_eq!(total, 0);
    assert_eq!(total, t2);
    assert_eq!(total, t3);
    assert_eq!(total, t4);
}
