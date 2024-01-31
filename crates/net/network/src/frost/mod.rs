#![allow(unreachable_pub)]
//! Testing gossiping of transactions.

use super::protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler};
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

pub mod manager;
pub mod messages;
pub mod protocol;

#[derive(Clone, Debug)]
struct ProtocolState {
    events: mpsc::UnboundedSender<ProtocolEvent>,
}

#[derive(Debug)]
enum ProtocolEvent {
    Established {
        #[allow(dead_code)]
        direction: Direction,
        peer_id: PeerId,
        to_connection: mpsc::UnboundedSender<Command>,
    },
}

enum Command {
    /// Send a ping message to the peer.
    PingMessage {
        msg: String,
        /// The response will be sent to this channel.
        response: oneshot::Sender<String>,
    },
}
