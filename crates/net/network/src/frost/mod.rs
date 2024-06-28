#![allow(unreachable_pub)]
//! Testing gossiping of transactions.
use core::fmt;

use reth_network_api::Direction;
use reth_primitives::SealedBlock;
use reth_rpc_types::PeerId;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// Manager implementation
pub mod manager;
/// Frost Messaging
pub mod messages;
/// Frost Protocol
pub mod protocol;

/// Protocol state containing peer protocol information.
#[derive(Clone, Debug)]
pub struct ProtocolState {
    events: mpsc::UnboundedSender<FrostProtocolEvent>,
    peer_message_forwarder: mpsc::UnboundedSender<FrostProtocolEvent>,
    authority_index: u16,
    peer_id: PeerId,
    authorities: Vec<PeerId>,
}

impl ProtocolState {
    /// Constructs a new Protocol State.
    pub fn new(
        events: mpsc::UnboundedSender<FrostProtocolEvent>,
        peer_message_forwarder: mpsc::UnboundedSender<FrostProtocolEvent>,
        authority_index: u16,
        peer_id: PeerId,
        authorities: Vec<PeerId>,
    ) -> Self {
        Self { events, peer_message_forwarder, authority_index, peer_id, authorities }
    }
}

/// Enum for peer message responses for dkg, signing and pbft
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PeerMessageResponse {
    /// Dkg response
    Dkg(DkgResponse),
    /// Signing response
    Signing(SigningResponse),
    /// PBFT related responses
    Pbft(PbftResponse),
    /// UTXO related responses
    Utxo(UtxoResponse),
}

impl fmt::Display for PeerMessageResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerMessageResponse::Dkg(response) => write!(f, "DKG Response: {}", response),
            PeerMessageResponse::Signing(response) => write!(f, "Signing Response: {}", response),
            PeerMessageResponse::Pbft(response) => write!(f, "PBFT Response: {}", response),
            PeerMessageResponse::Utxo(response) => write!(f, "Utxo Response: {}", response),
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

/// Response structure for PBFT internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PbftEventResponseType {
    /// in turn block producer proposes a block to sign    
    CoordinatorBlockProposal,
    /// peer precommitment
    PeerPreCommitment,
    /// peer commitment
    PeerCommitment,
}

impl fmt::Display for PbftEventResponseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PbftEventResponseType::CoordinatorBlockProposal => {
                write!(f, "coordinator block proposal")
            }
            PbftEventResponseType::PeerPreCommitment => write!(f, "peer precommitment"),
            PbftEventResponseType::PeerCommitment => write!(f, "peer commitment"),
        }
    }
}

/// Response structure for internal communication
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PbftResponse {
    /// The Response Type
    pub response_type: PbftEventResponseType,
    /// PBFT data
    pub data: SealedBlock,
}

impl fmt::Display for PbftResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - Data Size: {} bytes", self.response_type, self.data.size())
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
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
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
            DkgEventResponseType::DkgRound1 => write!(f, "dkground 1"),
            DkgEventResponseType::DkgRound2 => write!(f, "dkground 2"),
            DkgEventResponseType::DkgRound1Request => write!(f, "dkground 1 request"),
        }
    }
}

/// Event Response Variants indicating the type of response
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
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
            SigningEventResponseType::SignerRound1SigningPackage => {
                write!(f, "signer round 1 signing package")
            }
            SigningEventResponseType::CoordinatorRound1SigningPackage => {
                write!(f, "coordinator round 1 signing package")
            }
            SigningEventResponseType::SignerRound2SigningPackage => {
                write!(f, "signer round 2 signing package")
            }
            SigningEventResponseType::CoordinatorRound2SigningPackage => {
                write!(f, "coordinator round 2 signing package")
            }
        }
    }
}

/// Frost Protocol Events
#[derive(Debug)]
pub enum FrostProtocolEvent {
    /// An emitted event once the connection is established
    ConnectionEstablished {
        #[allow(dead_code)]
        /// the connection direction - we connected to them, or they to us
        direction: Direction,
        /// the other peer id
        peer_id: PeerId,
        /// the tx sender we send to the other peer to enable it to communicate with us
        to_connection: mpsc::UnboundedSender<FrostPeerCommand>,
    },
    /// An emitted event once a peer sends a message to another peer
    PeerMessage {
        /// The other peer id
        peer_id: PeerId,
        /// The message response
        response: PeerMessageResponse,
    },
    /// Peer confirmation
    PeerConfirmed(PeerId, u16),
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
        /// the connection direction - we connected to them, or they to us
        direction: Direction,
        /// the other peer id
        peer_id: PeerId,
        /// the tx sender we send to the other peer to enable it to communicate with us
        to_connection: mpsc::UnboundedSender<FrostPeerCommand>,
    },
    /// An emitted event once a peer sends a message to another peer
    PeerMessage {
        /// The other peer id
        peer_id: PeerId,
        /// The message response
        response: PeerMessageResponse,
    },
    /// Peer Confirmation
    PeerConfirmed(PeerId, u16),
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

#[cfg(test)]
mod tests {
    use super::{DkgEventResponseType, PeerMessageResponse};

    #[test]
    fn message_format() {
        let msg = PeerMessageResponse::Dkg(super::DkgResponse {
            response_type: DkgEventResponseType::DkgRound1,
            identifier: vec![2, 5, 6, 8],
            data: vec![0, 1, 2, 3],
        });
        println!("Display: {:?}", msg.to_string());
    }
}
