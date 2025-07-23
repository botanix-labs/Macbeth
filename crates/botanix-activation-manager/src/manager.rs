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

use botanix_storage::{
    models::{RuntimeVersion, Vote},
    ActivationManagerReaderWriter,
};
use reth_storage_errors::provider::ProviderResult;
use std::sync::{Arc, RwLock};

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
//
// TODO (lamafab): This should probably be based on validator set count, not
// some specific time period. Measuring sentiment over time and actually
// validating upgrade conditions are two separate things. Having to validate
// that many DB entries on each block is too expensive...
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
pub struct NetworkUpgrade {
    /// The runtime version of the proposed upgrade
    pub version: RuntimeVersion,

    /// The percentage approval rate (0-100) that must be reached for both
    /// Aye votes and compliant validators to activate the upgrade
    pub quorum: usize,

    /// The minimum number of distinct validators that must participate
    /// in voting for the upgrade to be considered valid
    pub min_validator_count: usize,

    /// The minimum block height at which the upgrade can activate
    pub target_height: u64,

    /// This validator's vote on the upgrade proposal (Aye/Nay)
    pub our_vote: Vote,

    /// Whether this validator is compliant with the upgrade
    pub is_compliant: bool,
}

/// A list of boolean flags indicating the status of each upgrade condition.
///
/// This struct tracks whether each condition required for a network upgrade is
/// currently satisfied. It's used for diagnostics and decision making.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConditionList {
    /// Whether the validator is compliant with the upgrade
    pub comp_req: bool,

    /// Whether the quorum approval rate for Aye votes has been reached
    pub aye_approval_req: bool,

    /// Whether the quorum approval rate for compliant validators has been reached
    pub comp_approval_req: bool,

    /// Whether the current block height is at or above the target height
    pub block_height_req: bool,
}

/// Current voting statistics for a network upgrade.
///
/// This struct provides a snapshot of validator voting activity for a pending
/// network upgrade, including the breakdown of different vote types and
/// compliance status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Polling {
    /// Number of validators who have voted "Aye" for the upgrade.
    pub ayes: usize,
    /// Number of validators who have voted "Nay" against the upgrade.
    pub nays: usize,
    /// Number of validators who have abstained from voting (or explicitly voted
    /// "Abstain").
    pub abstained: usize,
    /// Number of validators who are compliant with the upgrade (ready to
    /// accept it).
    pub compliant: usize,
    /// Total number of distinct validators who have cast any vote.
    pub total: usize,
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
    /// Creates a new `ActivationManager` instance with the specified configuration.
    pub fn new(
        client: DB,
        vote_retention_period: u64,
        active_version: RuntimeVersion,
        upgrade: Option<NetworkUpgrade>,
    ) -> Self {
        ActivationManager {
            client,
            vote_retention_period,
            active_version: Arc::new(RwLock::new(active_version)),
            upgrade: Arc::new(RwLock::new(upgrade)),
            _p: std::marker::PhantomData,
        }
    }

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
    /// * `proposer_address` - The address of the block proposer
    /// * `proposer_vote` - The proposer's vote on a network upgrade, if any. If `None` or if the
    ///   vote's version doesn't match the tracked upgrade, it's counted as abstained.
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
            // Prune **all** votes since the decision is now finalized.
            self.client.remove_upgrading_votes(block_height.saturating_add(1))?;

            // Future version detected, reject the block; this node is running an
            // outdated version of this software that cannot handle those finalized
            // blocks.
            if block_version != upgrade.version {
                return Ok(OnFinalizeBlockDecision::RejectBlockDeadEnd { version: block_version });
            }

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

    /// Returns current voting statistics for the tracked network upgrade.
    ///
    /// This method provides a snapshot of validator voting activity for the
    /// currently tracked upgrade, including vote counts and compliance status.
    /// The returned data can be used for monitoring upgrade progress and
    /// displaying voting statistics to users.
    ///
    /// # Returns
    /// * `Ok(Some((version, polling)))` if an upgrade is being tracked, where:
    ///   - `version` is the runtime version of the tracked upgrade
    ///   - `polling` contains current vote counts and compliance statistics
    /// * `Ok(None)` if no upgrade is currently being tracked
    /// * `Err` if there was an error retrieving voting data from the database
    pub fn get_upgrade_polling(&self) -> ProviderResult<Option<(RuntimeVersion, Polling)>> {
        let maybe_upgrade = self.upgrade.read().expect("poisoned lock");
        let Some(upgrade) = maybe_upgrade.as_ref() else {
            return Ok(None);
        };

        let (ayes, total) = self.client.get_aye_votes()?;
        let (nays, t2) = self.client.get_nay_votes()?;
        let (abstained, t3) = self.client.get_abstained_votes()?;
        let (compliant, t4) = self.client.get_compliance_count()?;

        debug_assert_eq!(total, t2);
        debug_assert_eq!(total, t3);
        debug_assert_eq!(total, t4);
        debug_assert_eq!(ayes + nays + abstained, total);

        let polling = Polling { ayes, nays, abstained, compliant, total };

        Ok(Some((upgrade.version, polling)))
    }

    /// Forces an upgrade to the specified version if it matches the currently
    /// tracked upgrade and the node is compliant.
    ///
    /// This method bypasses all normal upgrade conditions and immediately
    /// activates the specified upgrade version. It should only be called when
    /// the caller is certain that the upgrade has already been activated on the
    /// network, but the activation manager is unaware of this state (for
    /// example, due to a node restart after the upgrade was finalized).
    ///
    /// This method serves as a fast-track mechanism to synchronize the
    /// activation manager's internal state with the actual network state
    /// without waiting for upgraded blocks to be processed through the normal
    /// consensus flow.
    ///
    /// ## Safety
    ///
    /// This method should only be used when you are absolutely certain that:
    /// - The specified upgrade version has already been activated on the network
    /// - The node is capable of processing blocks with this version
    /// - The upgrade conditions were previously met through normal consensus
    ///
    /// Using this method incorrectly could cause the node to accept or propose blocks
    /// that the rest of the network rejects.
    ///
    /// # Parameters
    /// * `version` - The runtime version to force activate. Must match the currently tracked
    ///   upgrade version.
    ///
    /// # Returns
    /// * `true` if the upgrade was successfully forced (version matched, node is compliant)
    /// * `false` if no upgrade is being tracked, the version doesn't match, or the node is not
    ///   compliant with the upgrade
    pub fn force_upgrade_checked(&self, version: RuntimeVersion) -> bool {
        let maybe_upgrade = self.upgrade.read().expect("poisoned lock");
        let Some(upgrade) = maybe_upgrade.as_ref() else {
            return false;
        };

        if upgrade.version != version {
            return false;
        }

        if !upgrade.is_compliant {
            return false;
        }

        std::mem::drop(maybe_upgrade);

        // Upgrade is immediately active and mandatory for all future block
        // production. Update our active version and clear the pending upgrade.
        let mut upgrade = self.upgrade.write().expect("poisoned lock");
        *upgrade = None;
        //
        let mut active_version = self.active_version.write().expect("poisoned lock");
        *active_version = version;

        true
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
            vote: Vote::Abstain,
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

/// Represents a validator's stance on a network upgrade proposal.
///
/// This payload is included in each block's non-deterministic data when a node is
/// configured to participate in the network upgrade voting process. It communicates
/// the validator's current position on a specific upgrade version.
///
/// # Fields
///
/// * `version` - The specific runtime version that this vote applies to.
///
/// * `vote` - The validator's explicit opinion on the upgrade (Aye/Nay/Absent).
///
/// * `is_compliant` - Indicates whether the validator is technically ready to process blocks with
///   the upgrade version. When `true`, the validator has the necessary software version and
///   configuration to handle the upgrade. This can be independent of the vote - a validator may
///   vote `Nay` but still be prepared to follow the network if the upgrade is adopted.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct NetworkUpgradePayload {
    /// The runtime version that this vote applies to.
    pub version: RuntimeVersion,
    /// The validator's explicit opinion on the upgrade (Aye/Nay/Absent).
    pub vote: Vote,
    /// Indicates whether the validator is technically ready to process blocks with the upgrade
    /// version.
    pub is_compliant: bool,
}
