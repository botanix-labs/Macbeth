#![allow(unreachable_pub)]
use std::str::FromStr;

use reth_eth_wire::{capability::Capability, protocol::Protocol};
use reth_primitives::{Buf, BufMut, BytesMut};
use reth_rpc_types::PeerId;

const MESSAGE_VERSION: usize = 1;

/// A structured frost DKG message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DkgRequest {
    /// The version of the request message
    pub version: u16,
    /// Frost identifier
    pub identifier: Vec<u8>,
    /// Frost data
    pub data: Vec<u8>,
}

impl DkgRequest {
    /// Constructs a new DKG Request using a frost identifier and a data payload.
    pub fn new(identifier: Vec<u8>, data: Vec<u8>) -> Self {
        DkgRequest { version: MESSAGE_VERSION as u16, identifier, data }
    }
}

/// A structured frost sign message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignRequest {
    /// The version of the request message
    pub version: u16,
    /// Frost identifier
    pub identifier: Vec<u8>,
    /// Signing session id
    pub signing_session_id: Vec<u8>,
    /// Frost data
    pub psbt: Vec<u8>,
}

impl SignRequest {
    /// Constructs a new sign Request using a frost identifier, signing session id and a psbt
    /// payload.
    pub fn new(identifier: Vec<u8>, signing_session_id: Vec<u8>, psbt: Vec<u8>) -> Self {
        SignRequest { version: MESSAGE_VERSION as u16, identifier, signing_session_id, psbt }
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
}

/// Enum defining the frost message kind
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrostProtoMessageKind {
    /// Round 1 package
    Round1Dkg(DkgRequest),
    /// Round 2 package
    Round2Dkg(DkgRequest),
    /// Ping
    Ping,
    /// Pong
    Pong,
    /// Ping message with a user-defined message
    PingMessage(PeerId, u16),
    /// Pong message with a user peer id and an authority index
    PongMessage(PeerId, u16),
    /// Signers will add their signing commitments to the psbt
    SignerRound1SigningPackage(SignRequest),
    /// Coordinating node will collect the PSBTs with the signing commitments
    CoordinatorRound1SigningPackage(SignRequest),
    /// Signers get round 2 signing package
    SignerRound2SigningPackage(SignRequest),
    /// Coordinating node will collect the PSBTs with the partial sigs
    CoordinatorRound2SigningPackage(SignRequest),
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
    /// Returns the capability for the `ping` protocol.
    pub fn capability() -> Capability {
        Capability::new_static("frost", MESSAGE_VERSION)
    }

    /// Returns the protocol for the `test` protocol.
    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 6)
    }

    /// Creates a ping message
    pub fn ping() -> Self {
        Self { message_type: FrostProtoMessageId::Ping, message: FrostProtoMessageKind::Ping }
    }

    /// Creates a pong message
    pub fn pong() -> Self {
        Self { message_type: FrostProtoMessageId::Pong, message: FrostProtoMessageKind::Pong }
    }

    /// Creates a ping message
    pub fn ping_message(peer_id: PeerId, authority_index: u16) -> Self {
        Self {
            message_type: FrostProtoMessageId::PingMessage,
            message: FrostProtoMessageKind::PingMessage(peer_id, authority_index),
        }
    }
    /// Creates a ping message
    pub fn pong_message(peer_id: PeerId, authority_index: u16) -> Self {
        Self {
            message_type: FrostProtoMessageId::PongMessage,
            message: FrostProtoMessageKind::PongMessage(peer_id, authority_index),
        }
    }

    /// Creates a round1 package message
    pub fn round1_dkg_message(resource: DkgRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round1Dkg,
            message: FrostProtoMessageKind::Round1Dkg(resource),
        }
    }

    /// Creates a round2 package message
    pub fn round2_dkg_message(resource: DkgRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round2Dkg,
            message: FrostProtoMessageKind::Round2Dkg(resource),
        }
    }

    /// Signers adding their signing commitments to the psbt
    pub fn round1_signer_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::SignerRound1SigningPackage,
            message: FrostProtoMessageKind::SignerRound1SigningPackage(resource),
        }
    }

    /// Coordinating node collecting the PSBTs with the signing commitments
    pub fn round1_coordinator_signing_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::CoordinatorRound1SigningPackage,
            message: FrostProtoMessageKind::CoordinatorRound1SigningPackage(resource),
        }
    }

    /// Signers get round 2 signing package
    pub fn round2_signer_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::SignerRound2SigningPackage,
            message: FrostProtoMessageKind::SignerRound2SigningPackage(resource),
        }
    }

    /// Coordinating node collecting the PSBTs with the partial sigs
    pub fn round2_coordinator_signing_package_message(resource: SignRequest) -> Self {
        Self {
            message_type: FrostProtoMessageId::CoordinatorRound2SigningPackage,
            message: FrostProtoMessageKind::CoordinatorRound2SigningPackage(resource),
        }
    }

    /// Creates a new `TestProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            FrostProtoMessageKind::Round1Dkg(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // data
                buf.put_u32_le(resource.data.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.data);
            }
            FrostProtoMessageKind::Round2Dkg(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // data
                buf.put_u32_le(resource.data.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.data);
            }
            FrostProtoMessageKind::Ping => {}
            FrostProtoMessageKind::Pong => {}
            FrostProtoMessageKind::PingMessage(peer_id, authority_index) => {
                // peer id
                let peer_id_str = peer_id.to_string();
                let peer_id_bytes = peer_id_str.as_bytes();
                buf.put_u16_le(peer_id_bytes.len() as u16); // Store the length of the peer_id string
                buf.put_slice(peer_id_bytes); // Store the peer_id string itself
                                              // authority index
                buf.put_u16_le(*authority_index); // Store the authority_index
            }
            FrostProtoMessageKind::PongMessage(peer_id, authority_index) => {
                // peer id
                let peer_id_str = peer_id.to_string();
                let peer_id_bytes = peer_id_str.as_bytes();
                buf.put_u16_le(peer_id_bytes.len() as u16); // Store the length of the peer_id string
                buf.put_slice(peer_id_bytes); // Store the peer_id string itself
                                              // authority index
                buf.put_u16_le(*authority_index); // Store the authority_index
            }
            FrostProtoMessageKind::SignerRound1SigningPackage(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // signing session id
                buf.put_u32_le(resource.signing_session_id.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.signing_session_id);
                // psbt
                buf.put_u32_le(resource.psbt.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.psbt);
            }
            FrostProtoMessageKind::CoordinatorRound1SigningPackage(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // signing session id
                buf.put_u32_le(resource.signing_session_id.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.signing_session_id);
                // psbt
                buf.put_u32_le(resource.psbt.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.psbt);
            }
            FrostProtoMessageKind::SignerRound2SigningPackage(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // signing session id
                buf.put_u32_le(resource.signing_session_id.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.signing_session_id);
                // psbt
                buf.put_u32_le(resource.psbt.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.psbt);
            }
            FrostProtoMessageKind::CoordinatorRound2SigningPackage(resource) => {
                // identifier
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                // signing session id
                buf.put_u32_le(resource.signing_session_id.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.signing_session_id);
                // psbt
                buf.put_u32_le(resource.psbt.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.psbt);
            }
        }
        buf
    }

    /// Decodes a `TestProtoMessage` from the given message buffer.
    pub fn decode_message(buf: &mut &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None
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

                FrostProtoMessageKind::Round1Dkg(DkgRequest::new(identifier, data))
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

                FrostProtoMessageKind::Round2Dkg(DkgRequest::new(identifier, data))
            }
            FrostProtoMessageId::Ping => FrostProtoMessageKind::Ping,
            FrostProtoMessageId::Pong => FrostProtoMessageKind::Pong,
            FrostProtoMessageId::PingMessage => {
                let peer_id_len = u16::from_le_bytes(buf[..2].try_into().unwrap()) as usize;
                buf.advance(2);
                let peer_id_str = std::str::from_utf8(&buf[..peer_id_len]).unwrap();
                let peer_id = PeerId::from_str(peer_id_str).unwrap(); // Assuming from_str can never fail in this context
                buf.advance(peer_id_len);

                let authority_index = u16::from_le_bytes(buf[..2].try_into().unwrap());
                buf.advance(2);

                FrostProtoMessageKind::PingMessage(peer_id, authority_index)
            }
            FrostProtoMessageId::PongMessage => {
                let peer_id_len = u16::from_le_bytes(buf[..2].try_into().unwrap()) as usize;
                buf.advance(2);
                let peer_id_str = std::str::from_utf8(&buf[..peer_id_len]).unwrap();
                let peer_id = PeerId::from_str(peer_id_str).unwrap(); // Assuming from_str can never fail in this context
                buf.advance(peer_id_len);

                let authority_index = u16::from_le_bytes(buf[..2].try_into().unwrap());
                buf.advance(2);

                FrostProtoMessageKind::PongMessage(peer_id, authority_index)
            }
            FrostProtoMessageId::SignerRound1SigningPackage => {
                // id
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);
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
                    identifier,
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::CoordinatorRound1SigningPackage => {
                // id
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);
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
                    identifier,
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::SignerRound2SigningPackage => {
                // id
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);
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
                    identifier,
                    signing_session_id,
                    psbt,
                ))
            }
            FrostProtoMessageId::CoordinatorRound2SigningPackage => {
                // id
                let id_len = buf[0] as usize;
                buf.advance(1);
                let identifier = buf[..id_len].to_vec();
                buf.advance(id_len);
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
                    identifier,
                    signing_session_id,
                    psbt,
                ))
            }
        };
        Some(Self { message_type, message })
    }
}

mod tests {
    #[allow(unused_imports)]
    use super::{FrostProtoMessage, FrostProtoMessageId, FrostProtoMessageKind};
    #[allow(unused_imports)]
    use reth_rpc_types::PeerId;
    #[allow(unused_imports)]
    use std::str::FromStr;

    #[test]
    fn test_dkg_encoding_decoding() {
        use super::{DkgRequest, FrostProtoMessage, FrostProtoMessageId, FrostProtoMessageKind};

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
        use super::{FrostProtoMessage, FrostProtoMessageId, FrostProtoMessageKind, SignRequest};

        let signing_request =
            SignRequest::new(vec![1, 2, 3, 4], vec![5, 6, 7, 8, 9], vec![0, 1, 0, 1, 0]);

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
        let authority_index = 2u16;

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::PingMessage,
            message: FrostProtoMessageKind::PingMessage(peer_id, authority_index),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode PingMessage");

        // Verify that the decoded message matches the original message
        if let FrostProtoMessageKind::PingMessage(decoded_peer_id, decoded_authority_index) =
            decoded_message.message
        {
            assert_eq!(decoded_peer_id, peer_id, "PeerId does not match");
            assert_eq!(decoded_authority_index, authority_index, "Authority index does not match");
        } else {
            panic!("Decoded message is not a PingMessage");
        }
    }

    #[test]
    fn test_pong_message_encode_decode() {
        let peer_id = PeerId::from_str("6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0").unwrap();
        let authority_index = 20u16;

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::PongMessage,
            message: FrostProtoMessageKind::PongMessage(peer_id, authority_index),
        };

        // Encode the message
        let encoded_bytes = message.encoded();

        // Simulate receiving the encoded bytes and decoding them
        let mut encoded_bytes_slice: &[u8] = &encoded_bytes;
        let decoded_message = FrostProtoMessage::decode_message(&mut encoded_bytes_slice)
            .expect("Failed to decode PongMessage");

        // Verify that the decoded message matches the original message
        if let FrostProtoMessageKind::PongMessage(decoded_peer_id, decoded_authority_index) =
            decoded_message.message
        {
            assert_eq!(decoded_peer_id, peer_id, "PeerId does not match");
            assert_eq!(decoded_authority_index, authority_index, "Authority index does not match");
        } else {
            panic!("Decoded message is not a PongMessage");
        }
    }
}
