#![allow(unreachable_pub)]
//! Testing gossiping of transactions.

use futures::{Stream, StreamExt};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network_api::Direction;
use reth_primitives::BytesMut;
use reth_rpc_types::PeerId;
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::*;
use reth_eth_wire::capability::Capability;
use reth_primitives::{Buf, BufMut};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingPongProtoMessageId {
    Ping = 0x00,
    Pong = 0x01,
    PingMessage = 0x02,
    PongMessage = 0x03,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PingPongProtoMessageKind {
    Ping,
    Pong,
    PingMessage(String),
    PongMessage(String),
}

/// An protocol message, containing a message ID and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingPongProtoMessage {
    pub message_type: PingPongProtoMessageId,
    pub message: PingPongProtoMessageKind,
}

impl PingPongProtoMessage {
    /// Returns the capability for the `ping` protocol.
    pub fn capability() -> Capability {
        Capability::new_static("ping", 1)
    }

    /// Returns the protocol for the `test` protocol.
    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 4)
    }

    /// Creates a ping message
    pub fn ping() -> Self {
        Self { message_type: PingPongProtoMessageId::Ping, message: PingPongProtoMessageKind::Ping }
    }

    /// Creates a pong message
    pub fn pong() -> Self {
        Self { message_type: PingPongProtoMessageId::Pong, message: PingPongProtoMessageKind::Pong }
    }

    /// Creates a ping message
    pub fn ping_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: PingPongProtoMessageId::PingMessage,
            message: PingPongProtoMessageKind::PingMessage(msg.into()),
        }
    }
    /// Creates a ping message
    pub fn pong_message(msg: impl Into<String>) -> Self {
        Self {
            message_type: PingPongProtoMessageId::PongMessage,
            message: PingPongProtoMessageKind::PongMessage(msg.into()),
        }
    }

    /// Creates a new `TestProtoMessage` with the given message ID and payload.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type as u8);
        match &self.message {
            PingPongProtoMessageKind::Ping => {}
            PingPongProtoMessageKind::Pong => {}
            PingPongProtoMessageKind::PingMessage(msg) => {
                buf.put(msg.as_bytes());
            }
            PingPongProtoMessageKind::PongMessage(msg) => {
                buf.put(msg.as_bytes());
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
            0x00 => PingPongProtoMessageId::Ping,
            0x01 => PingPongProtoMessageId::Pong,
            0x02 => PingPongProtoMessageId::PingMessage,
            0x03 => PingPongProtoMessageId::PongMessage,
            _ => return None,
        };
        let message = match message_type {
            PingPongProtoMessageId::Ping => PingPongProtoMessageKind::Ping,
            PingPongProtoMessageId::Pong => PingPongProtoMessageKind::Pong,
            PingPongProtoMessageId::PingMessage => PingPongProtoMessageKind::PingMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
            PingPongProtoMessageId::PongMessage => PingPongProtoMessageKind::PongMessage(
                String::from_utf8_lossy(&buf[..]).into_owned(),
            ),
        };
        Some(Self { message_type, message })
    }
}
