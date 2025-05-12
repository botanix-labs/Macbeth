use bitcoin::secp256k1;
use encryption::{DkgHandshakeManager, KeyVerificationManager, SecureChannelManager};
use frost::keys::{
    dkg::{round1, round2},
    PublicKeyPackage,
};
use frost_secp256k1_tr::{self as frost, keys::KeyPackage};
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};
use thiserror::Error;

mod encryption;
#[cfg(test)]
mod tests;

// NOTE: As of now, the session context is always the same constant. In
// the future this may change, and might be handled differently.
pub const SESSION_CONTEXT: &[u8] = b"static-dkg-session-context";

/// Wrapper type for FROST identifiers used in the DKG protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Initiator(frost::Identifier);

#[cfg(test)]
impl From<frost::Identifier> for Initiator {
    fn from(id: frost::Identifier) -> Self {
        Initiator(id)
    }
}

/// Wrapper type for FROST identifiers used in the DKG protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Target(frost::Identifier);

#[cfg(test)]
impl From<frost::Identifier> for Target {
    fn from(id: frost::Identifier) -> Self {
        Target(id)
    }
}

mod sealed_pkg {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct SealedRoundOnePackage(round1::Package);

    impl SealedRoundOnePackage {
        pub fn new(
            package: round1::Package,
            auth: &mut DkgHandshakeManager,
        ) -> Result<(Self, secp256k1::PublicKey, secp256k1::ecdsa::Signature), encryption::Error>
        {
            let (eph_pub, sig) = auth.commit_round1(&package)?;

            Ok((SealedRoundOnePackage(package), eph_pub, sig))
        }
        pub fn extract(
            self,
            initiator: Initiator,
            eph_pub: secp256k1::PublicKey,
            signature: secp256k1::ecdsa::Signature,
            auth: &mut DkgHandshakeManager,
        ) -> Result<round1::Package, encryption::Error> {
            auth.validate_round1(initiator, eph_pub, signature, &self.0)?;

            Ok(self.0)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct SealedRoundThreeSignature(secp256k1::ecdsa::Signature);

    impl SealedRoundThreeSignature {
        pub fn new(
            package: frost::keys::PublicKeyPackage,
            auth: &mut KeyVerificationManager,
        ) -> Result<Self, encryption::Error> {
            let sig = auth.commit_round3(&package)?;
            Ok(SealedRoundThreeSignature(sig))
        }
        pub fn extract(
            self,
            initiator: Initiator,
            auth: &mut KeyVerificationManager,
        ) -> Result<secp256k1::ecdsa::Signature, encryption::Error> {
            auth.validate_round3(initiator, self.0)?;
            Ok(self.0)
        }
    }
}

/// A message payload for the DKG protocol that contains routing information
/// and the actual message content.
///
/// This struct is passed between participants in the DKG protocol and contains
/// addressing information (sender and recipient) along with the specific DKG message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DkgPayload {
    /// The FROST identifier of the sender
    pub sender: frost::Identifier,
    /// The FROST identifier of the intended recipient
    pub recipient: frost::Identifier,
    /// The actual DKG message content
    pub msg: DkgMessage,
}

/// Represents the different types of messages exchanged during the DKG
/// protocol.
///
/// Each variant corresponds to a specific stage of the DKG protocol, either
/// sending cryptographic packages or acknowledging receipt of packages.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DkgMessage {
    /// A round1 message containing the sender's FROST round1 package.
    Round1 {
        /// The original initiator of this package
        initiator: Initiator,
        // The session context, dictated by the coordinator
        context: Vec<u8>,
        // The session nonce, dictated by the coordinator
        nonce: u64,
        /// The ephemeral public key
        ephemeral_pub: secp256k1::PublicKey,
        /// The signature of the round1 package
        signature: secp256k1::ecdsa::Signature,
        /// The round1 package containing commitments to secret values
        package: sealed_pkg::SealedRoundOnePackage,
    },
    /// Acknowledges receipt of a round1 message.
    AckRound1 {
        /// The initiator whose round1 message is being acknowledged
        initiator: Initiator,
    },
    /// A round2 message containing a FROST round2 package. This message
    /// includes a target identifier since round2 packages are specifically
    /// generated for each participant.
    Round2 {
        /// The original initiator of this package
        initiator: Initiator,
        /// The intended target of this package
        target: Target,
        /// The nonce used for encryption
        nonce: u64,
        /// The encrypted round2 package
        package: Vec<u8>,
    },
    /// Acknowledges receipt of a round2 message.
    AckRound2 {
        /// The initiator whose round2 message is being acknowledged
        initiator: Initiator,
        /// The target of the round2 message being acknowledged
        target: Target,
    },
    /// A round3 message containing the final, aggregated public key package.
    Round3 {
        /// The original initiator of this package
        initiator: Initiator,
        /// The signature of the aggregated round3 package
        signature: sealed_pkg::SealedRoundThreeSignature,
    },
    /// Acknowledges receipt of a round3 message.
    AckRound3 {
        /// The initiator whose round3 message is being acknowledged
        initiator: Initiator,
    },
}

/// Configuration parameters for the DKG state machine.
///
/// This struct defines the key parameters for a threshold signing scheme
/// and timeout durations for the different rounds of the DKG protocol.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The maximum number of signers (participants) in the threshold scheme.
    /// This is typically equal to the total number of participants.
    pub max_signers: u16,

    /// The minimum number of signers required to produce a valid signature.
    pub min_signers: u16,

    /// The timeout duration for round1 packages. If an acknowledgment isn't
    /// received within this duration, the package will be resent.
    pub round1_package_timeout: Duration,

    /// The timeout duration for round2 packages. If an acknowledgment isn't
    /// received within this duration, the package will be resent.
    pub round2_package_timeout: Duration,

    /// The timeout duration for round3 packages. If an acknowledgment isn't
    /// received within this duration, the package will be resent.
    pub round3_package_timeout: Duration,

    /// The optional timeout duration for the entire DKG session. If the session
    /// isn't completed within this duration, it will be reset. This increments
    /// the session nonce and creates new round1 packages.
    pub pending_session_timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Represents the current stage of the DKG protocol.
pub enum Stage {
    /// Waiting for the coordinator to initialize the DKG session.
    AwaitingInit,
    /// Active exchange of round1 packages between participants.
    RoundOne,
    /// Active exchange of round2 packages between participants.
    RoundTwo,
    /// Active exchange of round3 packages (aggregated public keys).
    RoundThree,
    /// The DKG process was aborted.
    Aborted,
    /// DKG protocol finalized successfully.
    Finalized,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stage::AwaitingInit => write!(f, "Awaiting-Initialization"),
            Stage::RoundOne => write!(f, "Round-One"),
            Stage::RoundTwo => write!(f, "Round-Two"),
            Stage::RoundThree => write!(f, "Round-Three"),
            Stage::Aborted => write!(f, "Aborted"),
            Stage::Finalized => write!(f, "Finalized"),
        }
    }
}

#[derive(Debug, Clone)]
struct OutEntryRoundOne {
    package: sealed_pkg::SealedRoundOnePackage,
    ephemeral_pub: secp256k1::PublicKey,
    signature: secp256k1::ecdsa::Signature,
    timer: Option<Instant>,
    attempts: usize,
}

#[derive(Debug, Clone)]
struct OutEntryRoundTwo {
    ciphernonce: u64,
    ciphertext: Vec<u8>,
    timer: Option<Instant>,
    attempts: usize,
}

#[derive(Debug, Clone)]
struct OutEntryRoundThree {
    signature: sealed_pkg::SealedRoundThreeSignature,
    timer: Option<Instant>,
    attempts: usize,
}

#[derive(Debug)]
/// Represents the current state of the DKG process.
enum StageState {
    /// The DKG process is waiting for the coordinator to initialize the session.
    AwatingInit,
    /// The active stage of round1. Participants are exchanging round1 packages
    /// with each other. The coordinator distributes packages between all
    /// participants.
    RoundOne {
        context: Vec<u8>,
        nonce: u64,
        auth: DkgHandshakeManager,
        secret_package: round1::SecretPackage,
        in_round1_packages: BTreeMap<Initiator, round1::Package>,
        out_round1_packages: BTreeMap<(Initiator, frost::Identifier), Option<OutEntryRoundOne>>,
    },
    /// The active stage of round2. Participants are exchanging round2 packages
    /// with each other. The coordinator distributes packages between all
    /// participants and tracks which packages have been distributed through the
    /// dist_checklist.
    RoundTwo {
        pending: bool,
        auth: SecureChannelManager,
        secret_package: round2::SecretPackage,
        in_round1_packages: BTreeMap<frost::Identifier, round1::Package>,
        in_round2_packages: BTreeMap<Initiator, round2::Package>,
        out_round2_packages: BTreeMap<(Initiator, Target), Option<OutEntryRoundTwo>>,
    },
    /// The active stage of round3. Participants are exchanging aggregated
    /// public key packages to ensure everyone has the same final, aggregated
    /// public key.
    RoundThree {
        pending: bool,
        auth: KeyVerificationManager,
        secret_package: frost::keys::KeyPackage,
        public_key_package: PublicKeyPackage,
        in_round1_packages: BTreeMap<frost::Identifier, round1::Package>,
        in_round2_packages: BTreeMap<frost::Identifier, round2::Package>,
        in_round3_packages: BTreeMap<Initiator, secp256k1::ecdsa::Signature>,
        out_round3_packages: BTreeMap<(Initiator, frost::Identifier), Option<OutEntryRoundThree>>,
    },
    /// The DKG process was aborted. This can happen if FROST fails to generate
    /// new rounds, for example if a peer provided an incorrect or malformed
    /// package.
    Aborted,
    /// The DKG process completed successfully. All participants have verified
    /// they have the same public key package.
    Finalized {
        secret_package: frost::keys::KeyPackage,
        public_key_package: PublicKeyPackage,
    },
}

impl StageState {
    fn did_round_one_finalize(&self) -> bool {
        matches!(
            self,
            StageState::RoundTwo { .. } |
                StageState::RoundThree { .. } |
                StageState::Finalized { .. }
        )
    }
    fn did_round_two_finalize(&self) -> bool {
        matches!(self, StageState::RoundThree { .. } | StageState::Finalized { .. })
    }
    fn did_round_three_finalize(&self) -> bool {
        matches!(self, StageState::Finalized { .. })
    }
}

/// A queue for outgoing DKG payloads.
struct Queue {
    i: VecDeque<DkgPayload>,
    my_frost_id: frost::Identifier,
}

impl Queue {
    fn send_round1_ack(&mut self, initiator: Initiator, recipient: frost::Identifier) {
        let msg = DkgPayload {
            sender: self.my_frost_id,
            recipient,
            msg: DkgMessage::AckRound1 { initiator },
        };

        self.i.push_back(msg);
    }
    fn send_round2_ack(
        &mut self,
        initiator: Initiator,
        recipient: frost::Identifier,
        target: Target,
    ) {
        let msg = DkgPayload {
            sender: self.my_frost_id,
            recipient,
            msg: DkgMessage::AckRound2 { initiator, target },
        };

        self.i.push_back(msg);
    }

    fn send_round3_ack(&mut self, initiator: Initiator, recipient: frost::Identifier) {
        let msg = DkgPayload {
            sender: self.my_frost_id,
            recipient,
            msg: DkgMessage::AckRound3 { initiator },
        };

        self.i.push_back(msg);
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Bad DKG configuration: {0}")]
    BadConfig(String),
    #[error("Frost error: {0}")]
    Frost(#[from] frost::Error),
    #[error("Encryption error: {0}")]
    Encryption(#[from] encryption::Error),
}

/// The DKG state machine handles distributed key generation across multiple participants.
pub struct DkgStateMachine {
    config: Config,
    my_frost_id: frost::Identifier,
    my_static_sec: secp256k1::SecretKey,
    coordinator: frost::Identifier,
    members: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
    queue: Queue,
    state: StageState,
    session_nonce: Option<u64>,
    session_activated: Option<Instant>,
}

impl DkgStateMachine {
    /// Creates a new DKG state machine.
    ///
    /// This constructor initializes a new DKG state machine with the specified
    /// participant identifiers and configuration parameters.
    ///
    /// When created, the state machine behavior differs based on whether this participant
    /// is the coordinator:
    ///
    /// - If this participant is the coordinator (`my_frost_id == coordinator`), the state machine
    ///   enters the `RoundOne` stage by immediately sending the round1 packages to all other
    ///   participants. These initial packages can be retrieved by calling `send()`.
    ///
    /// - If this participant is not the coordinator, the state machine sets the `pending` flag to
    ///   `true`. It generates its round1 package but doesn't send it yet. Instead, it waits to
    ///   receive the coordinator's round1 package before becoming active and sending any messages.
    ///
    /// # Arguments
    ///
    /// * `my_frost_id` - The FROST identifier of this participant.
    /// * `coordinator` - The FROST identifier of the designated coordinator.
    /// * `members` - A list of all participant FROST identifiers in the DKG process, including this
    ///   participant and the coordinator.
    /// * `config` - Configuration parameters for the DKG process.
    /// * `session_nonce` - An increasing value that uniquely identifies this DKG session. Only the
    ///   coordinator should provide this value (typically using Unix time in seconds).
    ///   Non-coordinators should pass `None` and will accept the nonce from the coordinator's
    ///   initial message. The nonce increases automatically when sessions timeout and restart.
    pub fn new(
        my_frost_id: frost::Identifier,
        my_static_sec: secp256k1::SecretKey,
        coordinator: frost::Identifier,
        members: BTreeMap<frost::Identifier, secp256k1::PublicKey>,
        config: Config,
        session_nonce: Option<u64>,
    ) -> Result<Self, Error> {
        if !members.contains_key(&my_frost_id) {
            return Err(Error::BadConfig("my_frost_id not in members".to_string()));
        }

        if !members.contains_key(&coordinator) {
            return Err(Error::BadConfig("coordinator not in members".to_string()));
        }

        if config.min_signers > config.max_signers {
            return Err(Error::BadConfig(
                "min_signers cannot be greater than max_signers".to_string(),
            ));
        }

        if (members.len() as u16) != config.max_signers {
            return Err(Error::BadConfig("max_signers does not match member size".to_string()));
        }

        if my_frost_id == coordinator && session_nonce.is_none() {
            return Err(Error::BadConfig("coordinator must provide a session nonce".to_string()));
        }

        // Setup sending queue.
        let queue = Queue { i: VecDeque::new(), my_frost_id };

        let mut this = Self {
            config,
            my_frost_id,
            my_static_sec,
            coordinator,
            members,
            queue,
            state: StageState::AwatingInit,
            session_nonce: None,
            session_activated: None,
        };

        // The coordinator immediately starts the DKG process by sending a
        // `DkgMessage::NewSession` followed by the actual `DkgMessage::Round1`
        // package to each participant.
        //
        // Non-coordinators wait for the coordinator to send this message, and
        // initialize the round1 package and authentication layer then.
        if this.is_coordinator() {
            let nonce = session_nonce.expect("session nonce must be provided");
            this.init_new_session(nonce)?;

            debug_assert_eq!(this.stage(), Stage::RoundOne);
        }

        Ok(this)
    }
    /// Initializes a new DKG session with the given nonce by resetting the
    /// stage, setting up a new authentication layer, generating a new round1
    /// package, and sending it to all participants.
    ///
    /// Only the coordinator must call this method.
    fn init_new_session(&mut self, nonce: u64) -> Result<(), Error> {
        debug_assert!(self.is_coordinator());

        let context = SESSION_CONTEXT.to_vec();
        //
        self.session_activated = None;
        self.session_nonce = Some(nonce);
        self.queue.i.clear();

        // Generate the secret package and our round1 package
        let (secret_package, our_round1_package) = frost::keys::dkg::part1(
            self.my_frost_id,
            self.config.max_signers,
            self.config.min_signers,
            thread_rng(),
        )?;

        // AUTHENTICATION: Setup authentication and encryption layer.
        let mut auth = DkgHandshakeManager::new(
            &context,
            nonce,
            self.my_frost_id,
            self.my_static_sec,
            self.members.clone(),
        )?;

        // Seal our round1 package.
        let (sealed_round1_package, our_eph_pub, our_sig) =
            sealed_pkg::SealedRoundOnePackage::new(our_round1_package, &mut auth)?;

        let out_entry = OutEntryRoundOne {
            package: sealed_round1_package.clone(),
            ephemeral_pub: our_eph_pub,
            signature: our_sig,
            timer: None,
            attempts: 0,
        };

        let mut out_round1_packages = BTreeMap::new();

        // Prepare all outgoing round1 package entries that we need to have
        // acknowledged, including forwarded messages.
        //
        // For example; with three participants Alice (us), Bob, and Eve, we construct:
        // * Alice -> Bob
        // * Alice -> Eve
        // * Bob -> Eve (forwarded)
        // * Eve -> Bob (forwarded)
        for initiator in self.members.keys().copied() {
            for recipient in self.members.keys().copied() {
                // Skip ourself.
                if recipient == self.my_frost_id {
                    continue;
                }

                if initiator == recipient {
                    continue;
                }

                // Only set our packages; forwarded packages are set once
                // they're received, of course.
                let out_entry =
                    if initiator == self.my_frost_id { Some(out_entry.clone()) } else { None };

                // Track outgoing package.
                out_round1_packages.insert((Initiator(initiator), recipient), out_entry);

                if initiator != self.my_frost_id {
                    // Skip sending unless it's us.
                    continue;
                }

                // Push the round1 payload to the queue.
                let msg = DkgPayload {
                    sender: self.my_frost_id,
                    recipient,
                    msg: DkgMessage::Round1 {
                        initiator: Initiator(self.my_frost_id),
                        context: context.clone(),
                        nonce,
                        ephemeral_pub: our_eph_pub,
                        signature: our_sig,
                        package: sealed_round1_package.clone(),
                    },
                };

                self.queue.i.push_back(msg);
            }
        }

        self.state = StageState::RoundOne {
            nonce,
            auth,
            secret_package,
            in_round1_packages: BTreeMap::new(),
            out_round1_packages,
            context,
        };

        Ok(())
    }
    /// Returns the FROST identifier of this participant.
    pub fn frost_id(&self) -> frost::Identifier {
        self.my_frost_id
    }
    /// Checks if this participant is the coordinator of the DKG process.
    pub fn is_coordinator(&self) -> bool {
        self.my_frost_id == self.coordinator
    }
    /// Returns the current stage of the DKG protocol.
    pub fn stage(&self) -> Stage {
        match &self.state {
            StageState::AwatingInit => Stage::AwaitingInit,
            StageState::RoundOne { .. } => Stage::RoundOne,
            StageState::RoundTwo { .. } => Stage::RoundTwo,
            StageState::RoundThree { .. } => Stage::RoundThree,
            StageState::Finalized { .. } => Stage::Finalized,
            StageState::Aborted => Stage::Aborted,
        }
    }
    /// Returns the session nonce, if available.
    pub fn session_nonce(&self) -> Option<u64> {
        self.session_nonce
    }
    /// Returns the final, aggregated key packages if the DKG process has completed successfully.
    pub fn aggregate_key_packages(&self) -> Option<(&KeyPackage, &PublicKeyPackage)> {
        if let StageState::Finalized { secret_package, public_key_package, .. } = &self.state {
            Some((secret_package, public_key_package))
        } else {
            None
        }
    }
    /// Returns the duration until the next timeout event, if any.
    ///
    /// This method should be called after any state-changing operation,
    /// particularly after calling `send()`. The returned duration indicates how
    /// long to wait before calling `on_timeout()`.
    ///
    /// # Arguments
    ///
    /// * `now` - The current time
    ///
    /// # Returns
    ///
    /// An optional `Duration` until the next timeout event. If `None`, there
    /// are no pending timeout events.
    pub fn timeout(&self, now: Instant) -> Option<Duration> {
        let mut session_timeout = None;

        // If we're the coordinator and the DKG session has not been finalized...
        if self.is_coordinator() && self.stage() != Stage::Finalized {
            // And if a max session timeout has been configured...
            if let Some(max) = self.config.pending_session_timeout {
                // And if a DKG session has started...
                if let Some(session_activated) = self.session_activated {
                    let t = (session_activated + max).saturating_duration_since(now);
                    session_timeout = Some(t);
                }
            }
        }

        let t = match &self.state {
            StageState::RoundOne { out_round1_packages, .. } => out_round1_packages
                .values()
                .filter_map(|e| e.as_ref())
                .filter_map(|e| e.timer)
                .map(|t| t.saturating_duration_since(now))
                .min(),
            StageState::RoundTwo { out_round2_packages, .. } => out_round2_packages
                .values()
                .filter_map(|e| e.as_ref())
                .filter_map(|e| e.timer)
                .map(|t| t.saturating_duration_since(now))
                .min(),
            StageState::RoundThree { out_round3_packages, .. } => out_round3_packages
                .values()
                .filter_map(|e| e.as_ref())
                .filter_map(|e| e.timer)
                .map(|t| t.saturating_duration_since(now))
                .min(),
            _ => None,
        };

        match (t, session_timeout) {
            (Some(t), Some(s)) => Some(t.min(s)),
            (Some(t), None) => Some(t),
            (None, Some(x)) => Some(x),
            (None, None) => None,
        }
    }
    /// Processes timeout events for outgoing messages.
    ///
    /// This method should be called when the duration returned by `timeout()`
    /// expires. It will trigger re-sending of any messages that haven't been
    /// acknowledged. After calling this method, you should call `send()` in a
    /// loop to get any payloads that need to be re-sent.
    ///
    /// # Arguments
    ///
    /// * `now` - The current time
    pub fn on_timeout(&mut self, now: Instant) {
        let self_is_coordinator = self.is_coordinator();

        // If we're the coordinator and the DKG session has not been finalized...
        if self_is_coordinator && self.stage() != Stage::Finalized {
            // And if a max session timeout has been configured...
            if let Some(max) = self.config.pending_session_timeout {
                // And if a DKG session has started...
                if let Some(session_activated) = self.session_activated {
                    // And if the timeout has expired...
                    if now >= session_activated + max {
                        // Increment nonce.
                        let nonce = self.session_nonce.as_ref().expect("nonce tracker must be set");
                        let nonce = nonce.wrapping_add(1);

                        // Start a new session.
                        self.init_new_session(nonce).expect("failed to init new session");
                    }
                }
            }
        }

        match &mut self.state {
            StageState::RoundOne { context, nonce, out_round1_packages, .. } => {
                for ((initiator, recipient), entry) in out_round1_packages.iter() {
                    let Some(entry) = entry else {
                        // Package not available.
                        continue;
                    };

                    let Some(timer) = entry.timer else {
                        // No timer set.
                        continue;
                    };

                    // Check if the timer has expired, with a small tolerance
                    // adjustment.
                    if timer > now + Duration::from_millis(1) {
                        continue;
                    }

                    let msg = DkgPayload {
                        sender: self.my_frost_id,
                        recipient: *recipient,
                        msg: DkgMessage::Round1 {
                            initiator: *initiator,
                            context: context.clone(),
                            nonce: *nonce,
                            ephemeral_pub: entry.ephemeral_pub,
                            signature: entry.signature,
                            package: entry.package.clone(),
                        },
                    };

                    self.queue.i.push_back(msg);
                }
            }
            StageState::RoundTwo { out_round2_packages, .. } => {
                for ((initiator, target), entry) in out_round2_packages.iter() {
                    let Some(entry) = entry else {
                        // Package not available.
                        continue;
                    };

                    let Some(timer) = entry.timer else {
                        // No timer set.
                        continue;
                    };

                    // Check if the timer has expired, with a small tolerance
                    // adjustment.
                    if timer > now + Duration::from_millis(1) {
                        continue;
                    }

                    let recipient = if self_is_coordinator { target.0 } else { self.coordinator };

                    let msg = DkgPayload {
                        sender: self.my_frost_id,
                        recipient,
                        msg: DkgMessage::Round2 {
                            initiator: *initiator,
                            target: *target,
                            nonce: entry.ciphernonce,
                            package: entry.ciphertext.clone(),
                        },
                    };

                    self.queue.i.push_back(msg);
                }
            }
            StageState::RoundThree { out_round3_packages, .. } => {
                for ((initiator, recipient), entry) in out_round3_packages.iter() {
                    let Some(entry) = entry else {
                        // Package not available.
                        continue;
                    };

                    let Some(timer) = entry.timer else {
                        // No timer set.
                        continue;
                    };

                    // Check if the timer has expired, with a small tolerance
                    // adjustment.
                    if timer > now + Duration::from_millis(1) {
                        continue;
                    }

                    let msg = DkgPayload {
                        sender: self.my_frost_id,
                        recipient: *recipient,
                        msg: DkgMessage::Round3 {
                            initiator: *initiator,
                            signature: entry.signature.clone(),
                        },
                    };

                    self.queue.i.push_back(msg);
                }
            }
            _ => {}
        }
    }
    /// Returns the next outgoing payload, if any.
    ///
    /// This method should be called after any state changes, respectively after
    /// calling `recv()` or `on_timeout()`. It may return multiple payloads, so
    /// it should be called in a loop until it returns `None`.
    ///
    /// # Arguments
    ///
    /// * `now` - The current time, used for setting timeouts for outgoing messages
    ///
    /// # Returns
    ///
    /// An optional `DkgPayload` to be sent to another participant
    pub fn send(&mut self, now: Instant) -> Option<DkgPayload> {
        loop {
            let payload = self.queue.i.pop_front()?;

            match payload.msg {
                DkgMessage::Round1 { initiator, .. } => {
                    let StageState::RoundOne { out_round1_packages, .. } = &mut self.state else {
                        // Already expired.
                        continue;
                    };

                    let Some(Some(entry)) =
                        out_round1_packages.get_mut(&(initiator, payload.recipient))
                    else {
                        // Already expired or not available yet.
                        continue;
                    };

                    if self.session_activated.is_none() {
                        // Track the session start time.
                        self.session_activated = Some(now);
                    }

                    entry.timer = Some(now + self.config.round1_package_timeout);
                    entry.attempts += 1;
                }
                DkgMessage::Round2 { initiator, target, .. } => {
                    let StageState::RoundTwo { out_round2_packages, .. } = &mut self.state else {
                        // Already expired.
                        continue;
                    };

                    let Some(Some(entry)) = out_round2_packages.get_mut(&(initiator, target))
                    else {
                        // Already expired or not available yet.
                        continue;
                    };

                    entry.timer = Some(now + self.config.round2_package_timeout);
                    entry.attempts += 1;
                }
                DkgMessage::Round3 { initiator, .. } => {
                    let StageState::RoundThree { out_round3_packages, .. } = &mut self.state else {
                        // Already expired.
                        continue;
                    };

                    let Some(Some(entry)) =
                        out_round3_packages.get_mut(&(initiator, payload.recipient))
                    else {
                        // Already expired or not available yet.
                        continue;
                    };

                    entry.timer = Some(now + self.config.round3_package_timeout);
                    entry.attempts += 1;
                }
                _ => {
                    // Nothing to do.
                }
            }

            return Some(payload);
        }
    }
    /// Processes an incoming payload from another participant.
    ///
    /// This method updates the internal state based on the received payload.
    /// After calling this method, you should call `send()` in a loop to get any
    /// response payloads that need to be sent.
    ///
    /// # Arguments
    ///
    /// * `payload` - The incoming payload from another participant
    pub fn recv(&mut self, payload: DkgPayload) -> Result<(), Error> {
        if payload.recipient != self.my_frost_id {
            return Ok(());
        }

        let DkgPayload { sender, msg, .. } = payload;

        match msg {
            DkgMessage::Round1 {
                initiator,
                context,
                nonce,
                ephemeral_pub,
                signature,
                package,
                ..
            } => {
                #[rustfmt::skip]
                self.on_dkg_msg_round1(initiator, context, nonce, ephemeral_pub, signature, package, sender)?;
                self.transition_stage2_checked()?;
            }
            DkgMessage::AckRound1 { initiator } => {
                self.on_dkg_msg_ack_round1(initiator, sender)?;
                self.transition_stage2_checked()?;
            }
            DkgMessage::Round2 { initiator, target, nonce, package } => {
                self.on_dkg_msg_round2(initiator, target, nonce, package, sender)?;
                self.transition_stage3_checked()?;
            }
            DkgMessage::AckRound2 { initiator, target } => {
                self.on_dkg_msg_ack_round2(initiator, target)?;
                self.transition_stage3_checked()?;
            }
            DkgMessage::Round3 { initiator, signature } => {
                self.on_dkg_msg_round3(initiator, signature, sender)?;
                self.transition_final_checked()?;
            }
            DkgMessage::AckRound3 { initiator } => {
                self.on_dkg_msg_ack_round3(initiator, sender)?;
                self.transition_final_checked()?;
            }
        }

        Ok(())
    }
    fn on_dkg_msg_ack_round1(
        &mut self,
        initiator: Initiator,
        sender: frost::Identifier,
    ) -> Result<(), Error> {
        if !self.members.contains_key(&initiator.0) {
            return Ok(());
        }

        let StageState::RoundOne { out_round1_packages, .. } = &mut self.state else {
            // Ignore
            return Ok(());
        };

        out_round1_packages.remove(&(initiator, sender));

        Ok(())
    }
    fn on_dkg_msg_ack_round2(&mut self, initiator: Initiator, target: Target) -> Result<(), Error> {
        if !self.members.contains_key(&initiator.0) {
            return Ok(());
        }

        let StageState::RoundTwo { out_round2_packages, .. } = &mut self.state else {
            // Ignore
            return Ok(());
        };

        out_round2_packages.remove(&(initiator, target));

        Ok(())
    }
    fn on_dkg_msg_ack_round3(
        &mut self,
        initiator: Initiator,
        sender: frost::Identifier,
    ) -> Result<(), Error> {
        if !self.members.contains_key(&initiator.0) {
            return Ok(());
        }

        let StageState::RoundThree { out_round3_packages, .. } = &mut self.state else {
            // Ignore
            return Ok(());
        };

        out_round3_packages.remove(&(initiator, sender));

        Ok(())
    }
    /// Processes a new session initialization as request by the coordinator by
    /// validating the request, resetting the stage, setting up a new
    /// authentication layer, generating a new round1 package, and sending it to
    /// the coordinator. Returns `true` if the session was successfully
    /// initialized.
    ///
    /// Only non-coordinators should call this method.
    fn validate_new_session_init(
        &mut self,
        initiator: Initiator,
        their_context: Vec<u8>,
        their_nonce: u64,
        their_ephmeral_pub: secp256k1::PublicKey,
        their_signature: secp256k1::ecdsa::Signature,
        their_package: sealed_pkg::SealedRoundOnePackage,
    ) -> Result<bool, Error> {
        if self.is_coordinator() {
            return Ok(false);
        }

        if initiator.0 != self.coordinator {
            return Ok(false);
        }

        if their_context != SESSION_CONTEXT {
            return Ok(false);
        }

        if let Some(last) = self.session_nonce {
            if last >= their_nonce {
                // Outdated or active nonce, ignore.
                return Ok(false);
            }
        }

        // Cannot start a new session if the DKG process has already been
        // finalized.
        if self.stage() == Stage::Finalized {
            return Ok(false);
        }

        // AUTHENTICATION: Setup new session parameters as-is. We then validate
        // the coordinators' round1 package, which is inherently tied to those
        // parameters.
        let mut auth = DkgHandshakeManager::new(
            &their_context,
            their_nonce,
            self.my_frost_id,
            self.my_static_sec,
            self.members.clone(),
        )?;

        let their_package =
            their_package.extract(initiator, their_ephmeral_pub, their_signature, &mut auth)?;

        // Verification succeeded, the new session is accepted!

        // Set new session params.
        let context = their_context;
        let nonce = their_nonce;
        //
        self.session_activated = None;
        self.session_nonce = Some(nonce);
        self.queue.i.clear();

        // Track the coordinators' round1 package.
        let mut in_round1_packages = BTreeMap::new();
        in_round1_packages.insert(initiator, their_package.clone());

        self.queue.send_round1_ack(initiator, self.coordinator);

        // Generate the new secret package and our round1 package
        let (secret_package, our_round1_package) = frost::keys::dkg::part1(
            self.my_frost_id,
            self.config.max_signers,
            self.config.min_signers,
            thread_rng(),
        )?;

        // Seal our round1 package.
        let (sealed_round1_package, our_eph_pub, our_sig) =
            sealed_pkg::SealedRoundOnePackage::new(our_round1_package.clone(), &mut auth)?;

        let out_entry = OutEntryRoundOne {
            package: sealed_round1_package.clone(),
            ephemeral_pub: our_eph_pub,
            signature: our_sig,
            timer: None,
            attempts: 0,
        };

        let mut out_round1_packages = BTreeMap::new();

        // Non-coordinators only have one outgoing package to send (to the
        // coordinator).
        out_round1_packages
            .insert((Initiator(self.my_frost_id), self.coordinator), Some(out_entry));

        // Push the pending, outgoing payload to the queue.
        let msg = DkgPayload {
            sender: self.my_frost_id,
            recipient: self.coordinator,
            msg: DkgMessage::Round1 {
                context: context.clone(),
                nonce,
                initiator: Initiator(self.my_frost_id),
                ephemeral_pub: our_eph_pub,
                signature: our_sig,
                package: sealed_round1_package,
            },
        };

        self.queue.i.push_back(msg);

        self.state = StageState::RoundOne {
            context,
            nonce,
            auth,
            secret_package,
            in_round1_packages,
            out_round1_packages,
        };

        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    fn on_dkg_msg_round1(
        &mut self,
        initiator: Initiator,
        their_context: Vec<u8>,
        their_nonce: u64,
        their_ephmeral_pub: secp256k1::PublicKey,
        their_signature: secp256k1::ecdsa::Signature,
        their_package: sealed_pkg::SealedRoundOnePackage,
        sender: frost::Identifier,
    ) -> Result<(), Error> {
        if !self.members.contains_key(&initiator.0) {
            return Ok(());
        }

        if initiator.0 == self.my_frost_id {
            return Ok(());
        }

        if their_context != SESSION_CONTEXT {
            return Ok(());
        }

        // Do a simple check to determine whether we should start the new
        // session transition process.
        let do_new_session_check = if let Some(last) = self.session_nonce {
            match their_nonce.cmp(&last) {
                std::cmp::Ordering::Greater => {
                    // The package is for a new session, do check.
                    true
                }
                std::cmp::Ordering::Equal => {
                    // The package is for the active session, ignore check.
                    false
                }
                std::cmp::Ordering::Less => {
                    // Outdated nonce, ignore.
                    return Ok(());
                }
            }
        } else {
            // No session active, do check.
            debug_assert!(!self.is_coordinator());
            true
        };

        // Validate the new session request.
        if do_new_session_check &&
            self.validate_new_session_init(
                initiator,
                their_context,
                their_nonce,
                their_ephmeral_pub,
                their_signature,
                their_package.clone(),
            )?
        {
            // A coordinator never reaches this point.
            debug_assert!(!self.is_coordinator());

            // New session accepted!
            return Ok(());
        }

        let self_is_coordinator = self.is_coordinator();

        let StageState::RoundOne {
            context,
            nonce,
            auth,
            in_round1_packages,
            out_round1_packages,
            ..
        } = &mut self.state
        else {
            if self.state.did_round_one_finalize() {
                // Send acknowledgments for previous rounds.
                self.queue.send_round1_ack(initiator, sender);
            }

            // Nothing left to do.
            return Ok(());
        };

        if in_round1_packages.contains_key(&initiator) {
            self.queue.send_round1_ack(initiator, sender);
            return Ok(());
        }

        let extracted_package =
            their_package.clone().extract(initiator, their_ephmeral_pub, their_signature, auth)?;

        in_round1_packages.insert(initiator, extracted_package);
        self.queue.send_round1_ack(initiator, sender);

        if self_is_coordinator {
            // Forward the round1 package to all other members.
            for recipient in self.members.keys().copied() {
                if recipient == self.my_frost_id || recipient == sender {
                    continue;
                }

                // Track each outgoing package.
                let Some(entry) = out_round1_packages.get_mut(&(initiator, recipient)) else {
                    continue;
                };

                *entry = Some(OutEntryRoundOne {
                    package: their_package.clone(),
                    ephemeral_pub: their_ephmeral_pub,
                    signature: their_signature,
                    timer: None,
                    attempts: 0,
                });

                // Push outgoing payload to the queue.
                let msg = DkgPayload {
                    sender: self.my_frost_id,
                    recipient,
                    msg: DkgMessage::Round1 {
                        context: context.clone(),
                        nonce: *nonce,
                        initiator,
                        ephemeral_pub: their_ephmeral_pub,
                        signature: their_signature,
                        package: their_package.clone(),
                    },
                };

                self.queue.i.push_back(msg);
            }
        }

        Ok(())
    }
    fn on_dkg_msg_round2(
        &mut self,
        initiator: Initiator,
        target: Target,
        ciphernonce: u64,
        ciphertext: Vec<u8>,
        sender: frost::Identifier,
    ) -> Result<(), Error> {
        if !self.members.contains_key(&initiator.0) {
            return Ok(());
        }

        if initiator.0 == self.my_frost_id {
            return Ok(());
        }

        let self_is_coordinator = self.is_coordinator();

        let StageState::RoundTwo { pending, auth, in_round2_packages, out_round2_packages, .. } =
            &mut self.state
        else {
            if self.state.did_round_two_finalize() {
                // Send acknowledgments for previous rounds.
                self.queue.send_round2_ack(initiator, sender, target);
            }

            // Nothing left to do.
            return Ok(());
        };

        // If the target is us, we attempt to decrypt it and store the
        // plaintext version of it locally.
        if target.0 == self.my_frost_id {
            if in_round2_packages.contains_key(&initiator) {
                self.queue.send_round2_ack(initiator, sender, target);
                return Ok(());
            }

            let package = auth.validate_round2(initiator, ciphernonce, &ciphertext)?;

            // Insert the package.
            in_round2_packages.insert(initiator, package);
            self.queue.send_round2_ack(initiator, sender, target);

            // As a non-coordinator in the pending state, we start sending all
            // our round2 packages to the coordinator as soon as we receive the
            // first round2 package.
            if *pending {
                debug_assert!(!self_is_coordinator);

                for ((initiator, target), entry) in out_round2_packages.iter() {
                    let Some(entry) = entry else {
                        // Package not available.
                        continue;
                    };

                    debug_assert_eq!(initiator.0, self.my_frost_id);

                    // Push the pending, outgoing payload to the queue.
                    let msg = DkgPayload {
                        sender: self.my_frost_id,
                        recipient: self.coordinator,
                        msg: DkgMessage::Round2 {
                            initiator: *initiator,
                            target: *target,
                            nonce: entry.ciphernonce,
                            package: entry.ciphertext.clone(),
                        },
                    };

                    self.queue.i.push_back(msg);
                }

                *pending = false;
            }
        } else {
            // If we're the coordinator, then we forward this package to the
            // intended recipient.
            if !self_is_coordinator {
                // We're not the coordinator; just drop the package.
                return Ok(());
            }

            debug_assert!(!*pending);

            // Track the outgoing package.
            let Some(entry) = out_round2_packages.get_mut(&(initiator, target)) else {
                return Ok(());
            };

            *entry = Some(OutEntryRoundTwo {
                ciphernonce,
                ciphertext: ciphertext.clone(),
                timer: None,
                attempts: 0,
            });

            self.queue.send_round2_ack(initiator, sender, target);

            // Push the outgoing payload to the queue.
            let msg = DkgPayload {
                sender: self.my_frost_id,
                recipient: target.0,
                msg: DkgMessage::Round2 {
                    initiator,
                    target,
                    nonce: ciphernonce,
                    package: ciphertext,
                },
            };

            self.queue.i.push_back(msg);
        }

        Ok(())
    }
    fn on_dkg_msg_round3(
        &mut self,
        initiator: Initiator,
        sealed_signature: sealed_pkg::SealedRoundThreeSignature,
        sender: frost::Identifier,
    ) -> Result<(), Error> {
        if !self.members.contains_key(&initiator.0) {
            return Ok(());
        }

        if initiator.0 == self.my_frost_id {
            return Ok(());
        }

        let self_is_coordinator = self.is_coordinator();

        let StageState::RoundThree {
            pending, auth, in_round3_packages, out_round3_packages, ..
        } = &mut self.state
        else {
            if self.state.did_round_three_finalize() {
                // Send acknowledgments for previous rounds.
                self.queue.send_round3_ack(initiator, sender);
            }

            // Nothing left to do.
            return Ok(());
        };

        if in_round3_packages.contains_key(&initiator) {
            self.queue.send_round3_ack(initiator, sender);
            return Ok(());
        }

        let their_signature = sealed_signature.clone().extract(initiator, auth)?;

        in_round3_packages.insert(initiator, their_signature);
        self.queue.send_round3_ack(initiator, sender);

        if *pending {
            debug_assert!(!self_is_coordinator);
            debug_assert_eq!(out_round3_packages.len(), 1);

            for ((initiator, recipient), entry) in out_round3_packages.iter() {
                let Some(entry) = entry else {
                    // Package not available.
                    continue;
                };

                debug_assert_eq!(initiator.0, self.my_frost_id);
                debug_assert_eq!(recipient, &self.coordinator);

                // Push the pending, outgoing payload to the queue.
                let msg = DkgPayload {
                    sender: self.my_frost_id,
                    recipient: *recipient,
                    msg: DkgMessage::Round3 {
                        initiator: *initiator,
                        signature: entry.signature.clone(),
                    },
                };

                self.queue.i.push_back(msg);
            }

            *pending = false;
        }

        if self_is_coordinator {
            // Forward the round3 package to all other members.
            for recipient in self.members.keys().copied() {
                if recipient == self.my_frost_id || recipient == sender {
                    continue;
                }

                // Track each outgoing package.
                let Some(entry) = out_round3_packages.get_mut(&(initiator, recipient)) else {
                    continue;
                };

                *entry = Some(OutEntryRoundThree {
                    signature: sealed_signature.clone(),
                    timer: None,
                    attempts: 0,
                });

                // Push outgoing payload to the queue.
                let msg = DkgPayload {
                    sender: self.my_frost_id,
                    recipient,
                    msg: DkgMessage::Round3 { initiator, signature: sealed_signature.clone() },
                };

                self.queue.i.push_back(msg);
            }
        }

        Ok(())
    }
    fn transition_stage2_checked(&mut self) -> Result<(), Error> {
        let StageState::RoundOne {
            auth,
            secret_package,
            in_round1_packages,
            out_round1_packages,
            ..
        } = &mut self.state
        else {
            // Ignore
            return Ok(());
        };

        // If we're still missing packages or there are un-acked
        // outgoing packages, return early.
        if in_round1_packages.len() != self.members.len() - 1 || !out_round1_packages.is_empty() {
            return Ok(());
        }

        // Start transition.
        let in_round1_packages = std::mem::take(in_round1_packages)
            .into_iter()
            .map(|(initiator, pkg)| (initiator.0, pkg))
            .collect();

        // Finalize DKG part2.
        let Ok((secret_package, dkg2_shares)) =
            frost::keys::dkg::part2(secret_package.clone(), &in_round1_packages)
        else {
            // On error, we abort the entire DKG process.
            //
            // TODO (lamafab): We should probably do some extra things here,
            // such as communicating this information to the coordinator/peers.
            self.state = StageState::Aborted;
            return Ok(());
        };

        // AUTHENTICATION: Proceed to the next round.
        let mut auth = auth.finalize()?;

        // Assigning the shares to the corresponding recipient and final
        // target.
        let mut out_round2_packages = BTreeMap::new();

        for (target, our_package) in dkg2_shares.clone() {
            let initiator = Initiator(self.my_frost_id);
            let target = Target(target);

            // AUTHENTICATION: Encrypt the package for the target individually.
            let (ciphertext, ciphernonce) = auth.commit_round2(&target, &our_package)?;

            let out_entry = OutEntryRoundTwo {
                ciphernonce,
                ciphertext: ciphertext.clone(),
                timer: None,
                attempts: 0,
            };

            // Track each outgoing package.
            out_round2_packages.insert((initiator, target), Some(out_entry));

            if self.is_coordinator() {
                // Push each outgoing payload to the queue.
                let msg = DkgPayload {
                    sender: self.my_frost_id,
                    recipient: target.0,
                    msg: DkgMessage::Round2 {
                        initiator,
                        target,
                        nonce: ciphernonce,
                        package: ciphertext,
                    },
                };

                self.queue.i.push_back(msg);
            }
        }

        // Non-coordinators will wait until they receive the first
        // round2 package before they start sending theirs.
        let pending = !self.is_coordinator();

        if self.is_coordinator() {
            // As the coordinator, we prepare all additional outgoing/forwarded
            // round2 package entries that we need to have acknowledged.
            //
            // For example; with three participants Alice (us), Bob, and Eve, we construct:
            // * Bob -> Eve (forwarded)
            // * Eve -> Bob (forwarded)
            for initiator in self.members.keys().copied() {
                // Skip ourself, we set those entries before.
                if initiator == self.my_frost_id {
                    continue;
                }

                for target in self.members.keys().copied() {
                    // Skip ourself.
                    if target == self.my_frost_id {
                        continue;
                    }

                    if initiator == target {
                        continue;
                    }

                    // Create an entry with an empty package; it will be updated
                    // when it's received.
                    out_round2_packages.insert((Initiator(initiator), Target(target)), None);
                }
            }
        }

        self.state = StageState::RoundTwo {
            auth,
            pending,
            secret_package,
            in_round1_packages,
            in_round2_packages: BTreeMap::new(),
            out_round2_packages,
        };

        Ok(())
    }
    fn transition_stage3_checked(&mut self) -> Result<(), Error> {
        let StageState::RoundTwo {
            pending,
            auth,
            secret_package,
            in_round1_packages,
            in_round2_packages,
            out_round2_packages,
        } = &mut self.state
        else {
            // Ignore
            return Ok(());
        };

        debug_assert_eq!(in_round1_packages.len(), self.members.len() - 1);

        // If we're still missing packages or there are un-acked
        // outgoing packages, return early.
        if in_round2_packages.len() != self.members.len() - 1 || !out_round2_packages.is_empty() {
            return Ok(());
        }

        debug_assert!(!*pending);

        // Start transition.
        let in_round1_packages = std::mem::take(in_round1_packages);
        let in_round2_packages = std::mem::take(in_round2_packages)
            .into_iter()
            .map(|(initiator, pkg)| (initiator.0, pkg))
            .collect();

        // Finalize DKG part3
        let Ok((secret_package, public_key_package)) =
            frost::keys::dkg::part3(secret_package, &in_round1_packages, &in_round2_packages)
        else {
            // On error, we abort the entire DKG process.
            //
            // TODO (lamafab): We should probably do some extra things here,
            // such as communicating this information to the coordinator/peers.
            return Ok(());
        };

        // AUTHENTICATION: Proceed to the next round, then sign and seal the
        // package.
        let mut auth = auth.finalize()?;

        let our_sealed_signature =
            sealed_pkg::SealedRoundThreeSignature::new(public_key_package.clone(), &mut auth)?;

        let out_entry = OutEntryRoundThree {
            signature: our_sealed_signature.clone(),
            timer: None,
            attempts: 0,
        };

        let mut out_round3_packages = BTreeMap::new();
        let pending = !self.is_coordinator();

        if self.is_coordinator() {
            // Prepare all outgoing round3 package entries that we need to have
            // acknowledged, including forwarded messages.
            //
            // For example; with three participants Alice (us), Bob, and Eve, we construct:
            // * Alice -> Bob
            // * Alice -> Eve
            // * Bob -> Eve (forwarded)
            // * Eve -> Bob (forwarded)
            for initiator in self.members.keys().copied() {
                for recipient in self.members.keys().copied() {
                    // Skip ourself.
                    if recipient == self.my_frost_id {
                        continue;
                    }

                    if initiator == recipient {
                        continue;
                    }

                    // Only set our packages; forwarded packages are set once
                    // they're received, of course.
                    let out_entry =
                        if initiator == self.my_frost_id { Some(out_entry.clone()) } else { None };

                    out_round3_packages.insert((Initiator(initiator), recipient), out_entry);

                    if initiator != self.my_frost_id {
                        // Skip sending unless it's us.
                        continue;
                    }

                    let msg = DkgPayload {
                        sender: self.my_frost_id,
                        recipient,
                        msg: DkgMessage::Round3 {
                            initiator: Initiator(self.my_frost_id),
                            signature: our_sealed_signature.clone(),
                        },
                    };

                    self.queue.i.push_back(msg);
                }
            }
        } else {
            // Non-coordinators only have one outgoing package to send (to the
            // coordinator).
            out_round3_packages
                .insert((Initiator(self.my_frost_id), self.coordinator), Some(out_entry));
        }

        self.state = StageState::RoundThree {
            pending,
            auth,
            public_key_package,
            secret_package,
            in_round1_packages,
            in_round2_packages,
            in_round3_packages: BTreeMap::new(),
            out_round3_packages,
        };

        Ok(())
    }
    fn transition_final_checked(&mut self) -> Result<(), Error> {
        let StageState::RoundThree {
            pending,
            auth,
            public_key_package,
            secret_package,
            in_round1_packages,
            in_round2_packages,
            in_round3_packages,
            out_round3_packages,
        } = &mut self.state
        else {
            // Ignore
            return Ok(());
        };

        debug_assert_eq!(in_round1_packages.len(), self.members.len() - 1);
        debug_assert_eq!(in_round2_packages.len(), self.members.len() - 1);

        // If we're still missing packages or there are un-acked
        // outgoing packages, return early.
        if in_round3_packages.len() != self.members.len() - 1 || !out_round3_packages.is_empty() {
            return Ok(());
        }

        // TODO (lamafab): This is currently unused; implement a fourth round
        // where this is signed and verified? This could be used as a way to
        // guarantee that the coordinator has reliably forwarded all round3
        // packages, and that each and every participant has persisted the
        // aggregated keys on-disk.
        let _auth = auth.finalize()?;

        debug_assert!(!*pending);

        self.state = StageState::Finalized {
            secret_package: secret_package.clone(),
            public_key_package: public_key_package.clone(),
        };

        // Reset session parameters.
        self.session_nonce = None;
        self.session_activated = None;

        Ok(())
    }
}
