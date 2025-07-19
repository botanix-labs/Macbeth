use reth_db::models::activation_manager::Vote;
use reth_errors::ProviderResult;

/// Provides read and write operations for managing network upgrade votes and approval rates.
///
/// This trait provides functionality to track validators' votes on network upgrades,
/// calculate voting approval rates, and manage vote retention across blocks.
///
/// Implementations should maintain persistence of votes between blocks and handle
/// the removal of expired votes.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait ActivationManagerReaderWriter<Auth>: Send + Sync {
    /// Records or updates a validator's vote for a network upgrade.
    ///
    /// This method stores a validator's vote and acceptance status for a
    /// network upgrade at the given block height. If the validator has
    /// previously voted, their vote will be updated to the new values if and
    /// only if the botanix height is greater than the existing botanix height.
    ///
    /// # Parameters
    /// * `auth` - The public key identifying the validator
    /// * `vote` - The validator's vote (Aye or Nay)
    /// * `is_compliant` - Whether the validator is ready to accept the upgrade
    /// * `botanix_height` - The block height at which the vote was cast
    ///
    /// # Returns
    /// * `Ok(())` if the vote was successfully recorded
    /// * `Err` if there was an error recording the vote
    fn update_upgrading_vote(
        &self,
        auth: Auth,
        vote: Vote,
        is_compliant: bool,
        botanix_height: u64,
    ) -> ProviderResult<()>;

    /// Returns the count of validators who have voted "Aye" and the total
    /// number of validators who have voted.
    ///
    /// This method counts all validators who have cast an "Aye" vote for the
    /// network upgrade, regardless of their compliance status.
    ///
    /// # Returns
    /// * `Ok((aye_count, total_voters))` where:
    ///   - `aye_count` is the number of validators who voted "Aye"
    ///   - `total_voters` is the total number of distinct validators who have cast any vote
    /// * `Err` if there was an error retrieving the vote counts
    fn get_aye_votes(&self) -> ProviderResult<(usize, usize)>;

    /// Returns the count of validators who have voted "Nay" and the total
    /// number of validators who have voted.
    ///
    /// This method counts all validators who have cast a "Nay" vote for the
    /// network upgrade, regardless of their compliance status.
    ///
    /// # Returns
    /// * `Ok((nay_count, total_voters))` where:
    ///   - `nay_count` is the number of validators who voted "Nay"
    ///   - `total_voters` is the total number of distinct validators who have cast any vote
    /// * `Err` if there was an error retrieving the vote counts
    fn get_nay_votes(&self) -> ProviderResult<(usize, usize)>;

    /// Returns the count of validators who have abstained from voting and the
    /// total number of validators who have voted.
    ///
    /// This method counts all validators who have cast an "Absent" (abstain)
    /// vote for the network upgrade, regardless of their compliance status.
    ///
    /// # Returns
    /// * `Ok((abstain_count, total_voters))` where:
    ///   - `abstain_count` is the number of validators who voted "Absent" (abstained)
    ///   - `total_voters` is the total number of distinct validators who have cast any vote
    /// * `Err` if there was an error retrieving the vote counts
    fn get_abstained_votes(&self) -> ProviderResult<(usize, usize)>;

    /// Returns the count of validators who are compliant with the upgrade and
    /// the total number of validators who have voted.
    ///
    /// This method counts all validators who have indicated they are ready to
    /// accept the upgrade by setting `is_compliant` to `true` when casting
    /// their vote, regardless of whether they voted "Aye", "Nay", or "Absent".
    ///
    /// # Returns
    /// * `Ok((compliant_count, total_voters))` where:
    ///   - `compliant_count` is the number of validators who are compliant with the upgrade
    ///   - `total_voters` is the total number of distinct validators who have cast any vote
    /// * `Err` if there was an error retrieving the compliance counts
    fn get_compliance_count(&self) -> ProviderResult<(usize, usize)>;

    /// Calculates the approval rate of validators signaling support (voting Aye) for an upgrade.
    ///
    /// This method calculates the percentage of validators voting "Aye" out of the total
    /// number of validators who have cast votes, regardless of their compliance status.
    ///
    /// The formula used is:
    /// ```
    /// let total = total_votes.max(min_validator_count);
    /// let rate = (aye_votes * 100 + total - 1) / total
    /// ```
    /// This implements ceiling division to round up to the nearest percentage point.
    ///
    /// # Parameters
    /// * `min_validator_count` - The minimum number of validators required to calculate the
    ///   approval rate. `total` is set to that value if the total number of votes is less than
    ///   that.
    ///
    /// # Returns
    /// * `Ok((approval_rate, total))` where:
    ///   - `approval_rate` is the percentage (0-100) of Aye votes
    ///   - `total` is the number of distinct validators who have voted or `min_validator_count`,
    ///     whichever is greater.
    /// * `Err` if there was an error calculating the approval rate
    fn get_upgrading_approval_rate_ayes(
        &self,
        min_validator_count: usize,
    ) -> ProviderResult<(usize, usize)>;

    /// Calculates the approval rate of validators compliant with the upgrade.
    ///
    /// This method calculates the percentage of validators who are compliant
    /// with the upgrade out of the total number of validators who have cast
    /// votes, regardless of whether they voted Aye or Nay.
    ///
    /// The formula used is:
    /// ```
    /// let total = total_votes.max(min_validator_count);
    /// let rate = (compliant_votes * 100 + total - 1) / total
    /// ```
    /// This implements ceiling division to round up to the nearest percentage point.
    ///
    /// # Parameters
    /// * `min_validator_count` - The minimum number of validators required to calculate the
    ///   approval rate. `total` is set to that value if the total number of votes is less than
    ///   that.
    ///
    /// # Returns
    /// * `Ok((approval_rate, total_votes))` where:
    ///   - `approval_rate` is the percentage (0-100) of accepting validators
    ///   - `total` is the number of distinct validators who have voted or `min_validator_count`,
    ///     whichever is greater.
    /// * `Err` if there was an error calculating the approval
    fn get_upgrading_approval_rate_compliance(
        &self,
        min_validator_count: usize,
    ) -> ProviderResult<(usize, usize)>;

    /// Removes votes for blocks below the specified height.
    ///
    /// This method implements vote expiration by removing all votes that were
    /// recorded at block heights lower than the specified height. This ensures
    /// that only recent votes within the retention period are considered.
    ///
    /// # Parameters
    /// * `botanix_height` - Remove all votes recorded at heights lower than this
    ///
    /// # Returns
    /// * `Ok(count)` - The number of votes that were removed
    /// * `Err` if there was an error removing votes
    fn remove_upgrading_votes(&self, botanix_height: u64) -> ProviderResult<usize>;
}

/// Test utilities for validating `ActivationManagerReaderWriter` implementations.
///
/// This module provides conformance tests that verify the correct behavior of any
/// implementation of the `ActivationManagerReaderWriter` trait. These tests ensure
/// that voting mechanics, approval rate calculations, vote expiration, and data
/// consistency work as expected across different storage backends.
pub mod tests {
    use super::*;

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
        db.update_upgrading_vote(eve.clone(), Vote::Absent, false, 1).unwrap();
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
        db.update_upgrading_vote(eve.clone(), Vote::Aye, true, 2).unwrap();
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
    /// - Counting individual vote types (Aye, Nay, Absent)
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
        db.update_upgrading_vote(eve.clone(), Vote::Absent, false, 1).unwrap();
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
        db.update_upgrading_vote(eve.clone(), Vote::Aye, true, 2).unwrap();
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
}
