// The engine API supports a number of different operations
// Mainly: engine_exchangeCapabilities, engine_forkchoiceUpdatedV2, engine_getPayloadV2,
// engine_newPayloadV2 Interacting with the engine API can be done via RPC or the engine API shared
// queue

use reth_beacon_consensus::{BeaconEngineMessage, BeaconOnNewPayloadError, ForkchoiceStatus};
use reth_primitives::{BlockHash, SealedBlock, SealedHeader};
use reth_rpc_types::engine::{ForkchoiceState, PayloadStatus, PayloadStatusEnum};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use tracing::{debug, error};

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
    #[error("Response timeout")]
    Timeout,
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
    let mut ctr = 0;
    loop {
        if ctr == 5000 {
            return Err(SendForkChoiceUpdateError::Timeout)
        }
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
                    }
                    ForkchoiceStatus::Syncing => {
                        debug!(target: "consensus::authority", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                        // wait for the next fork choice update
                        continue
                    }
                }
            }
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Authority fork choice update failed");
                return Err(SendForkChoiceUpdateError::InvalidPayload)
            }
        }
        ctr += 1;
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{BlockBody, Header};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_send_fork_choice_update_payload_valid() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let header =
            SealedHeader { hash: Header::default().hash_slow(), header: Header::default() }; // Replace with actual values
        tokio::spawn(send_fork_choice_update_payload(header.clone(), tx.clone()));

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
}
