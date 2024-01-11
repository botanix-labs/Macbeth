// The engine API supports a number of different operations
// Mainly: engine_exchangeCapabilities, engine_forkchoiceUpdatedV2, engine_getPayloadV2,
// engine_newPayloadV2 Interacting with the engine API can be done via RPC or the engine API shared
// queue

use reth_beacon_consensus::BeaconEngineMessage;
use reth_primitives::{SealedBlock, SealedHeader};
use reth_rpc_types::engine::{PayloadStatus, PayloadStatusEnum};
use tokio::sync::mpsc::UnboundedSender;

use tracing::debug;

#[derive(Debug, thiserror::Error)]
/// Error type for sending a new payload to the engine
pub enum SendNewPayloadError {
    #[error("Engine error: {0}")]
    EngineError,
    #[error("Engine returned invalid payload")]
    InvalidPayload(String),
    #[error("Engine recieve error: {0}")]
    RecvError,
}

/// Sends a new payload to the engine.
/// This function sends a new payload to the engine and waits for the response.
/// It handles different payload status scenarios and returns an error if the payload is invalid.
pub async fn send_beacon_new_payload(
    sealed_block: SealedBlock,
    to_engine: UnboundedSender<BeaconEngineMessage>,
) -> Result<(), SendNewPayloadError> {
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
            PayloadStatusEnum::Syncing => {
                debug!(target: "consensus::authority", ?payload_status, "Authority fork new payload returned SYNCING, waiting for VALID");
                // wait for the next fork choice update
                continue
            }
            PayloadStatusEnum::Invalid { validation_error } => {
                debug!(target: "consensus::authority", ?payload_status, "Authority fork new payload returned INVALID, waiting for VALID");
                // wait for the next fork choice update
                return Err(SendNewPayloadError::InvalidPayload(validation_error))
            }
            PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted => {
                debug!(target: "consensus::authority", ?payload_status, "Authority fork new payload returned VALID");
                return Ok(())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
/// Error type for sending a new payload to the engine
pub enum SendForkChoiceUpdateError {
    #[error("Engine error: {0}")]
    EngineError,
    #[error("Engine returned invalid payload")]
    InvalidPayload,
    #[error("Engine recieve error: {0}")]
    RecvError,
}

/// Sends a FCU payload to the engine.
pub async fn send_fork_choice_update_payload(
    new_header: SealedHeader,
    to_engine: UnboundedSender<BeaconEngineMessage>,
) -> Result<(), SendForkChoiceUpdateError> {
    let state = ForkchoiceState {
        head_block_hash: new_header.hash,
        finalized_block_hash: new_header.hash,
        safe_block_hash: new_header.hash,
    };
    loop {
        // send the new update to the engine, this will trigger
        // the engine
        // to download and execute the block we just inserted
        let (tx, rx) = oneshot::channel();
        to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
            state,
            payload_attrs: None,
            tx,
        }).map_err(|_| SendForkChoiceUpdateError::EngineError)?;

        let recv = rx.await.map_err(|_| SendNewPayloadError::RecvError)?;

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
    }
}
