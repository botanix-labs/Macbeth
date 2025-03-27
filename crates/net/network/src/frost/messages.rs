#![allow(unreachable_pub)]
use core::fmt;
use std::str::FromStr;

use reth_eth_wire::{protocol::Protocol, Capability};
use reth_network_peers::PeerId;
use reth_primitives::{Buf, BufMut, BytesMut};

const MESSAGE_VERSION: usize = 0;
const WALLET_STATE_MESSAGE_VERSION: usize = 0;

/// A structured healthcheck message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthcheckRequest {
    /// healthcheck ping sender
    pub sender: PeerId,
    /// healthcheck ping receiver
    pub receiver: PeerId,
}

/// Healtcheck message builder
impl HealthcheckRequest {
    /// Constructs a new healthcheck request
    pub const fn new(sender: PeerId, receiver: PeerId) -> Self {
        Self { sender, receiver }
    }
}

impl fmt::Display for HealthcheckRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Healtcheck sender: {:?}. Healthcheck receiver: {:?}", self.sender, self.receiver)
    }
}

/// A structured frost DKG message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DkgRequest {
    /// The version of the request message
    pub version: u16,
    /// Frost data
    pub data: Vec<u8>,
    /// Frost identifier
    pub identifier: Vec<u8>,
}

impl DkgRequest {
    /// Constructs a new DKG Request using a frost identifier and a data payload.
    pub const fn new(data: Vec<u8>, identifier: Vec<u8>) -> Self {
        Self { version: MESSAGE_VERSION as u16, data, identifier }
    }
}

/// A structured frost sign message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignRequest {
    /// The version of the request message
    pub version: u16,
    /// Signing session id
    pub signing_session_id: Vec<u8>,
    /// Frost data
    pub psbt: Vec<u8>,
}

impl SignRequest {
    /// Constructs a new sign Request using a frost identifier, signing session id and a psbt
    /// payload.
    pub const fn new(signing_session_id: Vec<u8>, psbt: Vec<u8>) -> Self {
        Self { version: MESSAGE_VERSION as u16, signing_session_id, psbt }
    }
}

/// A structured wallet state message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletStateRequest {
    /// The version of the request message
    pub version: u16,
    /// utxos
    pub utxos: Vec<u8>,
    /// tracked transactions
    pub tracked_txs: Vec<u8>,
    /// pending pegouts
    pub pending_pegouts: Vec<u8>,
}

impl fmt::Display for WalletStateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WalletStateRequest:\n\
            - UTXOs: {} bytes\n\
            - Tracked Transactions: {} bytes\n\
            - Pending Pegouts: {} bytes",
            self.utxos.len(),
            self.tracked_txs.len(),
            self.pending_pegouts.len()
        )
    }
}

impl WalletStateRequest {
    /// Constructs a new wallet state request using a data payload.
    pub const fn new(utxos: Vec<u8>, tracked_txs: Vec<u8>, pending_pegouts: Vec<u8>) -> Self {
        Self { version: WALLET_STATE_MESSAGE_VERSION as u16, utxos, tracked_txs, pending_pegouts }
    }
}

/// Enum defining the frost message type as u8
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrostProtoMessageId {
    /// Round 1 package
    Round1Dkg = 0x00,
    /// Round 2 package
    Round2Dkg = 0x01,
    /// Ping
    Ping = 0x02,
    /// Pong
    Pong = 0x03,
    /// Ping message with a user-defined message
    PingMessage = 0x04,
    /// Pong message with a user-defined message
    PongMessage = 0x05,
    /// Signers will add their signing commitments to the psbt
    SignerRound1SigningPackage = 0x06,
    /// Coordinating node will collect the PSBTs with the signing commitments
    CoordinatorRound1SigningPackage = 0x07,
    /// Signers get round 2 signing package
    SignerRound2SigningPackage = 0x08,
    /// Coordinating node will collect the PSBTs with the partial sigs
    CoordinatorRound2SigningPackage = 0x09,
    /// Round 1 Dkg request message
    Round1DkgRequest = 0x0A,
    /// `WalletState`
    WalletState = 0x0B,
}

/// Enum defining the frost message kind
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrostProtoMessageKind {
    /// Round 1 package
    Round1Dkg(DkgRequest),
    /// Round 2 package
    Round2Dkg(DkgRequest),
    /// Round 1 Dkg request
    Round1DkgRequest(DkgRequest),
    /// Ping
    Ping,
    /// Pong
    Pong,
    /// Ping message with a user-defined message
    PingMessage(PeerId),
    /// Pong message with a user peer id
    PongMessage(PeerId),
    /// Signers will add their signing commitments to the psbt
    SignerRound1SigningPackage(SignRequest),
    /// Coordinating node will collect the PSBTs with the signing commitments
    CoordinatorRound1SigningPackage(SignRequest),
    /// Signers get round 2 signing package
    SignerRound2SigningPackage(SignRequest),
    /// Coordinating node will collect the PSBTs with the partial sigs
    CoordinatorRound2SigningPackage(SignRequest),
    /// Wallet state message
    WalletState(WalletStateRequest),
}

/// An protocol message, containing a message ID and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostProtoMessage {
    /// Message Type
    pub message_type: FrostProtoMessageId,
    /// Message Content
    pub message: FrostProtoMessageKind,
}

impl FrostProtoMessage {
    /// Returns the capability for the `frost` protocol.
    pub const fn capability() -> Capability {
        Capability::new_static("frost", MESSAGE_VERSION)
    }

    /// Returns the protocol for the `frost` protocol.
    pub const fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 16)
    }

    /// Creates a ping message
    pub const fn ping() -> Self {
        Self { message_type: FrostProtoMessageId::Ping, message: FrostProtoMessageKind::Ping }
    }

    /// Creates a pong message
    pub const fn pong() -> Self {
        Self { message_type: FrostProtoMessageId::Pong, message: FrostProtoMessageKind::Pong }
    }

    /// Creates a ping message
    pub const fn ping_message(peer_id: PeerId) -> Self {
        Self {
            message_type: FrostProtoMessageId::PingMessage,
            message: FrostProtoMessageKind::PingMessage(peer_id),
        }
    }
    /// Creates a ping message
    pub const fn pong_message(peer_id: PeerId) -> Self {
        Self {
            message_type: FrostProtoMessageId::PongMessage,
            message: FrostProtoMessageKind::PongMessage(peer_id),
        }
    }

    /// Creates a round1 package request message
    pub const fn round1_dkg_request_message(resource: DkgRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round1DkgRequest,
            message: FrostProtoMessageKind::Round1Dkg(resource),
        }
    }

    /// Creates a round1 package message
    pub const fn round1_dkg_message(resource: DkgRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round1Dkg,
            message: FrostProtoMessageKind::Round1Dkg(resource),
        }
    }

    /// Creates a round2 package message
    pub const fn round2_dkg_message(resource: DkgRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round2Dkg,
            message: FrostProtoMessageKind::Round2Dkg(resource),
        }
    }

    /// Signers adding their signing commitments to the psbt
    pub const fn round1_signer_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::SignerRound1SigningPackage,
            message: FrostProtoMessageKind::SignerRound1SigningPackage(resource),
        }
    }

    /// Coordinating node collecting the PSBTs with the signing commitments
    pub const fn round1_coordinator_signing_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::CoordinatorRound1SigningPackage,
            message: FrostProtoMessageKind::CoordinatorRound1SigningPackage(resource),
        }
    }

    /// Signers get round 2 signing package
    pub const fn round2_signer_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::SignerRound2SigningPackage,
            message: FrostProtoMessageKind::SignerRound2SigningPackage(resource),
        }
    }

    /// Coordinating node collecting the PSBTs with the partial sigs
    pub const fn round2_coordinator_signing_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::CoordinatorRound2SigningPackage,
            message: FrostProtoMessageKind::CoordinatorRound2SigningPackage(resource),
        }
    }

    /// Creates a wallet state message
    pub const fn wallet_state_message(resource: WalletStateRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::WalletState,
            message: FrostProtoMessageKind::WalletState(resource),
        }
    }

    /// Creates a new `TestProtoMessage` with the given message ID and payload.
    /// Creates a new Frost protocol with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            FrostProtoMessageKind::Round1Dkg(resource) |
            FrostProtoMessageKind::Round2Dkg(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // data
                buf.put_u32_le(resource.data.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.data);
            }
            FrostProtoMessageKind::Round1DkgRequest(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // data
                // TODO(armins) data is empty, simplify
                buf.put_u32_le(resource.data.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.data);
            }
            FrostProtoMessageKind::Ping | FrostProtoMessageKind::Pong => {}
            FrostProtoMessageKind::PingMessage(peer_id) |
            FrostProtoMessageKind::PongMessage(peer_id) => {
                // peer id
                let peer_id_str = peer_id.to_string();
                let peer_id_bytes = peer_id_str.as_bytes();
                buf.put_u16_le(peer_id_bytes.len() as u16); // Store the length of the peer_id string
                buf.put_slice(peer_id_bytes); // Store the peer_id string itself
            }
            FrostProtoMessageKind::SignerRound1SigningPackage(resource) |
            FrostProtoMessageKind::SignerRound2SigningPackage(resource) |
            FrostProtoMessageKind::CoordinatorRound1SigningPackage(resource) |
            FrostProtoMessageKind::CoordinatorRound2SigningPackage(resource) => {
                // signing session id
                buf.put_u32_le(resource.signing_session_id.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.signing_session_id);
                // psbt
                buf.put_u32_le(resource.psbt.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.psbt);
            }
            FrostProtoMessageKind::WalletState(resource) => {
                // serialize the utxos
                buf.put_u64_le(resource.utxos.len() as u64); // Use u64 to support larger utxos sizes
                buf.put_slice(&resource.utxos);

                // serialize the tracked txs
                buf.put_u64_le(resource.tracked_txs.len() as u64); // Use u64 to support larger tracked txs sizes
                buf.put_slice(&resource.tracked_txs);

                // serialize the pending pegouts
                buf.put_u64_le(resource.pending_pegouts.len() as u64); // Use u64 to support larger pending pegouts sizes
                buf.put_slice(&resource.pending_pegouts);
            }
        }
        buf
    }

    /// Decodes a Frost protocol message from the given message buffer.
    pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        let id = buf[0];
        buf.advance(1);
        let message_type = match id {
            0x00 => FrostProtoMessageId::Round1Dkg,
            0x01 => FrostProtoMessageId::Round2Dkg,
            0x02 => FrostProtoMessageId::Ping,
            0x03 => FrostProtoMessageId::Pong,
            0x04 => FrostProtoMessageId::PingMessage,
            0x05 => FrostProtoMessageId::PongMessage,
            0x06 => FrostProtoMessageId::SignerRound1SigningPackage,
            0x07 => FrostProtoMessageId::CoordinatorRound1SigningPackage,
            0x08 => FrostProtoMessageId::SignerRound2SigningPackage,
            0x09 => FrostProtoMessageId::CoordinatorRound2SigningPackage,
            0x0A => FrostProtoMessageId::Round1DkgRequest,
            0x0B => FrostProtoMessageId::WalletState,
            _ => return None,
        };
        let message = match message_type {
            // Other cases remain unchanged
            FrostProtoMessageId::Round1Dkg => {
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);

                let data_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let data = buf[..data_len].to_vec();
                buf.advance(data_len);

                FrostProtoMessageKind::Round1Dkg(DkgRequest::new(data, identifier))
            }
            FrostProtoMessageId::Round2Dkg => {
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);

                let data_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let data = buf[..data_len].to_vec();
                buf.advance(data_len);

                FrostProtoMessageKind::Round2Dkg(DkgRequest::new(data, identifier))
            }
            FrostProtoMessageId::Round1DkgRequest => {
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);

                let data_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let data = buf[..data_len].to_vec();
                buf.advance(data_len);

                FrostProtoMessageKind::Round1DkgRequest(DkgRequest::new(data, identifier))
            }

            FrostProtoMessageId::Ping => FrostProtoMessageKind::Ping,
            FrostProtoMessageId::Pong => FrostProtoMessageKind::Pong,
            FrostProtoMessageId::PingMessage => {
                let peer_id_len = u16::from_le_bytes(buf[..2].try_into().unwrap()) as usize;
                buf.advance(2);
                let peer_id_str = std::str::from_utf8(&buf[..peer_id_len]).unwrap();
                let peer_id = PeerId::from_str(peer_id_str).unwrap(); // Assuming from_str can never fail in this context
                buf.advance(peer_id_len);

                FrostProtoMessageKind::PingMessage(peer_id)
            }
            FrostProtoMessageId::PongMessage => {
                let peer_id_len = u16::from_le_bytes(buf[..2].try_into().unwrap()) as usize;
                buf.advance(2);
                let peer_id_str = std::str::from_utf8(&buf[..peer_id_len]).unwrap();
                let peer_id = PeerId::from_str(peer_id_str).unwrap(); // Assuming from_str can never fail in this context
                buf.advance(peer_id_len);

                FrostProtoMessageKind::PongMessage(peer_id)
            }
            FrostProtoMessageId::SignerRound1SigningPackage => {
                // Decode signing_session_id as u32
                let session_id_len = u32::from_le_bytes(
                    buf[..4].try_into().expect("Buffer underflow for session ID length"),
                ) as usize;
                buf.advance(4);
                let signing_session_id = buf[..session_id_len].to_vec();
                buf.advance(session_id_len);
                // psbt
                let psbt_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let psbt = buf[..psbt_len].to_vec();
                buf.advance(psbt_len);

                FrostProtoMessageKind::SignerRound1SigningPackage(SignRequest::new(
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::CoordinatorRound1SigningPackage => {
                // Decode signing_session_id as u32
                let session_id_len = u32::from_le_bytes(
                    buf[..4].try_into().expect("Buffer underflow for session ID length"),
                ) as usize;
                buf.advance(4);
                let signing_session_id = buf[..session_id_len].to_vec();
                buf.advance(session_id_len);
                // psbt
                let psbt_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let psbt = buf[..psbt_len].to_vec();
                buf.advance(psbt_len);

                FrostProtoMessageKind::CoordinatorRound1SigningPackage(SignRequest::new(
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::SignerRound2SigningPackage => {
                // Decode signing_session_id as u32
                let session_id_len = u32::from_le_bytes(
                    buf[..4].try_into().expect("Buffer underflow for session ID length"),
                ) as usize;
                buf.advance(4);
                let signing_session_id = buf[..session_id_len].to_vec();
                buf.advance(session_id_len);
                // psbt
                let psbt_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let psbt = buf[..psbt_len].to_vec();
                buf.advance(psbt_len);

                FrostProtoMessageKind::SignerRound2SigningPackage(SignRequest::new(
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::CoordinatorRound2SigningPackage => {
                // Decode signing_session_id as u32
                let session_id_len = u32::from_le_bytes(
                    buf[..4].try_into().expect("Buffer underflow for session ID length"),
                ) as usize;
                buf.advance(4);
                let signing_session_id = buf[..session_id_len].to_vec();
                buf.advance(session_id_len);
                // psbt
                let psbt_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                buf.advance(4);
                let psbt = buf[..psbt_len].to_vec();
                buf.advance(psbt_len);

                FrostProtoMessageKind::CoordinatorRound2SigningPackage(SignRequest::new(
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::WalletState => {
                // utxos
                let utxos_len = u64::from_le_bytes(buf[..8].try_into().unwrap()) as usize;
                buf.advance(8);
                let utxos = buf[..utxos_len].to_vec();
                buf.advance(utxos_len);

                // tracked txs
                let tracked_txs_len = u64::from_le_bytes(buf[..8].try_into().unwrap()) as usize;
                buf.advance(8);
                let tracked_txs = buf[..tracked_txs_len].to_vec();
                buf.advance(tracked_txs_len);

                // pending pegouts
                let pending_pegouts_len = u64::from_le_bytes(buf[..8].try_into().unwrap()) as usize;
                buf.advance(8);
                let pending_pegouts = buf[..pending_pegouts_len].to_vec();
                buf.advance(pending_pegouts_len);

                FrostProtoMessageKind::WalletState(WalletStateRequest::new(
                    utxos,
                    tracked_txs,
                    pending_pegouts,
                ))
            }
        };
        Some(Self { message_type, message })
    }
}

#[cfg(test)]
mod tests {
    use super::WalletStateRequest;
    #[allow(unused_imports)]
    use super::{
        DkgRequest, FrostProtoMessage, FrostProtoMessageId, FrostProtoMessageKind, SignRequest,
    };
    use itertools::Itertools;
    #[allow(unused_imports)]
    use reth_primitives::SealedBlock;
    #[allow(unused_imports)]
    use reth_rpc_types::PeerId;
    #[allow(unused_imports)]
    use std::str::FromStr;

    #[test]
    fn test_dkg_encoding_decoding() {
        let dkg_request = DkgRequest::new(vec![1, 2, 3, 4], vec![5, 6, 7, 8, 9]);

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::Round1Dkg,
            message: FrostProtoMessageKind::Round1Dkg(dkg_request),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode message");

        // Check that the decoded message matches the original message
        assert_eq!(decoded_message, message);
    }

    #[test]
    fn test_signing_encoding_decoding() {
        let signing_request = SignRequest::new(vec![5, 6, 7, 8, 9], vec![0, 1, 0, 1, 0]);

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::SignerRound1SigningPackage,
            message: FrostProtoMessageKind::SignerRound1SigningPackage(signing_request),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode message");
        // Check that the decoded message matches the original message
        assert_eq!(decoded_message, message);
    }

    #[test]
    fn test_ping_message_encode_decode() {
        let peer_id = PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap();

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::PingMessage,
            message: FrostProtoMessageKind::PingMessage(peer_id),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode PingMessage");

        // Verify that the decoded message matches the original message
        if let FrostProtoMessageKind::PingMessage(decoded_peer_id) = decoded_message.message {
            assert_eq!(decoded_peer_id, peer_id, "PeerId does not match");
        } else {
            panic!("Decoded message is not a PingMessage");
        }
    }

    #[test]
    fn test_pong_message_encode_decode() {
        let peer_id = PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap();

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::PongMessage,
            message: FrostProtoMessageKind::PongMessage(peer_id),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode PongMessage");

        // Verify that the decoded message matches the original message
        if let FrostProtoMessageKind::PongMessage(decoded_peer_id) = decoded_message.message {
            assert_eq!(decoded_peer_id, peer_id, "PeerId does not match");
        } else {
            panic!("Decoded message is not a PongMessage");
        }
    }

    #[test]
    fn test_wallet_state_encode_decode() {
        let utxos = "utxos".to_owned();
        let tracked_txs = "tracked_txs".to_owned();
        let pending_pegouts = "pending_pegouts".to_owned();

        let utxos_bytes = utxos.bytes().collect_vec();
        let tracked_txs_bytes = tracked_txs.bytes().collect_vec();
        let pending_pegouts_bytes = pending_pegouts.bytes().collect_vec();

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::WalletState,
            message: FrostProtoMessageKind::WalletState(WalletStateRequest::new(
                utxos_bytes,
                tracked_txs_bytes,
                pending_pegouts_bytes,
            )),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode WalletStateMessage");

        // Verify that the decoded message matches the original message
        if let FrostProtoMessageKind::WalletState(wallet_state_request) = decoded_message.message {
            let decoded_utxos = String::from_utf8(wallet_state_request.utxos).unwrap();
            let decoded_tracked_txs = String::from_utf8(wallet_state_request.tracked_txs).unwrap();
            let decoded_pending_pegouts =
                String::from_utf8(wallet_state_request.pending_pegouts).unwrap();

            assert_eq!(decoded_utxos, utxos, "utxos does not match");
            assert_eq!(decoded_tracked_txs, tracked_txs, "tracked_txs does not match");
            assert_eq!(decoded_pending_pegouts, pending_pegouts, "pending_pegouts does not match");
        } else {
            panic!("Decoded message is not a WalletState Message");
        }
    }
}
