#![allow(unreachable_pub)]
use futures::{FutureExt, Stream, StreamExt};
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
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::PollSender;
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
    messages::{FrostProtoMessage, FrostProtoMessageKind, HealthcheckRequest, SignRequest},
    FrostPeerCommand, FrostProtocolEvent, HealthcheckResponse, PeerMessageResponse,
};

/// Frost Protocol Handler
#[derive(Debug)]
pub struct FrostProtoHandler {
    /// My peer id
    pub my_peer_id: PeerId,
    /// Channel to send protocol events to the manager (Conn established/confirmed), peer message
    /// command
    pub protocol_events_tx: mpsc::Sender<FrostProtocolEvent>,
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
    protocol_events_tx: mpsc::Sender<FrostProtocolEvent>,
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

        let protocol_events_tx = PollSender::new(self.protocol_events_tx.clone());
        // update connection state
        FrostProtoConnection {
            protocol_events_tx,
            registration: RegistrationState::NotRegistered,
            conn_rx: conn,
            // Used to receive commands from me to be sent to the other peer.
            // Initialized once the connection can be registered.
            commands_rx: None,
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
    protocol_events_tx: PollSender<FrostProtocolEvent>,
    registration: RegistrationState,
    /// Channel to receive messages from other peers on the wire
    conn_rx: ProtocolConnection,
    /// Channel to receive commands from in the internal application to send to the other peers
    commands_rx: Option<UnboundedReceiverStream<FrostPeerCommand>>,
    /// My peer id
    my_peer_id: PeerId,
    /// Remote peer id
    peer_id: PeerId,
    /// direction of the connection
    #[allow(dead_code)]
    direction: Direction,
    pending_pong: Option<oneshot::Sender<String>>,
}

#[derive(Debug)]
// TODO(lamafab): implement a `Closed` variant?
enum RegistrationState {
    NotRegistered,
    Pending {
        remote_peer_rx: mpsc::UnboundedReceiver<FrostPeerCommand>,
        callback_rx: oneshot::Receiver<u64>,
    },
    Registered(u64),
}

impl FrostProtoConnection {
    fn reservation_guard(&mut self) -> SlotReservationGuard {
        SlotReservationGuard { protocol_events_tx: self.protocol_events_tx.clone() }
    }
}

/// Guard to ensure that the reserved slot in the events channel is released
/// once dropped.
struct SlotReservationGuard {
    protocol_events_tx: PollSender<FrostProtocolEvent>,
}

impl Drop for SlotReservationGuard {
    fn drop(&mut self) {
        self.protocol_events_tx.abort_send();
    }
}

impl Drop for FrostProtoConnection {
    fn drop(&mut self) {
        info!(target: "network::frost::protocol", "Dropping FrostProtoConnection for peer with id = {:?}", self.peer_id);

        let RegistrationState::Registered(idx) = self.registration else {
            // connection hasn't been registered yet
            return;
        };

        let event = FrostProtocolEvent::ConnectionClosed { idx };

        // try to let the FROST manager know about the connection termination so
        // the cleanup can be triggered. This can fail if the event channel is
        // full - the FROST manager does still have an implicit garbage
        // collection mechanism that will remove the dead connection after a
        // while.
        let res = self
            .protocol_events_tx
            .get_ref()
            .expect("poll sender closed unexpectedly")
            .try_send(event);

        if res.is_err() {
            warn!(target: "network::frost::protocol", "Failed to notify FROST manager about connection drop for peer with id = {:?}, connection idx = {}", self.peer_id, idx);
        }
    }
}

impl Stream for FrostProtoConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // on every new connection to us, we send an Established event to the FROST manager
        // and a tx handle to send Command messages to us directly.
        match &mut this.registration {
            RegistrationState::NotRegistered => {
                // reserve a slot for the connection registration event.
                if let Err(e) = ready!(this.protocol_events_tx.poll_reserve(cx)) {
                    // this error only occurs if the FROST manager has been dropped.
                    error!(target: "network::frost::protocol", "Failed to reserve slot in the events channel: {:?}", e);
                    return Poll::Ready(None);
                }

                let (remote_peer_tx, remote_peer_rx) = mpsc::unbounded_channel();
                let (callback_tx, mut callback_rx) = oneshot::channel();

                let connection_established_event = FrostProtocolEvent::ConnectionEstablished {
                    direction: this.direction,
                    peer_id: this.peer_id,
                    peer_commands_tx: remote_peer_tx,
                    sender: callback_tx,
                };

                if let Err(e) = this.protocol_events_tx.send_item(connection_established_event) {
                    // this error only occurs if the FROST manager has been dropped.
                    error!(target: "network::frost::protocol", "Failed to send ConnectionEstablished event: {:?}", e.to_string());
                    return Poll::Ready(None);
                }

                // Wait for the FROST manager to assign an idx to this connection.
                match callback_rx.poll_unpin(cx) {
                    Poll::Ready(Ok(idx)) => {
                        // connection was registered immediately and
                        // successfully (unlikely that actually happens in
                        // practice)
                        this.registration = RegistrationState::Registered(idx);
                        this.commands_rx = Some(UnboundedReceiverStream::new(remote_peer_rx));
                    }
                    Poll::Ready(Err(e)) => {
                        // this error only occurs if the FROST manager has been dropped.
                        error!(target: "network::frost::protocol", "Failed to send ConnectionEstablished event: {:?}", e.to_string());
                        return Poll::Ready(None);
                    }
                    Poll::Pending => {
                        // stage pending state, wait for callback to be resolved
                        this.registration =
                            RegistrationState::Pending { remote_peer_rx, callback_rx };
                        return Poll::Pending;
                    }
                }
            }
            RegistrationState::Pending { .. } => {
                // take ownership of data
                let RegistrationState::Pending { remote_peer_rx, mut callback_rx } =
                    std::mem::replace(&mut this.registration, RegistrationState::NotRegistered)
                else {
                    panic!("checked above")
                };

                match callback_rx.poll_unpin(cx) {
                    Poll::Ready(Ok(idx)) => {
                        // connection was registered successfully
                        this.registration = RegistrationState::Registered(idx);
                        this.commands_rx = Some(UnboundedReceiverStream::new(remote_peer_rx));
                    }
                    Poll::Ready(Err(e)) => {
                        error!(target: "network::frost::protocol", "Failed to send ConnectionEstablished event: {:?}", e.to_string());
                        return Poll::Ready(None);
                    }
                    Poll::Pending => {
                        // stage pending state, wait for callback to be resolved
                        this.registration =
                            RegistrationState::Pending { remote_peer_rx, callback_rx };

                        return Poll::Pending;
                    }
                }
            }
            RegistrationState::Registered(_) => { /* connection is already registered */ }
        }

        let commands_rx = this.commands_rx.as_mut().expect("commands_rx is not initialized");

        // poll the commands sent by us to send to another peer
        // TODO(lamafab): should we shutdown the connection if the commands_rx is dropped?
        if let Poll::Ready(Some(cmd)) = commands_rx.poll_next_unpin(cx) {
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
                                let req = DkgRequest::new(data, identifier);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_dkg_request_message(req).encoded(),
                                ))
                            }
                            DkgEventResponseType::DkgRound1 => {
                                let req = DkgRequest::new(data, identifier);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_dkg_message(req).encoded(),
                                ))
                            }
                            DkgEventResponseType::DkgRound2 => {
                                let req = DkgRequest::new(data, identifier);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round2_dkg_message(req).encoded(),
                                ))
                            }
                        }
                    }
                    PeerMessageResponse::Signing(signing_response) => {
                        let SigningResponse { response_type, signing_session_id, psbt } =
                            signing_response;
                        match response_type {
                            SigningEventResponseType::SignerRound1SigningPackage => {
                                let req = SignRequest::new(signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_signer_package_message(req).encoded(),
                                ))
                            }
                            SigningEventResponseType::CoordinatorRound1SigningPackage => {
                                let req = SignRequest::new(signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round1_coordinator_signing_package_message(
                                        req,
                                    )
                                    .encoded(),
                                ))
                            }
                            SigningEventResponseType::SignerRound2SigningPackage => {
                                let req = SignRequest::new(signing_session_id, psbt);
                                Poll::Ready(Some(
                                    FrostProtoMessage::round2_signer_package_message(req).encoded(),
                                ))
                            }
                            SigningEventResponseType::CoordinatorRound2SigningPackage => {
                                let req = SignRequest::new(signing_session_id, psbt);
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
                        let WalletStateResponse { utxos, tracked_txs, pending_pegouts } =
                            wallet_state_response;
                        let req = WalletStateRequest::new(utxos, tracked_txs, pending_pegouts);
                        Poll::Ready(Some(FrostProtoMessage::wallet_state_message(req).encoded()))
                    }
                },
            };
        }

        // before we even start processing the incoming messages from the other
        // peer, we first make sure that the FROST manager has enough capacity
        // by reserving a slot in the bounded events channel.
        //
        // The Waker will be woken up once the event channel has enough capacity
        // again.
        if let Err(e) = ready!(this.protocol_events_tx.poll_reserve(cx)) {
            // this error only occurs if the FROST manager has been dropped.
            error!(target: "network::frost::protocol", "Failed to reserve slot in the events channel: {:?}", e);
            return Poll::Ready(None);
        }

        // this drop guard releases the reserved slot automatically when this
        // function returns (early), including for any pending states. This has
        // no effect if a message actually gets sent to the FROST manager.
        let _guard = this.reservation_guard();

        let msg;
        loop {
            // poll the actual conn to peers for events from this other peer
            let Some(bytes) = ready!(this.conn_rx.poll_next_unpin(cx)) else {
            return Poll::Ready(None);
        };

            match FrostProtoMessage::decode_message(&mut &bytes[..]) {
                Some(m) => {
                    msg = m;
                    break;
                }
                None => {
                    warn!(target: "network::frost::protocol", "Failed to decode frost protocol message");
                    // drop this invalid message and continue the loop to poll the conn again.
                    continue;
                }
            }
        }

        // The frost manager will handle this message (often by forwarding it to
        // another task) and the response will be sent on command_rx for us to
        // send back to another peer.
        let protocol_event = match msg.message {
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
                        utxos: data.utxos,
                        tracked_txs: data.tracked_txs,
                        pending_pegouts: data.pending_pegouts,
                    }),
                    peer_id: this.peer_id,
            },
        };

        if let Err(e) = this.protocol_events_tx.send_item(protocol_event) {
            // this error only occurs if the FROST manager has been dropped.
            error!(target: "network::frost::protocol", "Failed to send protocol event: {:?}", e.to_string());
            return Poll::Ready(None);
        }

        Poll::Pending
    }
}
