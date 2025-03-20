use super::Db;
use crate::activation_manager::{
    ActivationManager, ActivationManagerBuilder, ConditionList, OnFinalizeBlockDecision,
    OnPrepareProposalDecision, OnProcessProposalDecision, VOTE_RETENTION_PERIOD,
};
use reth_db::models::activation_manager::{RuntimeVersion, Vote};
use reth_provider::ActivationManagerReaderWriter;
use secp256k1::{generate_keypair, rand::thread_rng};

/// Index for the ALICE validator in the test fixture
pub(super) const ALICE: usize = 0;
/// Index for the BOB validator in the test fixture
pub(super) const BOB: usize = 1;
/// Index for the EVE validator in the test fixture
pub(super) const EVE: usize = 2;

// Runtime version constants for tests
/// The default active runtime version used in tests (1.0)
pub(super) const ACTIVE_VERSION: RuntimeVersion = RuntimeVersion::new(1, 0);
/// The default upgrade runtime version used in tests (2.0)
pub(super) const UPGRADE_VERSION: RuntimeVersion = RuntimeVersion::new(2, 0);
/// An alternative version used for testing mismatched version scenarios (3.0)
pub(super) const INVALID_VERSION: RuntimeVersion = RuntimeVersion::new(3, 0);

/// Stores vote details for a validator in the test environment.
///
/// This structure holds information about a validator's vote on an upgrade
/// proposal, including which version they're voting on, their vote (Aye/Nay),
/// and whether they're ready to accept the upgrade.
struct VoteDetails {
    /// The runtime version that the validator is voting on
    version: RuntimeVersion,
    /// The validator's vote (Aye/Nay)
    vote: Vote,
    /// Whether the validator is ready to accept the upgrade
    is_compliant: bool,
}

/// Main test fixture for network upgrade scenarios.
///
/// This fixture manages the state for multiple validators in upgrade testing
/// scenarios, including their identities, databases, activation managers,
/// and voting status. It provides methods to configure validators with
/// different upgrade behaviors and to advance the simulation through blocks.
pub(super) struct UpgradeTestFixture {
    /// The block height at which the upgrade should activate (if conditions are met)
    upgrade_height: u64,
    /// The percentage approval rate (0-100) required for upgrade activation
    required_approval_rate: usize,
    /// The minimum number of distinct validators required to participate in voting
    min_validator_count: usize,

    /// The current block height in the simulation
    block_height: u64,
    /// How long votes are retained before expiring (in blocks)
    vote_retention_period: u64,

    /// The public keys representing each validator's identity
    addrs: [secp256k1::PublicKey; 3],
    /// In-memory databases for each validator
    dbs: [Db; 3],
    /// The activation manager instances for each validator
    managers: [Option<ActivationManager<Db>>; 3],
    /// Expected votes to be included in block proposals for each validator
    expected_votes: [Option<VoteDetails>; 3],
}

impl UpgradeTestFixture {
    /// Creates a new `UpgradeTestFixture` with the specified upgrade parameters.
    ///
    /// Initializes a test environment with three validators (ALICE, BOB, EVE) and
    /// configures the upgrade parameters to be used throughout the tests.
    ///
    /// # Parameters
    /// * `upgrade_height` - The block height at which the upgrade should activate
    /// * `approval_rate` - The percentage approval rate (0-100) required for upgrade activation
    /// * `min_validator_count` - The minimum number of validators that must participate
    ///
    /// # Returns
    /// * A new `UpgradeTestFixture` with default values and generated validator keys
    pub(super) fn new(upgrade_height: u64, approval_rate: usize) -> Self {
        // Generate keypairs
        let (_, alice_addr) = generate_keypair(&mut thread_rng());
        let (_, bob_addr) = generate_keypair(&mut thread_rng());
        let (_, eve_addr) = generate_keypair(&mut thread_rng());

        Self {
            upgrade_height,
            required_approval_rate: approval_rate,
            min_validator_count: 3,
            block_height: 0,
            vote_retention_period: VOTE_RETENTION_PERIOD,
            addrs: [alice_addr, bob_addr, eve_addr],
            dbs: [Db::new(), Db::new(), Db::new()],
            managers: [None, None, None],
            expected_votes: [None, None, None],
        }
    }

    /// Sets a custom vote retention period for the test.
    ///
    /// Allows configuring how long votes are retained before expiring during the test.
    /// This is useful for testing vote expiration scenarios with a shorter period.
    ///
    /// # Parameters
    /// * `vote_retention_period` - How long votes are retained (in blocks)
    ///
    /// # Returns
    /// * The fixture with the updated vote retention period
    pub(super) fn vote_retention_period(mut self, vote_retention_period: u64) -> Self {
        self.vote_retention_period = vote_retention_period;
        self
    }

    /// Configures a validator to ignore network upgrades.
    ///
    /// Sets up the specified validator to not participate in upgrade voting and
    /// to reject any blocks with versions other than the current active version.
    ///
    /// # Parameters
    /// * `validator` - The index of the validator to configure (ALICE, BOB, or EVE)
    ///
    /// # Returns
    /// * The fixture with the configured validator
    pub(super) fn setup_ignoring_validator(mut self, validator: usize) -> Self {
        let db = self.dbs[validator].clone();

        let manager = ActivationManagerBuilder::new(db, ACTIVE_VERSION)
            .vote_retention_period(self.vote_retention_period)
            .build_ignore_nework_upgrade();

        self.managers[validator] = Some(manager);
        self.expected_votes[validator] = None;

        self
    }

    /// Configures a validator to signal a vote without accepting the upgrade.
    ///
    /// Sets up the specified validator to include their vote in block proposals
    /// but to reject blocks with the upgraded version.
    ///
    /// # Parameters
    /// * `validator` - The index of the validator to configure (ALICE, BOB, or EVE)
    /// * `vote` - The validator's vote (Aye/Nay)
    ///
    /// # Returns
    /// * The fixture with the configured validator
    pub(super) fn setup_signaling_validator(mut self, validator: usize, vote: Vote) -> Self {
        let db = self.dbs[validator].clone();

        let manager = ActivationManagerBuilder::new(db, ACTIVE_VERSION)
            .vote_retention_period(self.vote_retention_period)
            .build_signal_network_upgrade(UPGRADE_VERSION, vote);

        self.managers[validator] = Some(manager);
        self.expected_votes[validator] =
            Some(VoteDetails { version: UPGRADE_VERSION, vote, is_compliant: false });

        self
    }

    /// Configures a validator to accept the standard upgrade version.
    ///
    /// Sets up the specified validator to both vote for and accept the upgrade
    /// when all conditions are met.
    ///
    /// # Parameters
    /// * `validator` - The index of the validator to configure (ALICE, BOB, or EVE)
    /// * `vote` - The validator's vote (Aye/Nay)
    ///
    /// # Returns
    /// * The fixture with the configured validator
    pub(super) fn setup_compliant_validator(self, validator: usize, vote: Vote) -> Self {
        self._do_setup_compliant_validator(validator, vote, UPGRADE_VERSION)
    }

    /// Configures a validator to accept an invalid upgrade version.
    ///
    /// Sets up the specified validator to both vote for and accept an alternative
    /// upgrade version (3.0). This is used to test how validators handle mismatched
    /// upgrade versions.
    ///
    /// # Parameters
    /// * `validator` - The index of the validator to configure (ALICE, BOB, or EVE)
    /// * `vote` - The validator's vote (Aye/Nay)
    ///
    /// # Returns
    /// * The fixture with the configured validator
    #[allow(non_snake_case)]
    pub(super) fn setup_INVALID_compliant_validator(self, validator: usize, vote: Vote) -> Self {
        self._do_setup_compliant_validator(validator, vote, INVALID_VERSION)
    }

    /// Internal method to set up a validator to accept a specific upgrade version.
    ///
    /// Configures the validator with the specified parameters and creates an
    /// ActivationManager that will accept the upgrade when conditions are met.
    ///
    /// # Parameters
    /// * `validator` - The index of the validator to configure
    /// * `vote` - The validator's vote (Aye/Nay)
    /// * `upgrade_version` - The version to track and accept
    ///
    /// # Returns
    /// * The fixture with the configured validator
    fn _do_setup_compliant_validator(
        mut self,
        validator: usize,
        vote: Vote,
        upgrade_version: RuntimeVersion,
    ) -> Self {
        let db = self.dbs[validator].clone();

        let manager = ActivationManagerBuilder::new(db, ACTIVE_VERSION)
            .vote_retention_period(self.vote_retention_period)
            .build_COMPLIANT_network_upgrade(
                upgrade_version,
                self.required_approval_rate,
                self.min_validator_count,
                self.upgrade_height,
                Some(vote),
            );

        self.managers[validator] = Some(manager);
        self.expected_votes[validator] =
            Some(VoteDetails { version: upgrade_version, vote, is_compliant: true });

        self
    }

    /// Returns the current block height in the test simulation.
    ///
    /// # Returns
    /// * The current block height
    pub(super) fn next_height(&self) -> u64 {
        self.block_height
    }

    /// Starts a block proposal process for testing.
    ///
    /// Creates a ProposingTestFixture that will simulate the process of a validator
    /// proposing a block with the specified version.
    ///
    /// # Parameters
    /// * `validator` - The index of the validator who will propose the block
    /// * `version` - The runtime version to propose for the block
    ///
    /// # Returns
    /// * A ProposingTestFixture for building and testing the block
    pub(super) fn start_proposal(
        &mut self,
        validator: usize,
        version: RuntimeVersion,
    ) -> ProposingTestFixture<'_> {
        ProposingTestFixture {
            i: self,
            proposer_index: validator,
            expected_proposal_version: version,
            expectations: [None, None, None],
            expected_conditions: [None, None, None],
        }
    }
}

/// Expectations for a validator's behavior during block processing.
///
/// This structure defines the expected outcomes when a validator processes
/// a block proposal, including whether they should accept or reject it,
/// and the expected voting statistics after processing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Expectations {
    /// Whether the validator should accept the block during `process_proposal`
    pub(super) process_pass: bool,
    /// Whether the validator should accept the block during `finalize_block`
    pub(super) finalize_pass: bool,
    /// The expected percentage (0-100) of Aye votes
    pub(super) aye_approval_rate: usize,
    /// The expected percentage (0-100) of compliant validators
    pub(super) compliance_approval_rate: usize,
}

/// Fixture for testing the block proposal and validation flow.
///
/// This structure builds on the main UpgradeTestFixture to test specific
/// block proposal scenarios. It allows setting expectations for each validator's
/// behavior and conducts the simulation of proposing, processing, and
/// finalizing blocks.
pub(super) struct ProposingTestFixture<'a> {
    i: &'a mut UpgradeTestFixture,

    /// The index of the validator proposing the current block
    proposer_index: usize,
    /// The runtime version expected in the proposal
    expected_proposal_version: RuntimeVersion,

    /// Expectations for each validator's behavior when processing the block
    expectations: [Option<Expectations>; 3],
    /// Expected upgrade conditions for each validator
    expected_conditions: [Option<ConditionList>; 3],
}

impl ProposingTestFixture<'_> {
    /// Sets the expected upgrade conditions for specified validators.
    ///
    /// Configures the test to expect the specified conditions when validators
    /// process the proposed block.
    ///
    /// # Parameters
    /// * `vals` - Array of validator indices to configure expectations for
    /// * `conditions` - The conditions to expect
    ///
    /// # Returns
    /// * The fixture with updated expectations
    pub(super) fn upgrade_conditions(mut self, vals: &[usize], conditions: ConditionList) -> Self {
        for v in vals {
            self.expected_conditions[*v] = Some(conditions);
        }
        self
    }

    /// Clears the expected upgrade conditions for specified validators.
    ///
    /// Configures the test to expect no upgrade conditions when validators
    /// process the proposed block.
    ///
    /// # Parameters
    /// * `vals` - Array of validator indices to clear expectations for
    ///
    /// # Returns
    /// * The fixture with updated expectations
    pub(super) fn upgrade_conditions_empty(mut self, vals: &[usize]) -> Self {
        for v in vals {
            self.expected_conditions[*v] = None;
        }
        self
    }

    /// Sets the behavior expectations for specified validators.
    ///
    /// Configures the test to expect certain behaviors and outcomes when
    /// validators process the proposed block.
    ///
    /// # Parameters
    /// * `vals` - Array of validator indices to configure expectations for
    /// * `expectations` - The behavior and outcomes to expect
    ///
    /// # Returns
    /// * The fixture with updated expectations
    pub(super) fn expectations(mut self, vals: &[usize], expectations: Expectations) -> Self {
        for v in vals {
            self.expectations[*v] = Some(expectations);
        }
        self
    }

    /// Clears the behavior expectations for specified validators.
    ///
    /// Configures the test to expect no specific behaviors when validators
    /// process the proposed block.
    ///
    /// # Parameters
    /// * `vals` - Array of validator indices to clear expectations for
    ///
    /// # Returns
    /// * The fixture with updated expectations
    pub(super) fn expectations_empty(mut self, vals: &[usize]) -> Self {
        for v in vals {
            self.expectations[*v] = None;
        }
        self
    }

    /// Builds a single block and advances the test simulation.
    ///
    /// Simulates the full process of proposing, processing, and finalizing a block
    /// and verifies that all validators behave according to expectations.
    pub(super) fn build_block(mut self) {
        self.do_build_block();
    }

    /// Builds multiple blocks until reaching the target height.
    ///
    /// Repeatedly builds blocks and advances the simulation until the
    /// specified block height is reached.
    ///
    /// # Parameters
    /// * `target_height` - The block height to build up to
    pub(super) fn build_blocks_until(mut self, target_height: u64) {
        while self.i.block_height < target_height {
            self.do_build_block();
        }
    }

    /// Internal method to simulate the full block building process.
    ///
    /// This method:
    /// 1. Has the proposer prepare a block proposal
    /// 2. Has each validator process the proposal
    /// 3. Has each validator finalize the block
    /// 4. Verifies that all validators behave according to expectations
    /// 5. Advances the block height
    fn do_build_block(&mut self) {
        dbg!(self.i.block_height);

        let proposer = self.i.managers[self.proposer_index].as_mut().unwrap();
        let proposer_addr = &self.i.addrs[self.proposer_index];

        // Proposer builds the block
        let OnPrepareProposalDecision {
            version: proposer_version,
            vote: proposer_vote,
            conditions: proposer_conditions,
        } = proposer.on_prepare_proposal(self.i.block_height).unwrap();

        // Verify proposed version
        assert_eq!(proposer_version, self.expected_proposal_version, "proposed different version");

        // Verify proposer's vote
        if let Some(p) = &proposer_vote {
            let e = self.i.expected_votes[self.proposer_index].as_ref().unwrap();
            assert_eq!(p.version, e.version, "voted for different version");
            assert_eq!(p.vote, e.vote, "voted with different vote");
            assert_eq!(p.is_compliant, e.is_compliant, "voted with different compliance");
        }

        // Verify proposer's conditions
        let expected_cond = self.expected_conditions[self.proposer_index].as_ref();
        assert_eq!(
            proposer_conditions.as_ref(),
            expected_cond,
            "proposed with different conditions"
        );

        // Each party processes and finalizes the block
        for (i, (manager, db)) in self.i.managers.iter_mut().zip(self.i.dbs.iter()).enumerate() {
            let auth_idx = i;
            dbg!(auth_idx);

            let Some(manager) = manager.as_mut() else {
                assert!(
                    self.expectations[i].is_none(),
                    "expected expectations for non-existent manager"
                );
                assert!(
                    self.expected_conditions[i].is_none(),
                    "expected conditions for non-existent manager"
                );

                continue;
            };

            let expect = self.expectations[i].as_ref().expect("expected expectations for manager");
            let expected_cond = self.expected_conditions[i].as_ref();

            // COMET-BFT: Process the proposal.
            let decision =
                manager.on_process_proposal(self.i.block_height, proposer_version).unwrap();

            match decision {
                OnProcessProposalDecision::Process { version, conditions } => {
                    assert!(expect.process_pass, "expected process pass");
                    assert_eq!(version, proposer_version, "processed different version");
                    assert_eq!(
                        conditions.as_ref(),
                        expected_cond,
                        "processed with different conditions"
                    );
                }
                OnProcessProposalDecision::RejectBlock { version, conditions } => {
                    assert!(!expect.process_pass, "expected process fail");
                    assert_eq!(version, proposer_version, "processed different version");
                    assert_eq!(
                        conditions.as_ref(),
                        expected_cond,
                        "processed with different conditions"
                    );
                }
            }

            // COMET-BFT: Finalize the proposal.
            let decision = manager
                .on_finalize_block(
                    proposer_version,
                    self.i.block_height,
                    *proposer_addr,
                    proposer_vote.clone(),
                )
                .unwrap();

            match decision {
                OnFinalizeBlockDecision::Finalize { version } => {
                    assert!(expect.finalize_pass, "expected finalize pass");
                    assert_eq!(version, proposer_version, "finalized different version");
                }
                OnFinalizeBlockDecision::RejectBlockDeadEnd { version } => {
                    assert!(!expect.finalize_pass, "expected finalize fail");
                    assert_eq!(version, proposer_version, "(none-)finalized different version");
                }
            }

            // Verify approval rates
            let (ayes, votes) =
                db.get_upgrading_approval_rate_ayes(self.i.min_validator_count).unwrap();

            let (compliance, v2) =
                db.get_upgrading_approval_rate_compliance(self.i.min_validator_count).unwrap();

            assert_eq!(votes, v2);
            assert!(votes >= self.i.min_validator_count, "voter count does not meet minimum");

            assert_eq!(ayes, expect.aye_approval_rate, "ayes approval_rate mismatch");
            assert_eq!(
                compliance, expect.compliance_approval_rate,
                "compliant approval_rate mismatch"
            );
        }

        self.i.block_height += 1;
    }
}
