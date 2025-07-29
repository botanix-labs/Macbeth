use crate::models::Vote;
use reth_storage_errors::provider::ProviderResult;

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
    /// This method counts all validators who have cast an "Abstain"
    /// vote for the network upgrade, regardless of their compliance status.
    ///
    /// # Returns
    /// * `Ok((abstain_count, total_voters))` where:
    ///   - `abstain_count` is the number of validators who voted "Abstain"
    ///   - `total_voters` is the total number of distinct validators who have cast any vote
    /// * `Err` if there was an error retrieving the vote counts
    fn get_abstained_votes(&self) -> ProviderResult<(usize, usize)>;

    /// Returns the count of validators who are compliant with the upgrade and
    /// the total number of validators who have voted.
    ///
    /// This method counts all validators who have indicated they are ready to
    /// accept the upgrade by setting `is_compliant` to `true` when casting
    /// their vote, regardless of whether they voted "Aye", "Nay", or "Abstain".
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
    /// ```text
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
    /// ```text
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
