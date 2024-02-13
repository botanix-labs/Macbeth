#![allow(unreachable_pub)]
use std::str::FromStr;

use reth_eth_wire::{capability::Capability, protocol::Protocol};
use reth_primitives::{Buf, BufMut, BytesMut};
use reth_rpc_types::PeerId;

const MESSAGE_VERSION: usize = 1;

/// A structured frost message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The version of the request message
    pub version: u16,
    /// Frost identifier
    pub identifier: Vec<u8>,
    /// Frost data
    pub data: Vec<u8>,
}

impl Request {
    /// Constructs a new Request using a frost identifier and a data payload.
    pub fn new(identifier: Vec<u8>, data: Vec<u8>) -> Self {
        Request { version: MESSAGE_VERSION as u16, identifier, data }
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
}

/// Enum defining the frost message kind
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrostProtoMessageKind {
    /// Round 1 package
    Round1Dkg(Request),
    /// Round 2 package
    Round2Dkg(Request),
    /// Ping
    Ping,
    /// Pong
    Pong,
    /// Ping message with a user-defined message
    PingMessage(PeerId, u16),
    /// Pong message with a user peer id and an authority index
    PongMessage(PeerId, u16),
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
    pub fn round1_dkg_message(resource: Request) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round1Dkg,
            message: FrostProtoMessageKind::Round1Dkg(resource),
        }
    }

    /// Creates a round2 package message
    pub fn round2_dkg_message(resource: Request) -> Self {
        Self {
            message_type: FrostProtoMessageId::Round2Dkg,
            message: FrostProtoMessageKind::Round2Dkg(resource),
        }
    }

    /// Creates a new `TestProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            FrostProtoMessageKind::Round1Dkg(resource) => {
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                buf.put_u32_le(resource.data.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.data);
            }
            FrostProtoMessageKind::Round2Dkg(resource) => {
                buf.put_u8(resource.identifier.len() as u8); // Assuming identifier is not too long
                buf.put_slice(&resource.identifier);
                buf.put_u32_le(resource.data.len() as u32); // Use u32 to support larger data sizes
                buf.put_slice(&resource.data);
            }
            FrostProtoMessageKind::Ping => {}
            FrostProtoMessageKind::Pong => {}
            FrostProtoMessageKind::PingMessage(peer_id, authority_index) => {
                let peer_id_str = peer_id.to_string();
                let peer_id_bytes = peer_id_str.as_bytes();
                buf.put_u16_le(peer_id_bytes.len() as u16); // Store the length of the peer_id string
                buf.put_slice(peer_id_bytes); // Store the peer_id string itself
                buf.put_u16_le(*authority_index); // Store the authority_index
            }
            FrostProtoMessageKind::PongMessage(peer_id, authority_index) => {
                let peer_id_str = peer_id.to_string();
                let peer_id_bytes = peer_id_str.as_bytes();
                buf.put_u16_le(peer_id_bytes.len() as u16); // Store the length of the peer_id string
                buf.put_slice(peer_id_bytes); // Store the peer_id string itself
                buf.put_u16_le(*authority_index); // Store the authority_index
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

                FrostProtoMessageKind::Round1Dkg(Request::new(identifier, data))
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

                FrostProtoMessageKind::Round2Dkg(Request::new(identifier, data))
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
        use super::{FrostProtoMessage, FrostProtoMessageId, FrostProtoMessageKind, Request};

        let request = Request::new(vec![1, 2, 3, 4], vec![5, 6, 7, 8, 9]);

        let message = FrostProtoMessage {
            message_type: FrostProtoMessageId::Round1Dkg,
            message: FrostProtoMessageKind::Round1Dkg(request),
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
            message: FrostProtoMessageKind::PingMessage(peer_id.clone(), authority_index),
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
            message: FrostProtoMessageKind::PongMessage(peer_id.clone(), authority_index),
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
