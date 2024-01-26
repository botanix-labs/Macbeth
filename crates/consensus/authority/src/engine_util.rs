// The engine API supports a number of different operations
// Mainly: engine_exchangeCapabilities, engine_forkchoiceUpdatedV2, engine_getPayloadV2,
// engine_newPayloadV2 Interacting with the engine API can be done via RPC or the engine API shared
// queue

use reth_beacon_consensus::{BeaconEngineMessage, BeaconOnNewPayloadError, ForkchoiceStatus};
use reth_payload_builder::error::PayloadBuilderError;
use reth_primitives::{revm_primitives::FixedBytes, BlockHash, SealedBlock, TransactionSigned};
use reth_rpc_types::engine::{
    ForkchoiceState, PayloadAttributes, PayloadId, PayloadStatus, PayloadStatusEnum,
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use tracing::{debug, error, trace};

#[derive(Debug, thiserror::Error)]
/// Error type for sending a new payload to the engine
pub(crate) enum SendNewPayloadError {
    #[error("Engine error")]
    EngineError,
    #[error("Engine returned invalid payload")]
    InvalidPayload(String),
    #[error("Engine recieve error")]
    RecvError,
    #[error("Beacon new payload error: {0}")]
    BeaconError(BeaconOnNewPayloadError),
}

/// Sends a new payload to the engine.
/// This function sends a new payload to the engine and waits for the response.
/// It handles different payload status scenarios and returns an error if the payload is invalid.
pub(crate) async fn send_beacon_new_payload(
    sealed_block: SealedBlock,
    to_engine: UnboundedSender<BeaconEngineMessage>,
) -> Result<PayloadStatus, SendNewPayloadError> {
    loop {
        let (tx, rx) = oneshot::channel();
        let payload = BeaconEngineMessage::NewPayload {
            payload: sealed_block.clone().into(),
            cancun_fields: None,
            tx,
        };
        to_engine.send(payload).map_err(|_| SendNewPayloadError::EngineError)?;
        let recv = rx.await.map_err(|_| SendNewPayloadError::RecvError)?;
        match recv {
            Ok(recv) => {
                match recv.status {
                    PayloadStatusEnum::Syncing => {
                        debug!(target: "consensus::authority", ?recv, "Authority fork new payload returned SYNCING, waiting for VALID");
                        // wait for the next fork choice update
                        continue
                    }
                    PayloadStatusEnum::Invalid { validation_error } => {
                        // wait for the next fork choice update
                        return Err(SendNewPayloadError::InvalidPayload(validation_error))
                    }
                    PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted => {
                        debug!(target: "consensus::authority", ?recv, "Authority fork new payload returned VALID");
                        return Ok(recv)
                    }
                }
            }
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Authority new payload failed");
                return Err(SendNewPayloadError::BeaconError(err))
            }
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
/// Error type for sending a new payload to the engine
pub(crate) enum SendForkChoiceUpdateError {
    #[error("Engine error")]
    EngineError,
    #[error("Engine returned invalid payload")]
    InvalidPayload,
    #[error("Engine recieve error")]
    RecvError,
}

/// Sends a FCU payload to the engine.
pub(crate) async fn send_fork_choice_update_payload(
    new_block_hash: BlockHash,
    to_engine: UnboundedSender<BeaconEngineMessage>,
) -> Result<(), SendForkChoiceUpdateError> {
    let state = ForkchoiceState {
        head_block_hash: new_block_hash,
        finalized_block_hash: new_block_hash,
        safe_block_hash: new_block_hash,
    };
    loop {
        // send the new update to the engine, this will trigger
        // the engine
        // to download and execute the block we just inserted
        let (tx, rx) = oneshot::channel();
        to_engine
            .send(BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs: None, tx })
            .map_err(|_| SendForkChoiceUpdateError::EngineError)?;

        let recv = rx.await.map_err(|_| SendForkChoiceUpdateError::RecvError)?;
        match recv {
            Ok(fcu_response) => {
                match fcu_response.forkchoice_status() {
                    ForkchoiceStatus::Valid => return Ok(()),
                    ForkchoiceStatus::Invalid => {
                        error!(target: "consensus::authority", ?fcu_response, "Forkchoice update returned invalid response");
                        // TODO(armins) maybe we should return the status here
                        return Ok(())
                    }
                    ForkchoiceStatus::Syncing => {
                        trace!(target: "consensus::authority", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                        // wait for the next fork choice update
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue
                    }
                }
            }
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Authority fork choice update failed");
                return Err(SendForkChoiceUpdateError::InvalidPayload)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
/// Error type for starting a new payload
pub(crate) enum StartNewPayloadError {
    #[error("Engine error")]
    EngineError,
    #[error("Engine recieve error")]
    RecvError,
    #[error("No payload error")]
    NoPayload(PayloadBuilderError),
}

/// Start a new payload job and returns the payload id if it exists.
///
/// This function creates a `BeaconEngineMessage::StartNewPayload` message and sends it to the
/// Beacon Engine. The payload id is returned if received successfully, otherwise an error is
/// logged and None is returned.
///
/// # Arguments
///
/// * `to_engine` - The sender to send the message to the Beacon Engine.
/// * `payload_attributes` - The payload attributes.
/// * `parent` - The parent block hash the payload will be built on.
pub(crate) async fn start_new_payload(
    to_engine: UnboundedSender<BeaconEngineMessage>,
    payload_attributes: PayloadAttributes,
    parent: FixedBytes<32>,
) -> Result<PayloadId, StartNewPayloadError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let result =
        to_engine.send(BeaconEngineMessage::StartNewPayload { payload_attributes, parent, tx });

    match result {
        Ok(_) => match rx.await {
            Ok(payload_id) => payload_id.map_err(|e| StartNewPayloadError::NoPayload(e)),
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Receiver error, channel closed");
                Err(StartNewPayloadError::RecvError)
            }
        },
        Err(e) => {
            error!(target: "consensus::authority", ?e, "Failed to send start new payload request");
            Err(StartNewPayloadError::EngineError)
        }
    }
}

#[derive(Debug, thiserror::Error)]
/// Error type for getting the best transactions from a payload
pub(crate) enum BestTransactionsError {
    #[error("Engine error")]
    EngineError,
    #[error("Engine recieve error")]
    RecvError,
    #[error("Empy payload error")]
    PayloadEmptyError,
}

/// Gets the best transactions from the payload with the given id.
///
/// This function creates a `BeaconEngineMessage::BestPayload` message and sends it to the
/// Beacon Engine. The best transactions are returned if received successfully,
/// otherwise an error is logged and None is returned.
///
/// # Arguments
/// * `to_engine` - The sender to send the message to the Beacon Engine.
/// * `payload_id` - The payload id to get the best transactions from.
pub(crate) async fn best_transactions_from_payload(
    to_engine: UnboundedSender<BeaconEngineMessage>,
    payload_id: PayloadId,
) -> Result<Vec<TransactionSigned>, BestTransactionsError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let result = to_engine.send(BeaconEngineMessage::BestPayload { tx, payload_id });

    match result {
        Ok(_) => match rx.await {
            Ok(payload) => {
                let payload = payload.map(|p| p.block().clone().body);
                payload.ok_or(BestTransactionsError::PayloadEmptyError)
            }
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to receive best payload");
                Err(BestTransactionsError::RecvError)
            }
        },
        Err(e) => {
            error!(target: "consensus::authority", ?e, "Failed to send best payload request");
            Err(BestTransactionsError::EngineError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{Address, BlockBody, Header, SealedHeader};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_send_fork_choice_update_payload_valid() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let header =
            SealedHeader { hash: Header::default().hash_slow(), header: Header::default() }; // Replace with actual values
        tokio::spawn(send_fork_choice_update_payload(header.hash, tx.clone()));

        // Ensure that the engine received the message
        let msg = rx.recv().await.unwrap();
        match msg {
            BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                assert_eq!(state.head_block_hash, header.hash);
                assert_eq!(state.finalized_block_hash, header.hash);
                assert_eq!(state.safe_block_hash, header.hash);
                assert!(payload_attrs.is_none());
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[tokio::test]
    async fn test_send_new_payload_valid() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let block_body = BlockBody::default();
        let header =
            SealedHeader { hash: Header::default().hash_slow(), header: Header::default() };
        let sealed_block = SealedBlock::new(header, block_body);
        tokio::spawn(send_beacon_new_payload(sealed_block.clone(), tx.clone()));

        // Ensure that the engine received the message
        let msg = rx.recv().await.unwrap();
        match msg {
            BeaconEngineMessage::NewPayload { payload, cancun_fields, tx } => {
                assert_eq!(payload.block_hash(), sealed_block.hash());
                assert_eq!(cancun_fields, None);
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[tokio::test]
    async fn test_start_new_payload() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let payload_attributes = PayloadAttributes {
            timestamp: 0,
            prev_randao: FixedBytes::default(),
            suggested_fee_recipient: Address::default(),
            withdrawals: None,
            parent_beacon_block_root: None,
        };
        let parent = FixedBytes::default();
        tokio::spawn(start_new_payload(tx.clone(), payload_attributes, parent));

        // Ensure that the engine received the message
        let msg = rx.recv().await.unwrap();
        match msg {
            BeaconEngineMessage::StartNewPayload { payload_attributes, parent, tx } => {
                assert_eq!(payload_attributes, payload_attributes);
                assert_eq!(parent, parent);
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[tokio::test]
    async fn test_get_best_transactions_from_payload() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let payload_id = PayloadId::new([0; 8]);
        tokio::spawn(best_transactions_from_payload(tx.clone(), payload_id));

        // Ensure that the engine received the message
        let msg = rx.recv().await.unwrap();
        match msg {
            BeaconEngineMessage::BestPayload { tx, payload_id } => {
                assert_eq!(payload_id, payload_id);
            }
            _ => panic!("Unexpected message type"),
        }
    }
}
