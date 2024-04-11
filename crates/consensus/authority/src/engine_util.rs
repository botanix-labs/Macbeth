// The engine API supports a number of different operations
// Mainly: engine_exchangeCapabilities, engine_forkchoiceUpdatedV2, engine_getPayloadV2,
// engine_newPayloadV2 Interacting with the engine API can be done via RPC or the engine API shared
// queue

use reth_beacon_consensus::{BeaconEngineMessage, BeaconOnNewPayloadError, ForkchoiceStatus};
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::{
    error::PayloadBuilderError, EthBuiltPayload, EthPayloadBuilderAttributes, PayloadBuilderHandle,
};
use reth_primitives::{BlockHash, SealedBlock};
use reth_rpc_types::engine::{ForkchoiceState, PayloadId, PayloadStatus, PayloadStatusEnum};
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
pub(crate) async fn send_beacon_new_payload<Engine: reth_node_api::EngineTypes>(
    sealed_block: SealedBlock,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
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
                        continue;
                    }
                    PayloadStatusEnum::Invalid { validation_error } => {
                        // wait for the next fork choice update
                        return Err(SendNewPayloadError::InvalidPayload(validation_error));
                    }
                    PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted => {
                        debug!(target: "consensus::authority", ?recv, "Authority fork new payload returned VALID");
                        return Ok(recv);
                    }
                }
            }
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Authority new payload failed");
                return Err(SendNewPayloadError::BeaconError(err));
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
pub(crate) async fn send_fork_choice_update_payload<Engine: reth_node_api::EngineTypes>(
    new_block_hash: BlockHash,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
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
                        return Ok(());
                    }
                    ForkchoiceStatus::Syncing => {
                        trace!(target: "consensus::authority", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                        // wait for the next fork choice update
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
            Err(err) => {
                error!(target: "consensus::authority", ?err, "Authority fork choice update failed");
                return Err(SendForkChoiceUpdateError::InvalidPayload);
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
/// Error type for starting a new payload
pub(crate) enum StartNewPayloadError {
    #[error("Engine error")]
    EngineError(PayloadBuilderError),
}

/// Start a new payload job and returns the payload id if it exists.
///
/// This function creates a `BeaconEngineMessage::StartNewPayload` message and sends it to the
/// Beacon Engine. The payload id is returned if received successfully, otherwise a
/// StartNewPayloadError is returned.
///
/// # Arguments
///
/// * `to_engine` - The sender to send the message to the Beacon Engine.
/// * `payload_attributes` - The payload attributes.
/// * `parent` - The parent block hash the payload will be built on.
pub(crate) async fn start_new_payload(
    payload_builder: &PayloadBuilderHandle<EthEngineTypes>,
    payload_attributes: EthPayloadBuilderAttributes,
) -> Result<PayloadId, StartNewPayloadError> {
    let payload_id = payload_builder
        .new_payload(payload_attributes)
        .await
        .map_err(StartNewPayloadError::EngineError)?;
    Ok(payload_id)
}

#[derive(Debug, thiserror::Error)]
/// Error type for getting the best transactions from a payload
pub(crate) enum BestTransactionsError {
    #[error("Engine error")]
    EngineError(PayloadBuilderError),
    #[error("Empy payload error")]
    PayloadEmpty,
}

/// Gets the best transactions from the payload with the given id.
///
/// This function creates a `BeaconEngineMessage::BestPayload` message and sends it to the
/// Beacon Engine. The best transactions are returned if received successfully,
/// otherwise a BestTransactionsError is returned.
///
/// # Arguments
/// * `to_engine` - The sender to send the message to the Beacon Engine.
/// * `payload_id` - The payload id to get the best transactions from.
pub(crate) async fn best_transactions_from_payload(
    payload_builder: &PayloadBuilderHandle<EthEngineTypes>,
    payload_id: PayloadId,
) -> Result<EthBuiltPayload, BestTransactionsError> {
    let best_txs = payload_builder
        .best_payload(payload_id)
        .await
        .transpose()
        .map_err(BestTransactionsError::EngineError)?
        .ok_or_else(|| BestTransactionsError::PayloadEmpty)?;
    if best_txs.block().body.is_empty() {
        return Err(BestTransactionsError::PayloadEmpty);
    }
    Ok(best_txs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_payload_builder::test_utils::{spawn_test_payload_service, test_payload_service};
    use reth_primitives::{
        address, b256, bloom, bytes, revm_primitives::FixedBytes, Address, BlockBody, Header,
        SealedHeader, U256,
    };
    use reth_rpc_types::engine::PayloadAttributes;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_send_fork_choice_update_payload_valid() {
        let (tx, mut rx) = mpsc::unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();

        let header = Header {
            parent_hash: b256!("e0a94a7a3c9617401586b1a27025d2d9671332d22d540e0af72b069170380f2a"),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: address!("ba5e000000000000000000000000000000000000"),
            state_root: b256!("ec3c94b18b8a1cff7d60f8d258ec723312932928626b4c9355eb4ab3568ec7f7"),
            transactions_root: b256!("50f738580ed699f0469702c7ccc63ed2e51bc034be9479b7bff4e68dee84accf"),
            receipts_root: b256!("29b0562f7140574dd0d50dee8a271b22e1a0a7b78fca58f7c60370d8317ba2a9"),
            logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            difficulty: U256::from(0x020000),
            number: 0x01_u64,
            gas_limit: 0x016345785d8a0000_u64,
            gas_used: 0x015534_u64,
            timestamp: 0x079e,
            extra_data: bytes!("42"),
            mix_hash: b256!("0000000000000000000000000000000000000000000000000000000000000000"),
            nonce: 0,
            base_fee_per_gas: Some(0x036b_u64),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
        };
        let header = SealedHeader::new(header.clone(), header.hash_slow());
        tokio::spawn(send_fork_choice_update_payload(header.hash_slow(), tx.clone()));

        // Ensure that the engine received the message
        let msg = rx.recv().await.unwrap();
        match msg {
            BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx: _ } => {
                assert_eq!(state.head_block_hash, header.hash_slow());
                assert_eq!(state.finalized_block_hash, header.hash_slow());
                assert_eq!(state.safe_block_hash, header.hash_slow());
                assert!(payload_attrs.is_none());
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[tokio::test]
    async fn test_send_new_payload_valid() {
        let (tx, mut rx) = mpsc::unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();

        let block_body = BlockBody::default();
        let header = Header {
            parent_hash: b256!("e0a94a7a3c9617401586b1a27025d2d9671332d22d540e0af72b069170380f2a"),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: address!("ba5e000000000000000000000000000000000000"),
            state_root: b256!("ec3c94b18b8a1cff7d60f8d258ec723312932928626b4c9355eb4ab3568ec7f7"),
            transactions_root: b256!("50f738580ed699f0469702c7ccc63ed2e51bc034be9479b7bff4e68dee84accf"),
            receipts_root: b256!("29b0562f7140574dd0d50dee8a271b22e1a0a7b78fca58f7c60370d8317ba2a9"),
            logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            difficulty: U256::from(0x020000),
            number: 0x01_u64,
            gas_limit: 0x016345785d8a0000_u64,
            gas_used: 0x015534_u64,
            timestamp: 0x079e,
            extra_data: bytes!("42"),
            mix_hash: b256!("0000000000000000000000000000000000000000000000000000000000000000"),
            nonce: 0,
            base_fee_per_gas: Some(0x036b_u64),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
        };
        let header = SealedHeader::new(header.clone(), header.hash_slow());
        let sealed_block = SealedBlock::new(header, block_body);
        tokio::spawn(send_beacon_new_payload(sealed_block.clone(), tx.clone()));

        // Ensure that the engine received the message
        let msg = rx.recv().await.unwrap();
        match msg {
            BeaconEngineMessage::NewPayload { payload, cancun_fields, tx: _ } => {
                assert_eq!(payload.block_hash(), sealed_block.hash());
                assert_eq!(cancun_fields, None);
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[tokio::test]
    async fn test_start_new_payload() {
        let payload_attributes = PayloadAttributes {
            timestamp: 0,
            prev_randao: FixedBytes::default(),
            suggested_fee_recipient: Address::default(),
            withdrawals: None,
            parent_beacon_block_root: None,
        };
        let parent = FixedBytes::default();
        let payload_attr = EthPayloadBuilderAttributes::new(parent, payload_attributes);

        let payload_service_handle = spawn_test_payload_service::<EthEngineTypes>();
        let payload_id = payload_service_handle.new_payload(payload_attr).await;
        assert!(payload_id.is_ok());
    }

    #[tokio::test]
    async fn test_get_best_transactions_from_payload() {
        let payload_attributes = PayloadAttributes {
            timestamp: 0,
            prev_randao: FixedBytes::default(),
            suggested_fee_recipient: Address::default(),
            withdrawals: None,
            parent_beacon_block_root: None,
        };
        let parent = FixedBytes::default();
        let payload_attr = EthPayloadBuilderAttributes::new(parent, payload_attributes);

        let payload_service_handle = spawn_test_payload_service::<EthEngineTypes>();
        let payload_id = payload_service_handle.new_payload(payload_attr).await;
        assert!(payload_id.is_ok());
        let payload_id = payload_id.unwrap();

        let best_payload = payload_service_handle.best_payload(payload_id.clone()).await;
        assert!(best_payload.is_some());
        let best_payload = best_payload.unwrap();
        assert!(best_payload.is_ok());
        let best_payload = best_payload.unwrap();
        assert_eq!(best_payload.id(), payload_id);
    }
}
