use crate::{ActivationManager, MIN_QUORUM, MIN_VALIDATOR_COUNT, VOTE_RETENTION_PERIOD};
use botanix_storage::{
    models::{RuntimeVersion, Vote},
    ActivationManagerReaderWriter,
};

/// Builder for constructing an `ActivationManager` with specific configuration.
///
/// This builder provides methods to configure how a validator will handle
/// network upgrades - whether it will ignore them, signal votes without
/// being compliant, or fully accept upgrades when conditions are met.
pub struct ActivationManagerBuilder<DB, Auth> {
    /// The database client implementing ActivationManagerReaderWriter
    client: DB,

    /// The currently active runtime version
    active_version: RuntimeVersion,

    /// How long votes are retained in blocks before expiring
    vote_retention_period: u64,

    /// Configuration for a pending network upgrade, if any
    upgrade: Option<crate::manager::NetworkUpgrade>,

    _p: std::marker::PhantomData<Auth>,
}

impl<DB, Auth> ActivationManagerBuilder<DB, Auth>
where
    DB: ActivationManagerReaderWriter<Auth>,
{
    /// Creates a new ActivationManagerBuilder with specified database client and active version.
    ///
    /// This is the primary constructor for initializing the builder with a storage backend
    /// and the current active runtime version.
    ///
    /// # Parameters
    /// * `client` - The database client implementing ActivationManagerReaderWriter
    /// * `active_version` - The currently active runtime version
    ///
    /// # Returns
    /// * A new ActivationManagerBuilder configured with default settings
    pub fn new(client: DB, active_version: RuntimeVersion) -> Self {
        ActivationManagerBuilder {
            client,
            active_version,
            vote_retention_period: VOTE_RETENTION_PERIOD,
            upgrade: None,
            _p: std::marker::PhantomData,
        }
    }

    /// Sets a custom vote retention period.
    ///
    /// This method allows customizing how long votes are retained before
    /// being pruned. The default is `VOTE_RETENTION_PERIOD` (518,400 blocks).
    ///
    /// # Parameters
    /// * `period` - The number of blocks to retain votes
    ///
    /// # Returns
    /// The builder instance with updated vote retention period.
    pub fn vote_retention_period(mut self, period: u64) -> Self {
        self.vote_retention_period = period;
        self
    }

    /// Finalizes the builder to create an ActivationManager that ignores
    /// network upgrades.
    ///
    /// This creates a manager that will:
    /// * Not participate in upgrade voting
    /// * Not include any NetworkUpgradePayload in proposals
    /// * Reject any blocks with versions other than the current active version
    ///
    /// This is the default operating mode for nodes that are not participating
    /// in the upgrade process and will result in the node rejecting blocks if
    /// an upgrade is activated on the network.
    ///
    /// # Returns
    /// * An ActivationManager configured to ignore network upgrades
    pub fn build_ignore_nework_upgrade(self) -> ActivationManager<DB, Auth> {
        debug_assert!(self.upgrade.is_none());
        self._finalize()
    }

    /// Finalizes the builder to create an ActivationManager that signals a vote
    /// on an upgrade.
    ///
    /// This creates a manager that will:
    /// * Include the specified vote in the NetworkUpgradePayload of proposals
    /// * Still reject any blocks with the upgrade version (is_compliant = false)
    /// * Track votes from other validators
    ///
    /// This mode allows validators to signal their support or opposition to an
    /// upgrade before they are ready to accept upgraded blocks.
    ///
    /// # Parameters
    /// * `upgrade_version` - The runtime version of the proposed upgrade
    /// * `our_vote` - This validator's vote (Aye/Nay) on the upgrade
    ///
    /// # Returns
    /// * An ActivationManager configured to signal a vote on the upgrade
    ///
    /// # Panics
    /// * If the upgrade version is not greater than the active version
    pub fn build_signal_network_upgrade(
        mut self,
        upgrade_version: RuntimeVersion,
        our_vote: Vote,
    ) -> ActivationManager<DB, Auth> {
        assert!(self.active_version < upgrade_version);

        #[rustfmt::skip]
        let upgrade = crate::manager::NetworkUpgrade {
            version: upgrade_version,
            quorum: usize::MAX,
            min_validator_count: usize::MAX,
            target_height: u64::MAX,
            our_vote,
            is_compliant: false, // Reject
        };

        self.upgrade = Some(upgrade);
        self._finalize()
    }

    /// Finalizes the builder to create an ActivationManager that accepts an upgrade when conditions
    /// are met.
    ///
    /// This creates a manager that will:
    /// * Include the specified vote in the NetworkUpgradePayload of proposals
    /// * Accept and process blocks with the upgrade version once all conditions are met
    /// * Track votes from other validators and calculate approval rates
    /// * Transition to the upgrade version automatically when an upgraded block is finalized
    ///
    /// This mode should be used when validators are prepared for an upgrade and have
    /// the necessary code and migrations in place to handle the new version.
    ///
    /// # Parameters
    /// * `upgrade_version` - The runtime version of the proposed upgrade
    /// * `quorum` - The percentage approval rate (0-100) that must be reached for activation
    /// * `min_validator_count` - The minimum validator count used for calculating approval rates
    /// * `target_height` - The minimum block height at which the upgrade can activate
    /// * `our_vote` - This validator's vote (Aye/Nay), defaults to Aye if None
    ///
    /// # Returns
    /// * An ActivationManager configured to accept the upgrade when conditions are met
    ///
    /// # Panics
    /// * If the `upgrade_version` is not greater than the active version
    /// * If the `quorum` is less than `MIN_QUORUM`
    /// * If the `min_validator_count` is less than `MIN_VALIDATOR_COUNT`
    #[allow(non_snake_case)]
    #[rustfmt::skip]
    pub fn build_COMPLIANT_network_upgrade(
        mut self,
        upgrade_version: RuntimeVersion,
        quorum: usize,
        min_validator_count: usize,
        target_height: u64,
        our_vote: Option<Vote>,
    ) -> ActivationManager<DB, Auth> {
        assert!(
            self.active_version < upgrade_version,
            "upgrade version is older than the active version"
        );

        assert!(
            quorum >= MIN_QUORUM,
            "minimum validator count is less than the minimum required"
        );

        assert!(
            min_validator_count >= MIN_VALIDATOR_COUNT,
            "minimum validator count is less than the minimum required"
        );

        let upgrade = crate::manager::NetworkUpgrade {
            version: upgrade_version,
            quorum,
            min_validator_count,
            target_height,
            our_vote: our_vote.unwrap_or(Vote::Aye),
            is_compliant: true,
        };

        self.upgrade = Some(upgrade);
        self._finalize()
    }

    fn _finalize(self) -> ActivationManager<DB, Auth> {
        ActivationManager::new(
            self.client,
            self.vote_retention_period,
            self.active_version,
            self.upgrade,
        )
    }
}
