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
use tracing::{error, info};

use crate::{
    frost::{
        messages::{DkgRequest, PbftRequest},
        DkgEventResponseType, DkgResponse, SigningEventResponseType, SigningResponse,
    },
    protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler},
};

use super::{
    messages::{
        FrostProtoMessage, FrostProtoMessageKind, HealthcheckRequest, SignRequest, UtxoRequest,
    },
    FrostPeerCommand, FrostProtocolEvent, HealthcheckResponse, PbftEventResponseType, PbftResponse,
    PeerMessageResponse, ProtocolState, UtxoSetResponse,
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
        // TODO (armin) constant time?
        if !self.state.authorities.contains(self.state.network_handle.peer_id()) {
            return None;
        }
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
        // TODO (armin) constant time?
        if !self.state.authorities.contains(self.state.network_handle.peer_id()) {
            return None;
        }
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
            // outgoing - I am connecting with a peer
            initial_ping: direction
                .is_outgoing()
                .then(|| FrostProtoMessage::ping_message(*self.state.network_handle.peer_id())),
            // used to receive commands from me to send to the other peer
            commands: UnboundedReceiverStream::new(remote_peer_rx),
            pending_pong: None, // when the conn. is just established, there is no pending pong
            my_peer_id: *self.state.network_handle.peer_id(),
            peer_id,
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
    my_peer_id: PeerId,
    peer_id: PeerId,
}

impl Stream for FrostProtoConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // in case of outgoing (i am connecting with a peer), send him a pure PING message
        if let Some(initial_ping) = this.initial_ping.take() {
            return Poll::Ready(Some(initial_ping.encoded()));
        }
        let peer_message_forwarder = this.peer_message_forwarder.clone();
        // poll the commands send by us to another peer
        if let Poll::Ready(Some(cmd)) = this.commands.poll_next_unpin(cx) {
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
                    PeerMessageResponse::Pbft(pbft_response) => {
                        let PbftResponse { response_type, data } = pbft_response;
                        let req = PbftRequest::new(data);
                        match response_type {
                            PbftEventResponseType::CoordinatorBlockProposal => {
                                info!(
                                    target: "network::frost::protocol", "sending PBFT coordinator block proposal"
                                );
                                Poll::Ready(Some(
                                    FrostProtoMessage::coordinator_block_proposal_message(req)
                                        .encoded(),
                                ))
                            }
                            PbftEventResponseType::PeerPreCommitment => {
                                info!(target: "network::frost::protocol", "sending PBFT peer pre-commitment");
                                Poll::Ready(Some(
                                    FrostProtoMessage::peer_pre_commitment_message(req).encoded(),
                                ))
                            }
                            PbftEventResponseType::PeerCommitment => {
                                info!(target: "network::frost::protocol", "sending PBFT peer commitment");
                                Poll::Ready(Some(
                                    FrostProtoMessage::peer_commit_message(req).encoded(),
                                ))
                            }
                        }
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
                    PeerMessageResponse::Utxo(utxo_response) => {
                        let UtxoSetResponse { data } = utxo_response;
                        let req = UtxoRequest::new(data);
                        Poll::Ready(Some(FrostProtoMessage::utxo_message(req).encoded()))
                    }
                },
            };
        }

        // poll the actual conn to peers for events from other peers
        let Some(msg) = ready!(this.conn.poll_next_unpin(cx)) else { return Poll::Ready(None) };

        // if deserialization fails, skip
        let Some(msg) = FrostProtoMessage::decode_message(&mut &msg[..]) else {
            return Poll::Ready(None);
        };

        // react on message type sent to us by another peer
        match msg.message {
            FrostProtoMessageKind::Healthcheck(data) => {
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
            FrostProtoMessageKind::PingMessage(peer_id) => {
                info!(target: "network::frost::protocol", "Received ping message from peer with id = {:?} Replying with pong...", peer_id);
                if let Err(e) =
                    peer_message_forwarder.send(FrostProtocolEvent::PeerConfirmed(peer_id))
                {
                    error!(target: "network::frost::protocol", "Failed to forward received pong message from peer id {:?}. Error = {:?}", peer_id, e);
                }

                // answer with pong of my_authority_index
                return Poll::Ready(Some(
                    FrostProtoMessage::pong_message(this.my_peer_id).encoded(),
                ));
            }
            // other peers answers with pong message with a peer id and authority index
            FrostProtoMessageKind::PongMessage(peer_id) => {
                if let Err(e) =
                    peer_message_forwarder.send(FrostProtocolEvent::PeerConfirmed(peer_id))
                {
                    error!(target: "network::frost::protocol", "Failed to forward received PongMessage from peer id {:?}. Error = {:?}", peer_id, e);
                }

                if let Some(sender) = this.pending_pong.take() {
                    sender.send("Confirmed".to_string()).ok();
                }
            }
            FrostProtoMessageKind::CoordinatorBlockProposal(data) => {
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
                    peer_id: this.peer_id,
                    response: PeerMessageResponse::Pbft(PbftResponse {
                        response_type: PbftEventResponseType::CoordinatorBlockProposal,
                        data: data.block,
                    }),
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received CoordinatorBlockProposal message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::PeerPreCommitment(data) => {
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
                    peer_id: this.peer_id,
                    response: PeerMessageResponse::Pbft(PbftResponse {
                        response_type: PbftEventResponseType::PeerPreCommitment,
                        data: data.block,
                    }),
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received PeerPreCommitment message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::PeerCommit(data) => {
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
                    peer_id: this.peer_id,
                    response: PeerMessageResponse::Pbft(PbftResponse {
                        response_type: PbftEventResponseType::PeerCommitment,
                        data: data.block,
                    }),
                }) {
                    error!(target: "network::frost::protocol", "Failed to forward received PeerCommit message. Error = {:?}", e);
                }
            }
            FrostProtoMessageKind::Round1Dkg(data) => {
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
                if let Err(e) = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
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
            FrostProtoMessageKind::Utxo(data) => {
                let _ = peer_message_forwarder.send(FrostProtocolEvent::PeerMessage {
                    response: PeerMessageResponse::Utxo(UtxoSetResponse { data: data.data }),
                    peer_id: this.peer_id,
                });
            }
        }

        Poll::Pending
    }
}
