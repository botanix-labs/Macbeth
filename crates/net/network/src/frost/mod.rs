#![allow(unreachable_pub)]
//! Testing gossiping of transactions.
use core::fmt;

use reth_network_api::Direction;
use reth_network_peers::PeerId;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// Manager implementation
pub mod manager;
/// Frost Messaging
pub mod messages;
/// Frost Protocol
pub mod protocol;

/// Enum for peer message responses for dkg, signing and pbft
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PeerMessageResponse {
    /// Dkg response
    Dkg(DkgResponse),
    /// Signing response
    Signing(SigningResponse),
    /// UTXO related responses
    Utxo(UtxoSetResponse),
    /// Healtcheck response
    Healthcheck(HealthcheckResponse),
}

impl fmt::Display for PeerMessageResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dkg(response) => write!(f, "DKG Response: {}", response),
            Self::Signing(response) => write!(f, "Signing Response: {}", response),
            Self::Utxo(response) => write!(f, "Utxo Response: {}", response),
            Self::Healthcheck(response) => {
                write!(f, "Health Response: {:?}", response)
            }
        }
    }
}

/// Response structure for internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DkgResponse {
    /// The Response Type
    pub response_type: DkgEventResponseType,
    /// Frost Identifier
    pub identifier: Vec<u8>,
    /// Frost Data
    pub data: Vec<u8>,
}

impl fmt::Display for DkgResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} - Identifier Size: {} bytes, Data Size: {} bytes",
            self.response_type,
            self.identifier.len(),
            self.data.len()
        )
    }
}

/// Response structure for internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UtxoSetResponse {
    /// Utxo Set Data (Compressed and Serialized)
    pub data: Vec<u8>,
}

impl fmt::Display for UtxoSetResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Utxo data Size: {} bytes", self.data.len())
    }
}

/// Response structure for internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HealthcheckResponse {
    /// the ping requester
    pub sender: PeerId,
    /// pinged peer
    pub receiver: PeerId,
}

impl fmt::Display for HealthcheckResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sender: {}, Receiver: {}", self.sender, self.receiver,)
    }
}

/// Response structure for internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SigningResponse {
    /// The Response Type
    pub response_type: SigningEventResponseType,
    /// Frost identifier
    pub identifier: Vec<u8>,
    /// Signing session id
    pub signing_session_id: Vec<u8>,
    /// Frost data
    pub psbt: Vec<u8>,
}

impl fmt::Display for SigningResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} - Identifier Size: {} bytes, Session ID Size: {} bytes, PSBT Size: {} bytes",
            self.response_type,
            self.identifier.len(),
            self.signing_session_id.len(),
            self.psbt.len()
        )
    }
}

/// Event Response Variants indicating the type of response
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub enum DkgEventResponseType {
    /// DKG round 1 request
    DkgRound1Request,
    /// DKG round 1
    DkgRound1,
    /// DKG round 2
    DkgRound2,
}

impl fmt::Display for DkgEventResponseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DkgRound1 => write!(f, "dkground 1"),
            Self::DkgRound2 => write!(f, "dkground 2"),
            Self::DkgRound1Request => write!(f, "dkground 1 request"),
        }
    }
}

/// Response structure for internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UtxoResponse {
    /// serialized utxo data set
    pub data: Vec<u8>,
}

impl fmt::Display for UtxoResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Utxo Data Size: {} bytes", self.data.len(),)
    }
}

/// Event Response Variants indicating the type of response
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone, Copy)]
pub enum SigningEventResponseType {
    /// Signers will add their signing commitments to the psbt
    SignerRound1SigningPackage,
    /// Coordinating node will collect the PSBTs with the signing commitments
    CoordinatorRound1SigningPackage,
    /// Signers get round 2 signing package
    SignerRound2SigningPackage,
    /// Coordinating node will collect the PSBTs with the partial sigs
    CoordinatorRound2SigningPackage,
}

impl fmt::Display for SigningEventResponseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignerRound1SigningPackage => {
                write!(f, "signer round 1 signing package")
            }
            Self::CoordinatorRound1SigningPackage => {
                write!(f, "coordinator round 1 signing package")
            }
            Self::SignerRound2SigningPackage => {
                write!(f, "signer round 2 signing package")
            }
            Self::CoordinatorRound2SigningPackage => {
                write!(f, "coordinator round 2 signing package")
            }
        }
    }
}

/// Frost Protocol Events
#[derive(Debug, Clone)]
pub enum FrostProtocolEvent {
    /// An emitted event once the connection is established
    ConnectionEstablished {
        #[allow(dead_code)]
        /// the connection direction - we connected to them, or they to us
        direction: Direction,
        /// the other peer id
        peer_id: PeerId,
        /// the tx sender we send to the other peer to enable it to communicate with us
        peer_commands_tx: mpsc::UnboundedSender<FrostPeerCommand>,
    },
    /// An emitted event once a peer sends a message to another peer
    PeerMessage {
        /// The other peer id
        peer_id: PeerId,
        /// The message response
        response: PeerMessageResponse,
    },
}

/// All events related to frost events emitted by the network.
/// These are events that are emitted by the network to the frost manager.
/// And most likely will be used to update the frost task state.
#[derive(Debug)]
pub enum NetworkFrostEvent {
    /// Represents the event of receiving a list of transactions from a peer.
    ///
    /// This indicates transactions that were broadcasted to us from the peer.
    ConnectionEstablished {
        #[allow(dead_code)]
        /// the connection direction - we dialed them, or they dialed us
        direction: Direction,
        /// the other peer id
        peer_id: PeerId,
        /// the tx sender we send to the other peer to enable it to communicate with us
        peer_commands_tx: mpsc::UnboundedSender<FrostPeerCommand>,
    },
    /// An emitted event once a peer sends a message to another peer
    PeerMessage {
        /// The other peer id
        peer_id: PeerId,
        /// The message response
        response: PeerMessageResponse,
    },
}

/// Commands sent by us to a peer.
/// These are commands that are sent by the frost manager to the network via most likely the frost
/// task
#[derive(Debug)]
pub enum FrostPeerCommand {
    /// Send a ping message to the peer.
    PingMessage {
        /// The message text
        msg: String,
        /// The stringified response will be sent to this channel.
        response: oneshot::Sender<String>,
    },
    /// An emitted event once a peer sends a message to another peer
    PeerMessage(PeerMessageResponse),
}
