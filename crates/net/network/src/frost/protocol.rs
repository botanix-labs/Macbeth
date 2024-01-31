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
use tracing::info;

use crate::protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler};

use super::{
    messages::{PingPongProtoMessage, PingPongProtoMessageKind},
    Command, ProtocolEvent, ProtocolState,
};

#[derive(Debug)]
pub struct PingPongProtoHandler {
    pub state: ProtocolState,
}

impl ProtocolHandler for PingPongProtoHandler {
    type ConnectionHandler = PingPongConnectionHandler;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(PingPongConnectionHandler { state: self.state.clone() })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(PingPongConnectionHandler { state: self.state.clone() })
    }
}

#[derive(Debug)]
pub struct PingPongConnectionHandler {
    state: ProtocolState,
}

impl ConnectionHandler for PingPongConnectionHandler {
    type Connection = PingPongProtoConnection;

    fn protocol(&self) -> Protocol {
        PingPongProtoMessage::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        direction: Direction,
        _peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state
            .events
            .send(ProtocolEvent::Established { direction, peer_id: _peer_id, to_connection: tx })
            .ok();
        PingPongProtoConnection {
            conn,
            initial_ping: direction.is_outgoing().then(PingPongProtoMessage::ping),
            commands: UnboundedReceiverStream::new(rx),
            pending_pong: None,
        }
    }
}

pub struct PingPongProtoConnection {
    conn: ProtocolConnection,
    initial_ping: Option<PingPongProtoMessage>,
    commands: UnboundedReceiverStream<Command>,
    pending_pong: Option<oneshot::Sender<String>>,
}

impl Stream for PingPongProtoConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(initial_ping) = this.initial_ping.take() {
            info!(">>>>>>>>> INITIAL PING {:?}", initial_ping);
            return Poll::Ready(Some(initial_ping.encoded()))
        }

        loop {
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                return match cmd {
                    Command::PingMessage { msg, response } => {
                        info!(">>>>>>>>> PING 1 {:?}", msg);
                        this.pending_pong = Some(response);
                        Poll::Ready(Some(PingPongProtoMessage::ping_message(msg).encoded()))
                    }
                }
            }
            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else { return Poll::Ready(None) };

            let Some(msg) = PingPongProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None)
            };

            match msg.message {
                PingPongProtoMessageKind::Ping => {
                    info!(">>>>>>>>> PING 2");
                    return Poll::Ready(Some(PingPongProtoMessage::pong().encoded()))
                }
                PingPongProtoMessageKind::Pong => {
                    info!(">>>>>>>>> PONG 2");
                }
                PingPongProtoMessageKind::PingMessage(msg) => {
                    info!(">>>>>>>>> PING MSG 2 {:?}", msg);
                    return Poll::Ready(Some(PingPongProtoMessage::pong_message(msg).encoded()))
                }
                PingPongProtoMessageKind::PongMessage(msg) => {
                    info!(">>>>>>>>> PONG MSG 2 {:?}", msg);
                    if let Some(sender) = this.pending_pong.take() {
                        sender.send(msg).ok();
                    }
                    continue
                }
            }

            return Poll::Pending
        }
    }
}
