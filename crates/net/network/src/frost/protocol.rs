#![allow(unreachable_pub)]
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

use crate::{
    frost::{messages::Request, EventResponseType, Response},
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
};

use super::{
    messages::{FrostProtoMessage, FrostProtoMessageKind},
    FrostPeerCommand, FrostProtocolEvent, ProtocolState,
};

/// Frost Protocol Handler
#[derive(Debug)]
pub struct FrostProtoHandler {
    /// The Frost Protocol State
    pub state: ProtocolState,
}

impl ProtocolHandler for FrostProtoHandler {
    type ConnectionHandler = FrostConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        // once the other side establishes conn with us, clone and send the sender half to them
        Some(FrostConnectionHandler { state: self.state.clone() })
    }

    /// Invoked when a new outgoing connection to the remote is requested.
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        // once I establish conn with the other peer, clone and send the sender half to them
        Some(FrostConnectionHandler { state: self.state.clone() })
    }
}

/// Frost Connection Handler
#[derive(Debug)]
pub struct FrostConnectionHandler {
    state: ProtocolState,
}

impl ConnectionHandler for FrostConnectionHandler {
    type Connection = FrostProtoConnection;

    /// Returns the protocol to announce when the RLPx connection will be established.
    ///
    /// This will be negotiated with the remote peer.
    fn protocol(&self) -> Protocol {
        FrostProtoMessage::protocol()
    }

    /// Invoked when the RLPx connection has been established by the peer does not share the
    /// protocol.
    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    /// Invoked when the RLPx connection was established.
    ///
    /// The returned future should resolve when the connection should disconnect.
    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        // on every new connection to us, send over the cloned shared channel an Established event
        // to the other side and a tx handle to send Command messages to us directly
        let (remote_peer_tx, remote_peer_rx) = mpsc::unbounded_channel();
        self.state
            .events
            .send(FrostProtocolEvent::ConnectionEstablished {
                direction,
                peer_id,
                to_connection: remote_peer_tx,
            })
            .ok();
        let peer_message_forwarder = self.state.peer_message_forwarder.clone();
        // update connection state
        FrostProtoConnection {
            peer_message_forwarder,
            conn,
            // incoming - another peer is connecting with me (set the ping message to Some),
            // outgoing - i am connecting with a peer
            initial_ping: direction.is_outgoing().then(|| {
                FrostProtoMessage::ping_message(self.state.peer_id, self.state.authority_index)
            }),
            // used to receive commands from me to send to the other peer
            commands: UnboundedReceiverStream::new(remote_peer_rx),
            pending_pong: None, // when the conn. is just esablished, there is no pending pong
            my_authority_index: self.state.authority_index,
            my_peer_id: self.state.peer_id,
        }
    }
}

/// Frost Protocol Connection
#[derive(Debug)]
pub struct FrostProtoConnection {
    peer_message_forwarder: mpsc::UnboundedSender<FrostProtocolEvent>,
    conn: ProtocolConnection,
    initial_ping: Option<FrostProtoMessage>,
    commands: UnboundedReceiverStream<FrostPeerCommand>,
    pending_pong: Option<oneshot::Sender<String>>,
    my_authority_index: u16,
    my_peer_id: PeerId,
}

impl Stream for FrostProtoConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // in case of outgoing (i am connecting with a peer), send him a pure PING message
        if let Some(initial_ping) = this.initial_ping.take() {
            //info!(
            //    ">>>>>>>>> SENDING GREETINGS PING TO PEER I AM CONNECTING WITH {:?}",
            //    initial_ping
            //);
            return Poll::Ready(Some(initial_ping.encoded()))
        }
        let peer_message_forwarder = this.peer_message_forwarder.clone();
        loop {
            // poll the commands send by us to another peer
            if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
                return match cmd {
                    // if I want to send a ping message, save the response channel to later (below)
                    // answer once the pong is received
                    FrostPeerCommand::PingMessage { msg: _, response } => {
                        //info!(">>>>>>>>> SENDING PING WITH MY AUTH INDEX {:?}", msg);
                        this.pending_pong = Some(response);
                        Poll::Ready(Some(
                            FrostProtoMessage::ping_message(
                                this.my_peer_id,
                                this.my_authority_index,
                            )
                            .encoded(),
                        ))
                    }
                    FrostPeerCommand::PeerMessage(response) => {
                        let Response { response_type, identifier, data } = response;
                        match response_type {
                            EventResponseType::DkgRound1 => {
                                let req = Request::new(identifier, data);
                                //info!(">>>>>>>>> [PROTOCOL] SENDING ROUND 1 DKG DATA = {:?}",
                                // req);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_dkg_message(req).encoded(),
                                ))
                            }
                            EventResponseType::DkgRound2 => {
                                let req = Request::new(identifier, data);
                                //info!(">>>>>>>>> [PROTOCOL] SENDING ROUND 2 DKG DATA = {:?}",
                                // req);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round2_dkg_message(req).encoded(),
                                ))
                            }
                        }
                    }
                }
            }

            // poll the actual conn to peers for events from other peers
            let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else { return Poll::Ready(None) };

            // if deserialization fails, skipp
            let Some(msg) = FrostProtoMessage::decode_message(&mut &msg[..]) else {
                return Poll::Ready(None)
            };

            // react on message type
            match msg.message {
                FrostProtoMessageKind::Ping => {
                    //info!(">>>>>>>>> RECEIVED PING FROM PEER. SENDING PONG...");
                    return Poll::Ready(Some(FrostProtoMessage::pong().encoded()))
                }
                FrostProtoMessageKind::Pong => {
                    //info!(">>>>>>>>> RECEIVED PONG FROM PEER. --.");
                }
                FrostProtoMessageKind::PingMessage(peer_id, authority_index) => {
                    //info!(
                    //    ">>>>>>>>> RECEIVED PING FROM PEER WITH HIS AUTH INDEX = {}. SENDING PONG
                    // WITH MY AUTH INDEX",    authority_index
                    //);

                    let _ = peer_message_forwarder
                        .send(FrostProtocolEvent::PeerConfirmed(peer_id, authority_index));

                    // answer with pong of my_authority_index
                    return Poll::Ready(Some(
                        FrostProtoMessage::pong_message(this.my_peer_id, this.my_authority_index)
                            .encoded(),
                    ))
                }
                // other peers answers with pong message with a peer id and authority index
                FrostProtoMessageKind::PongMessage(peer_id, authority_index) => {
                    //info!(
                    //    ">>>>>>>>> RECEIVED PONG FROM PEER WITH AUTH INDEX {}. CONFIRMING
                    // RECEIVED",    authority_index
                    //);

                    let _ = peer_message_forwarder
                        .send(FrostProtocolEvent::PeerConfirmed(peer_id, authority_index));
                    //info!(">>>>>>>>> FORWARDED.");

                    if let Some(sender) = this.pending_pong.take() {
                        sender.send("Confirmed".to_string()).ok();
                    }
                }
                FrostProtoMessageKind::Round1Dkg(data) => {
                    //info!(">>>>>>>>> [PROTOCOL] RECEIVED DKG 1 PACKAGE FROM PEER. {:?}", data);
                    let _ =
                        peer_message_forwarder.send(FrostProtocolEvent::PeerMessage(Response {
                            response_type: EventResponseType::DkgRound1,
                            identifier: data.identifier,
                            data: data.data,
                        }));
                }
                FrostProtoMessageKind::Round2Dkg(data) => {
                    //info!(">>>>>>>>> [PROTOCOL] RECEIVED DKG 2 PACKAGE FROM PEER. {:?}", data);
                    let _ =
                        peer_message_forwarder.send(FrostProtocolEvent::PeerMessage(Response {
                            response_type: EventResponseType::DkgRound2,
                            identifier: data.identifier,
                            data: data.data,
                        }));
                }
            }

            return Poll::Pending
        }
    }
}
