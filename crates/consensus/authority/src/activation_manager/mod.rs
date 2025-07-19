//! # Activation Manager
//!
//! Coordinates and manages network protocol upgrades in the Botanix blockchain.
//!
//! The activation manager is responsible for safely transitioning the blockchain
//! network to new protocol versions without causing chain splits or consensus
//! failures. It implements a distributed voting and activation mechanism that
//! allows validators to signal support, prepare for, and eventually activate
//! upgrades in a coordinated manner.
//!
//! ## Overview
//!
//! Network upgrades in Botanix follow a two-phase approach:
//! 1. **Signaling Phase**: Validators vote on upgrade proposals to gauge support
//! 2. **Compliance Phase**: Validators indicate readiness to process upgraded blocks
//!
//! An upgrade activates only when all of the following conditions are met:
//! - A sufficient percentage of validators have voted "Aye" (meeting the quorum approval rate)
//! - A sufficient percentage of validators are compliant with the upgrade (meeting the quorum
//!   approval rate)
//! - A minimum number of distinct validators have participated in voting
//! - The current block height has reached or passed the scheduled target height
//! - The local node is compliant with the upgrade
//!
//! ## Integration with CometBFT
//!
//! The activation manager integrates with CometBFT's ABCI at three critical points:
//!
//! 1. **Block Proposal** (`on_prepare_proposal`): Determines which runtime version to use and
//!    includes the proposer's vote on pending upgrades.
//!
//! 2. **Block Validation** (`on_process_proposal`): Validates incoming block proposals based on
//!    their version and the current upgrade state.
//!
//! 3. **Block Finalization** (`on_finalize_block`): Tracks votes, finalizes blocks, and handles
//!    version transitions when upgrades are activated.
//!
//! ## Configuration Modes
//!
//! Nodes can be configured in three modes:
//!
//! - **Ignore Upgrades**: Default mode where the node doesn't participate in upgrades and rejects
//!   blocks with versions other than the current active version.
//!
//! - **Signal Only**: The node votes on upgrades but doesn't accept upgraded blocks, allowing
//!   validators to signal intentions before updating their software.
//!
//! - **Accept Upgrades**: The node votes on and accepts upgrades when conditions are met, enabling
//!   full participation in the upgrade process.
//!
//! ## Vote Tracking and Expiration
//!
//! The manager tracks votes from validators and calculates support approval rates for:
//! - Aye votes (percentage of validators voting in favor)
//! - Compliant validators (percentage of validators ready to upgrade)
//!
//! Votes automatically expire after a configured retention period (default: 30 days)
//! to ensure that upgrade decisions reflect recent consensus rather than outdated votes.
//!
//! ## Usage
//!
//! Nodes typically start in the "Ignore Upgrades" mode by default. When a potential
//! upgrade is discussed, validators can switch to "Signal Only" mode to participate in
//! voting. Once a new node version with upgrade support is released, validators can
//! switch to "Accept Upgrades" mode to fully participate in the upgrade process.

use crate::comet_bft::non_deterministic_data::NetworkUpgradePayload;
use reth_provider::{ActivationManagerReaderWriter, ProviderResult};
use std::sync::{Arc, RwLock};

// Reexports
pub use reth_db::models::activation_manager::{RuntimeVersion, Vote};

#[cfg(test)]
mod tests;

/// The minimum required quorum percentage for network upgrades.
///
/// This represents the minimum percentage (67%) of validators that must:
/// 1. Vote "Aye" for an upgrade proposal
/// 2. Be configured to accept the upgrade
///
/// This approval rate ensures a supermajority consensus for any network upgrade,
/// reducing the risk of chain splits while allowing for a reasonable approval rate
/// that can be achieved in practice.
///
/// The value is set at 67% to align with CometBFT's 2/3 consensus approval rate,
/// ensuring that network upgrades only activate when they have similar levels
/// of support as other consensus decisions.
pub const MIN_QUORUM: usize = 67;

/// The minimum validator count used for calculating approval rates.
///
/// This constant sets a minimum denominator when calculating approval rates,
/// ensuring that upgrades require broad support even when few validators have
/// explicitly voted. By using this minimum in the denominator, we prevent
/// scenarios where a small number of active validators could reach the required
/// approval percentage without true network-wide support.
pub const MIN_VALIDATOR_COUNT: usize = 3;
// (TODO lamafab): This should be increased with time as we move towards larger
// and dynamic federations, eventually.

/// The period (in blocks) for which votes are retained and considered valid.
///
/// Votes older than this many blocks are automatically pruned and no longer
/// count toward quorum calculations. This ensures that upgrade decisions
/// reflect the current state of the network and prevents old votes from
/// influencing current decisions.
///
/// At an average rate of 12 blocks per minute (5 second blocks), this equals:
/// - 518,400 blocks
/// - 43,200 minutes
/// - 720 hours
/// - 30 days
///
/// This extended retention period allows validators sufficient time to:
/// 1. Signal support for an upgrade
/// 2. Update their software to prepare for the upgrade
/// 3. Switch their configuration to accept the upgrade
///
/// While ensuring votes eventually expire if an upgrade does not proceed.
pub const VOTE_RETENTION_PERIOD: u64 = 518_400;

/// The decision returned by the activation manager during the prepare proposal
/// phase.
///
/// This struct represents the decision made by a validator when preparing a new
/// block proposal. It contains the runtime version to use for the block, any
/// vote the validator wishes to include in the proposal, and the current state
/// of upgrade conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct OnPrepareProposalDecision {
    /// The runtime version that should be used for the block proposal. This
    /// will be either the active version or an upgrade version if conditions
    /// are met.
    pub version: RuntimeVersion,

    /// The validator's vote on a pending upgrade to include in the proposal.
    /// This is included in the Non-Deterministic Data (NDD) of the block
    /// proposal. If None, no vote is included in the proposal.
    pub vote: Option<NetworkUpgradePayload>,

    /// The current state of all upgrade conditions. This is used for diagnostic
    /// purposes to understand why an upgrade is being proposed or not proposed.
    pub conditions: Option<ConditionList>,
}

/// The decision returned by the activation manager when processing a block
/// proposal.
///
/// This enum represents whether a validator should accept or reject a proposed
/// block based on the runtime version and network upgrade conditions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnProcessProposalDecision {
    /// Accept and process the block proposal.
    ///
    /// This is returned when the block version is valid (either the active
    /// version or an upgrade version that meets all conditions).
    Process {
        /// The runtime version of the block being processed
        version: RuntimeVersion,
        /// The current state of all upgrade conditions (for diagnostic
        /// purposes)
        conditions: Option<ConditionList>,
    },

    /// Reject the block proposal.
    ///
    /// This is returned when the block version is invalid (not the active
    /// version and not an upgrade version that meets all conditions).
    RejectBlock {
        /// The runtime version of the block being rejected
        version: RuntimeVersion,
        /// The current state of all upgrade conditions (for diagnostic purposes)
        conditions: Option<ConditionList>,
    },
}

/// The decision returned by the activation manager when finalizing a block.
///
/// This enum represents whether a validator should finalize a block or reject
/// it as a dead end (meaning the validator cannot continue on this chain).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnFinalizeBlockDecision {
    /// Finalize the block and continue processing the chain.
    ///
    /// This is returned for blocks with versions the validator can process.
    Finalize {
        /// The runtime version of the block being finalized
        version: RuntimeVersion,
    },

    /// Reject the block as a dead end that the validator cannot follow.
    ///
    /// This is returned for blocks with versions the validator explicitly
    /// refuses to support or cannot process. This results in a consensus split.
    RejectBlockDeadEnd {
        /// The runtime version of the block being rejected
        version: RuntimeVersion,
    },
}

/// Internal representation of a network upgrade configuration.
///
/// This struct holds all the parameters and state related to a pending network
/// upgrade that a validator is tracking.
struct NetworkUpgrade {
    /// The runtime version of the proposed upgrade
    version: RuntimeVersion,

    /// The percentage approval rate (0-100) that must be reached for both
    /// Aye votes and compliant validators to activate the upgrade
    quorum: usize,

    /// The minimum number of distinct validators that must participate
    /// in voting for the upgrade to be considered valid
    min_validator_count: usize,

    /// The minimum block height at which the upgrade can activate
    target_height: u64,

    /// This validator's vote on the upgrade proposal (Aye/Nay)
    our_vote: Vote,

    /// Whether this validator is compliant with the upgrade
    is_compliant: bool,
}

/// A list of boolean flags indicating the status of each upgrade condition.
///
/// This struct tracks whether each condition required for a network upgrade is
/// currently satisfied. It's used for diagnostics and decision making.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConditionList {
    /// Whether the validator is compliant with the upgrade
    comp_req: bool,

    /// Whether the quorum approval rate for Aye votes has been reached
    aye_approval_req: bool,

    /// Whether the quorum approval rate for compliant validators has been reached
    comp_approval_req: bool,

    /// Whether the current block height is at or above the target height
    block_height_req: bool,
}

impl ConditionList {
    /// Checks if all upgrade conditions are passing.
    ///
    /// Returns true only when all conditions are satisfied, indicating
    /// that the network is ready to activate the upgrade.
    pub fn all_passing(&self) -> bool {
        self.comp_req && self.aye_approval_req && self.comp_approval_req && self.block_height_req
    }
}

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
    upgrade: Option<NetworkUpgrade>,

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
        let upgrade = NetworkUpgrade {
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

        let upgrade = NetworkUpgrade {
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
        ActivationManager {
            client: self.client,
            vote_retention_period: self.vote_retention_period,
            active_version: Arc::new(RwLock::new(self.active_version)),
            upgrade: Arc::new(RwLock::new(self.upgrade)),
            _p: std::marker::PhantomData,
        }
    }
}

/// Manages network protocol upgrades in the Botanix blockchain.
///
/// The `ActivationManager` is responsible for coordinating the network-wide
/// upgrade process, ensuring that validators can safely transition to new
/// protocol versions without causing chain splits or consensus failures. It
/// tracks validator votes, calculates support approval rates, and determines when
/// proposed upgrades should activate.
///
/// This component interfaces with CometBFT's ABCI during three critical phases:
/// 1. Block proposal preparation (`on_prepare_proposal`)
/// 2. Block proposal validation (`on_process_proposal`)
/// 3. Block finalization (`on_finalize_block`)
///
/// During each phase, the manager makes appropriate decisions based on the
/// current upgrade state, validator votes, and configured upgrade conditions.
///
/// The manager maintains thread-safe access to the active version and upgrade
/// state through atomic reference counting and read-write locks, allowing it to
/// be shared between concurrent contexts.
#[derive(Clone)]
pub struct ActivationManager<DB, Auth> {
    /// Database client for persisting and retrieving votes
    client: DB,

    /// How long (in blocks) votes are retained before expiring
    vote_retention_period: u64,

    /// The currently active runtime version
    /// Protected by a read-write lock for thread-safe updates
    active_version: Arc<RwLock<RuntimeVersion>>,

    /// Configuration for a pending network upgrade, if any
    /// Protected by a read-write lock for thread-safe updates
    upgrade: Arc<RwLock<Option<NetworkUpgrade>>>,

    _p: std::marker::PhantomData<Auth>,
}

impl<DB, Auth> ActivationManager<DB, Auth>
where
    DB: ActivationManagerReaderWriter<Auth>,
{
    /// Prepares a proposal decision based on the current upgrade state.
    ///
    /// This method is called during CometBFT's `prepare_proposal` phase to determine:
    /// 1. Which runtime version to use for the proposed block (active or upgrade)
    /// 2. Whether to include a vote for a pending network upgrade
    /// 3. Whether all conditions are met to propose an upgraded block
    ///
    /// ## Behavior
    ///
    /// - Returns the appropriate block version to propose (active or upgrade)
    /// - Includes this validator's vote on pending upgrades regardless of decision
    /// - Proposes an upgraded block only when all upgrade conditions are met:
    ///   * Validator is compliant with the upgrade
    ///   * Quorum approval rate for Aye votes is reached
    ///   * Quorum approval rate for compliant validators is reached
    ///   * Current block height is at or above the target height
    ///
    /// # Parameters
    /// * `block_height` - The height of the block being proposed
    ///
    /// # Returns
    /// * `OnPrepareProposalDecision` containing:
    ///   - The version to use (active or upgrade)
    ///   - Optional vote to include in the proposal
    ///   - Status of all upgrade conditions
    pub fn on_prepare_proposal(
        &self,
        block_height: u64,
    ) -> ProviderResult<OnPrepareProposalDecision> {
        let mut our_vote = None;
        let mut conditions = None;

        // Check if we're tracking a network upgrade.
        let maybe_upgrade = self.upgrade.read().expect("poisoned lock");
        if let Some(upgrade) = maybe_upgrade.as_ref() {
            assert!(upgrade.quorum >= MIN_QUORUM);

            // Prepare our vote information to be included in the proposal.
            our_vote = Some(NetworkUpgradePayload {
                version: upgrade.version,
                vote: upgrade.our_vote,
                is_compliant: upgrade.is_compliant,
            });

            // Process the upgraded block only if ALL conditions are met.
            let c = self._validate_conditions(upgrade, block_height)?;
            if c.all_passing() {
                // Signal that we should create a proposal with the upgraded version
                return Ok(OnPrepareProposalDecision {
                    version: upgrade.version,
                    vote: our_vote,
                    conditions: Some(c),
                });
            }

            conditions = Some(c);
        }

        // Default case: propose a block with the current active version. We
        // include our vote (if any) to signal our upgrade intentions, even when
        // not yet proposing upgraded blocks.
        let active_version = self.active_version.read().expect("poisoned lock");
        Ok(OnPrepareProposalDecision { version: *active_version, vote: our_vote, conditions })
    }

    /// Processes an incoming block proposal, tracking votes and determining if
    /// it should be accepted.
    ///
    /// This method is called during CometBFT's `process_proposal` phase to
    /// determine whether to accept or reject a block proposal based on its
    /// version and the current upgrade state.
    ///
    /// ## Behavior
    ///
    /// - Accepts blocks with the active version
    /// - Accepts blocks with an upgrade version if the validator is compliant with the upgrade AND
    ///   all conditions are met
    /// - Rejects blocks with unknown versions or when conditions aren't met
    ///
    /// # Parameters
    /// * `block_height` - The height of the block being processed
    /// * `block_version` - The runtime version of the block being processed
    ///
    /// # Returns
    /// * `OnProcessProposalDecision::Process` if the block should be accepted
    /// * `OnProcessProposalDecision::RejectBlock` if the block should be rejected
    pub fn on_process_proposal(
        &self,
        block_height: u64,
        block_version: RuntimeVersion,
    ) -> ProviderResult<OnProcessProposalDecision> {
        let mut conditions = None;

        // Check if block matches a pending upgrade we're tracking.
        let maybe_upgrade = self.upgrade.read().expect("poisoned lock");
        if let Some(upgrade) = maybe_upgrade.as_ref() {
            let c = self._validate_conditions(upgrade, block_height)?;
            conditions = Some(c);

            // Process the upgraded block only if ALL conditions are met and if
            // it matches the upgrade version.
            if c.all_passing() && block_version == upgrade.version {
                return Ok(OnProcessProposalDecision::Process {
                    version: upgrade.version,
                    conditions,
                });
            }
        }

        // Check if the proposed block uses the currently active runtime version.
        let active_version = self.active_version.read().expect("poisoned lock");
        if block_version == *active_version {
            return Ok(OnProcessProposalDecision::Process { version: *active_version, conditions });
        }

        Ok(OnProcessProposalDecision::RejectBlock { version: block_version, conditions })
    }

    /// Finalizes a block and handles potential version transitions.
    ///
    /// This method is called during CometBFT's `finalize_block` phase to track
    /// votes, finalize blocks, and handle version transitions when upgrades are
    /// activated.
    ///
    /// ## Behavior
    ///
    /// - Tracks the proposer's vote for network upgrades
    /// - Accepts any block version during sync (relying on CometBFT's consensus)
    /// - If an upgraded block is finalized, updates the active version
    /// - Rejects blocks if explicitly configured to reject that upgrade
    /// - Cleans up voting data once a version transition occurs
    /// - DOES NOT recheck quorum or target height during finalization
    ///
    /// # Parameters
    /// * `block_version` - The runtime version of the block being finalized
    /// * `block_height` - The height of the block being finalized
    /// * `proposer_address` - The public key of the block proposer
    /// * `proposer_vote` - The proposer's vote on a network upgrade, if any
    ///
    /// # Returns
    /// * `OnFinalizeBlockDecision::Finalize` if the block should be finalized
    /// * `OnFinalizeBlockDecision
    pub fn on_finalize_block(
        &self,
        block_version: RuntimeVersion,
        block_height: u64,
        proposer_address: Auth,
        proposer_vote: Option<NetworkUpgradePayload>,
    ) -> ProviderResult<OnFinalizeBlockDecision> {
        // Track the proposer's vote for upgrade intention, if provided
        self._track_vote(block_height, proposer_address, proposer_vote)?;

        // NOTE: Upgrade condition checks are deliberately omitted here. These
        // validations have already occurred during the backing phase in
        // `on_process_proposal`. When CometBFT finalizes an upgraded block, it
        // confirms that the required majority of validators have endorsed the
        // upgrade. Additionally, syncing nodes lack historical context about
        // previous forks (including migrations and code changes), preventing
        // them from independently validating past version transitions.
        // Consequently, any block finalized by CometBFT must be accepted by all
        // nodes.
        //
        // However, nodes can resist future upgrades, and their consequent
        // behavior is considered undefined.

        let active_version = self.active_version.read().expect("poisoned lock");
        if block_version <= *active_version {
            return Ok(OnFinalizeBlockDecision::Finalize { version: block_version });
        }

        let mut maybe_upgrade = self.upgrade.write().expect("poisoned lock");
        if let Some(upgrade) = maybe_upgrade.as_ref() {
            // Future version detected, reject the block; this node is running an
            // outdated version of this software that cannot handle those finalized
            // blocks.
            if block_version != upgrade.version {
                return Ok(OnFinalizeBlockDecision::RejectBlockDeadEnd { version: block_version });
            }

            // Prune **all** votes since the decision is now finalized.
            self.client.remove_upgrading_votes(block_height.saturating_add(1))?;

            // For nodes that are explicitly not accepting this upgrade version,
            // reject the block and halt further processing.
            if !upgrade.is_compliant {
                // Upgrade has been activated; clear pending upgrade.
                *maybe_upgrade = None;

                return Ok(OnFinalizeBlockDecision::RejectBlockDeadEnd { version: block_version });
            }

            std::mem::drop(active_version);

            // Upgrade is now active and mandatory for all future block
            // production. Update our active version and clear the pending
            // upgrade.
            let mut active_version = self.active_version.write().expect("poisoned lock");
            *active_version = upgrade.version;
            *maybe_upgrade = None;

            return Ok(OnFinalizeBlockDecision::Finalize { version: *active_version });
        }

        // Future version detected, reject the block; this node is running an
        // outdated version of this software that cannot handle those finalized
        // blocks.
        Ok(OnFinalizeBlockDecision::RejectBlockDeadEnd { version: block_version })
    }

    /// Validates all conditions required for an upgrade to proceed.
    ///
    /// This internal method checks whether all conditions required for a network
    /// upgrade are currently met, including:
    /// - Validator is configured to accept the upgrade
    /// - Quorum approval rate for Aye votes is reached
    /// - Quorum approval rate for compliant validators is reached
    /// - Current block height is at or above the target height
    ///
    /// # Parameters
    /// * `upgrade` - The NetworkUpgrade configuration to validate
    /// * `block_height` - The current block height
    ///
    /// # Returns
    /// * `ConditionList` containing the status of each upgrade condition
    fn _validate_conditions(
        &self,
        upgrade: &NetworkUpgrade,
        block_height: u64,
    ) -> ProviderResult<ConditionList> {
        let (aye_approval, total_votes) =
            self.client.get_upgrading_approval_rate_ayes(upgrade.min_validator_count)?;

        let (comp_approval, t2) =
            self.client.get_upgrading_approval_rate_compliance(upgrade.min_validator_count)?;

        debug_assert_eq!(total_votes, t2);
        debug_assert!(total_votes >= upgrade.min_validator_count);

        let list = ConditionList {
            comp_req: upgrade.is_compliant,
            aye_approval_req: aye_approval >= upgrade.quorum,
            comp_approval_req: comp_approval >= upgrade.quorum,
            block_height_req: block_height >= upgrade.target_height,
        };

        Ok(list)
    }

    /// Tracks validator votes for network upgrades.
    ///
    /// This internal method processes votes from block proposers and stores them
    /// in the database if they're relevant to a tracked upgrade. It also prunes
    /// outdated votes beyond the configured retention period.
    ///
    /// ## Behavior
    ///
    /// - Stores votes only if they're relevant to a tracked upgrade
    /// - Updates the database with vote details including block height
    /// - Prunes outdated votes beyond the configured retention period
    ///
    /// # Parameters
    /// * `block_height` - The height of the block containing the vote
    /// * `addr` - The public key of the validator casting the vote
    /// * `vote` - The NetworkUpgradePayload containing the vote details
    ///
    /// # Returns
    /// * `Ok(())` if the vote was successfully processed
    fn _track_vote(
        &self,
        block_height: u64,
        addr: Auth,
        vote: Option<NetworkUpgradePayload>,
    ) -> ProviderResult<()> {
        // Check if we're tracking an upgrade.
        let maybe_upgrade = self.upgrade.read().expect("poisoned lock");
        let Some(upgrade) = maybe_upgrade.as_ref() else {
            return Ok(());
        };

        let abstained_vote = || NetworkUpgradePayload {
            version: upgrade.version,
            vote: Vote::Absent,
            is_compliant: false,
        };

        // If the proposer provided an explicit vote, we check whether the
        // version matches our upgrade version. If the version is mismatched or
        // if no vote is provided at all, then we implicilty mark the vote as
        // abstained.
        let vote = if let Some(vote) = vote {
            if vote.version == upgrade.version {
                vote
            } else {
                abstained_vote()
            }
        } else {
            // Abstained vote
            abstained_vote()
        };

        // Then track the vote.
        self.client.update_upgrading_vote(addr, vote.vote, vote.is_compliant, block_height)?;

        // Prune old votes.
        let prune_below = block_height.saturating_sub(self.vote_retention_period);
        self.client.remove_upgrading_votes(prune_below)?;

        Ok(())
    }
}
