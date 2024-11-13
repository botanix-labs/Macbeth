#![allow(unreachable_pub)]
use futures::{Stream, StreamExt};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network_api::Direction;
use reth_network_peers::PeerId;
use reth_primitives::BytesMut;
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info, warn};

use crate::{
    frost::{
        messages::{DkgRequest, WalletStateRequest},
        DkgEventResponseType, DkgResponse, SigningEventResponseType, SigningResponse,
        WalletStateResponse,
    },
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
};

use super::{
    messages::{
        FrostProtoMessage, FrostProtoMessageKind, HealthcheckRequest, SignRequest, UtxoRequest,
    },
    FrostPeerCommand, FrostProtocolEvent, HealthcheckResponse, PeerMessageResponse,
};

/// Frost Protocol Handler
#[derive(Debug)]
pub struct FrostProtoHandler {
    /// My peer id
    pub my_peer_id: PeerId,
    /// Channel to send protocol events to the manager (Conn established/confirmed), peer message
    /// command
    pub protocol_events_tx: broadcast::Sender<FrostProtocolEvent>,
}

impl ProtocolHandler for FrostProtoHandler {
    type ConnectionHandler = FrostConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        // once the other side establishes conn with us, clone and send the sender half to them
        Some(FrostConnectionHandler {
            my_peer_id: self.my_peer_id,
            protocol_events_tx: self.protocol_events_tx.clone(),
        })
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
        Some(FrostConnectionHandler {
            my_peer_id: self.my_peer_id,
            protocol_events_tx: self.protocol_events_tx.clone(),
        })
    }
}

/// Frost Connection Handler
#[derive(Debug)]
pub struct FrostConnectionHandler {
    /// My peer id
    my_peer_id: PeerId,
    /// Channel to send protocol events to the manager (Conn established/confirmed), peer message
    /// command
    protocol_events_tx: broadcast::Sender<FrostProtocolEvent>,
}

impl ConnectionHandler for FrostConnectionHandler {
    type Connection = FrostProtoConnection;

    /// Returns the protocol to announce when the `RLPx` connection will be established.
    ///
    /// This will be negotiated with the remote peer.
    fn protocol(&self) -> Protocol {
        FrostProtoMessage::protocol()
    }

    /// Invoked when the `RLPx` connection has been established by the peer does not share the
    /// protocol.
    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    /// Invoked when the `RLPx` connection was established.
    ///
    /// The returned future should resolve when the connection should disconnect.
    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        info!(target: "network::frost::protocol::into_connection", "Establishing connection with peer with id = {:?}, direction = {:?}", peer_id, direction);
        // on every new connection to us, send over the cloned shared channel an Established event
        // to the other side and a tx handle to send Command messages to us directly
        let (remote_peer_tx, remote_peer_rx) = mpsc::unbounded_channel();
        let connection_established_event = FrostProtocolEvent::ConnectionEstablished {
            direction,
            peer_id,
            peer_commands_tx: remote_peer_tx,
        };

        if let Err(e) = self.protocol_events_tx.send(connection_established_event) {
            error!(target: "network::frost::protocol::into_connection", "Failed to send ConnectionEstablished event: {:?}", e.to_string());
        }

        let protocol_events_tx = self.protocol_events_tx.clone();
        // update connection state
        FrostProtoConnection {
            protocol_events_tx,
            conn_rx: conn,
            // incoming - another peer is connecting with me (set the ping message to Some),
            // outgoing - I am connecting with a peer
            initial_ping: direction
                .is_outgoing()
                .then(|| FrostProtoMessage::ping_message(self.my_peer_id)),
            // used to receive commands from me to send to the other peer
            commands_rx: UnboundedReceiverStream::new(remote_peer_rx),
            pending_pong: None, // when the conn. is just established, there is no pending pong
            my_peer_id: self.my_peer_id,
            peer_id,
            direction,
        }
    }
}

/// Frost Protocol Connection
#[derive(Debug)]
pub struct FrostProtoConnection {
    /// Channel to send protocol events to the manager (Conn established/confirmed), peer message
    /// command
    protocol_events_tx: broadcast::Sender<FrostProtocolEvent>,
    /// Channel to receive messages from other peers on the wire
    conn_rx: ProtocolConnection,
    /// Channel to receive commands from in the internal application to send to the other peers
    commands_rx: UnboundedReceiverStream<FrostPeerCommand>,
    /// My peer id
    my_peer_id: PeerId,
    /// Remote peer id
    peer_id: PeerId,
    /// direction of the connection
    #[allow(dead_code)]
    direction: Direction,
    #[allow(dead_code)]
    initial_ping: Option<FrostProtoMessage>,
    pending_pong: Option<oneshot::Sender<String>>,
}

impl Drop for FrostProtoConnection {
    fn drop(&mut self) {
        info!(target: "network::frost::protocol", "Dropping FrostProtoConnection for peer with id = {:?}", self.peer_id);
    }
}

impl Stream for FrostProtoConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        info!(target: "network::frost::protocol", "Polling next message from peer with id = {:?}", this.peer_id);

        // in case of outgoing (I am dialing this a peer), send him a pure PING message
        // TODO there is no reason to have this initial ping and pong maybe we can just remove it
        // if let Some(initial_ping) = this.initial_ping.take() {
        //     return Poll::Ready(Some(initial_ping.encoded()));
        // }

        // poll the commands sent by us to send to another peer
        if let Poll::Ready(Some(cmd)) = this.commands_rx.poll_next_unpin(cx) {
            info!(target: "network::frost::protocol", "Received command: {:?}", cmd);
            return match cmd {
                // if I want to send a ping message, save the response channel to later (below)
                // answer once the pong is received
                FrostPeerCommand::PingMessage { msg: _, response } => {
                    this.pending_pong = Some(response);
                    Poll::Ready(Some(FrostProtoMessage::ping_message(this.my_peer_id).encoded()))
                }
                FrostPeerCommand::PeerMessage(response) => match response {
                    PeerMessageResponse::Healthcheck(healthcheck_response) => {
                        let HealthcheckResponse { sender, receiver } = healthcheck_response;
                        let req = HealthcheckRequest::new(sender, receiver);

                        Poll::Ready(Some(FrostProtoMessage::peer_health_message(req).encoded()))
                    }
                    PeerMessageResponse::Dkg(dkg_response) => {
                        let DkgResponse { response_type, identifier, data } = dkg_response;
                        match response_type {
                            DkgEventResponseType::DkgRound1Request => {
                                let req = DkgRequest::new(identifier, data);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_dkg_request_message(req).encoded(),
                                ))
                            }
                            DkgEventResponseType::DkgRound1 => {
                                let req = DkgRequest::new(identifier, data);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_dkg_message(req).encoded(),
                                ))
                            }
                            DkgEventResponseType::DkgRound2 => {
                                let req = DkgRequest::new(identifier, data);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round2_dkg_message(req).encoded(),
                                ))
                            }
                        }
                    }
                    PeerMessageResponse::Signing(signing_response) => {
                        let SigningResponse { response_type, identifier, signing_session_id, psbt } =
                            signing_response;
                        match response_type {
                            SigningEventResponseType::SignerRound1SigningPackage => {
                                let req = SignRequest::new(identifier, signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_signer_package_message(req).encoded(),
                                ))
                            }
                            SigningEventResponseType::CoordinatorRound1SigningPackage => {
                                let req = SignRequest::new(identifier, signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_coordinator_signing_package_message(
                                        req,
                                    )
                                    .encoded(),
                                ))
                            }
                            SigningEventResponseType::SignerRound2SigningPackage => {
                                let req = SignRequest::new(identifier, signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round2_signer_package_message(req).encoded(),
                                ))
                            }
                            SigningEventResponseType::CoordinatorRound2SigningPackage => {
                                let req = SignRequest::new(identifier, signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round2_coordinator_signing_package_message(
                                        req,
                                    )
                                    .encoded(),
                                ))
                            }
                        }
                    }
                    PeerMessageResponse::WalletState(wallet_state_response) => {
                        let WalletStateResponse { data } = wallet_state_response;
                        let req = WalletStateRequest::new(data);
                        Poll::Ready(Some(FrostProtoMessage::wallet_state_message(req).encoded()))
                    }
                },
            };
        }

        // poll the actual conn to peers for events from this other peer
        let Some(msg) = ready!(this.conn_rx.poll_next_unpin(cx)) else {
            return Poll::Ready(None);
        };

        // if deserialization fails, skip
        let Some(msg) = FrostProtoMessage::decode_message(&mut &msg[..]) else {
            warn!(target: "network::frost::protocol", "Failed to decode frost protocol message");
            return Poll::Ready(None);
        };

        // react on message type sent to us by another peer
        // The frost manager will handle this req (often by forwarding it to another task) and
        // the response will be sent on command_rx for us to send back to another
        // peer
        let protocol_events_tx = this.protocol_events_tx.clone();
        info!(target: "network::frost::protocol", "Receivers count: {}", protocol_events_tx.receiver_count());
        match msg.message {
            FrostProtoMessageKind::Healthcheck(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    peer_id: this.peer_id,
                    response: PeerMessageResponse::Healthcheck(HealthcheckResponse {
                        receiver: data.receiver,
                        sender: data.sender,
                    }),
                }) {
                    error!(target: "network::frost::protocol", "Failed to send healthcheck message {:?}. Error = {:?}", data, e);
                }
            }
            FrostProtoMessageKind::Ping => {
                info!(target: "network::frost::protocol", "Received ping message from peer. Replying with pong...");
                return Poll::Ready(Some(FrostProtoMessage::pong().encoded()));
            }
            FrostProtoMessageKind::Pong => {}
            FrostProtoMessageKind::PingMessage(_peer_id) => {
                // answer with pong and my peer id
                return Poll::Ready(Some(
                    FrostProtoMessage::pong_message(this.my_peer_id).encoded(),
                ));
            }
            // other peers answers with pong message with a peer id and authority index
            FrostProtoMessageKind::PongMessage(_peer_id) => {
                if let Some(sender) = this.pending_pong.take() {
                    sender.send("Confirmed".to_string()).ok();
                }
            }
            FrostProtoMessageKind::Round1Dkg(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Dkg(DkgResponse {
                        response_type: DkgEventResponseType::DkgRound1,
                        identifier: data.identifier,
                        data: data.data,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received Round1Dkg message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::Round1DkgRequest(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Dkg(DkgResponse {
                        response_type: DkgEventResponseType::DkgRound1Request,
                        identifier: data.identifier,
                        data: data.data,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received Round1DkgRequest message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::Round2Dkg(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Dkg(DkgResponse {
                        response_type: DkgEventResponseType::DkgRound2,
                        identifier: data.identifier,
                        data: data.data,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received Round2Dkg message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::SignerRound1SigningPackage(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Signing(SigningResponse {
                        response_type: SigningEventResponseType::SignerRound1SigningPackage,
                        identifier: data.identifier,
                        signing_session_id: data.signing_session_id,
                        psbt: data.psbt,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received SignerRound1SigningPackage message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::CoordinatorRound1SigningPackage(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Signing(SigningResponse {
                        response_type: SigningEventResponseType::CoordinatorRound1SigningPackage,
                        identifier: data.identifier,
                        signing_session_id: data.signing_session_id,
                        psbt: data.psbt,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received CoordinatorRound1SigningPackage message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::SignerRound2SigningPackage(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Signing(SigningResponse {
                        response_type: SigningEventResponseType::SignerRound2SigningPackage,
                        identifier: data.identifier,
                        signing_session_id: data.signing_session_id,
                        psbt: data.psbt,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received SignerRound2SigningPackage message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::CoordinatorRound2SigningPackage(data) => {
                if let Err(e) = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Signing(SigningResponse {
                        response_type: SigningEventResponseType::CoordinatorRound2SigningPackage,
                        identifier: data.identifier,
                        signing_session_id: data.signing_session_id,
                        psbt: data.psbt,
                    }),
                    peer_id: this.peer_id,
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received CoordinatorRound2SigningPackage message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::WalletState(data) => {
                let _ = protocol_events_tx.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::WalletState(WalletStateResponse {
                        data: data.data,
                    }),
                    peer_id: this.peer_id,
                });
            }
            // deprecated: TODO remove
            FrostProtoMessageKind::Utxo(_data) => {}
        }

        Poll::Pending
    }
}
