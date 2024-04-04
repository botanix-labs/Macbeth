use std::time::Duration;

use crate::{
    engine_util::{self, BestTransactionsError},
    frost_task::{FrostNotification, FrostNotificationMessage},
    task::BlockProductionTask,
    utils::{get_witness_data_from_psbt, is_testnet},
};

use bitcoin::{psbt::Psbt, Witness};
use reth_consensus_common::utils;
use reth_eth_wire::NewBlock;
use reth_interfaces::blockchain_tree::{
    BlockValidationKind::SkipStateRootValidation, BlockchainTreeEngine,
};
use reth_node_api::{ConfigureEvmEnv, EngineTypes};
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_primitives::{
    botanix::BotanixConsensusPackage, public_key_to_address, Block, SealedBlockWithSenders, B256,
};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::engine::PayloadAttributes;
use ruint::Uint;
use tracing::{error, info, warn};

impl<Client, EvmConfig, Engine: reth_node_api::EngineTypes>
    BlockProductionTask<Client, EvmConfig, Engine>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    Engine: EngineTypes + 'static,
    EvmConfig:
        ConfigureEvmEnv + Clone + Unpin + Send + Sync + 'static + reth_node_api::ConfigureEvm,
{
    pub(crate) async fn try_build_block(&mut self) {
        // Check if we are in_turn
        let is_inturn = self.epoch_manager.poll().await;

        if !is_inturn {
            info!(target: "consensus::authority", "Not in turn, skipping");
            return;
        }

        let storage = self.storage.write().await;
        let (best_block, best_hash) = storage.get_best_block_and_hash().expect("best block exists");
        drop(storage);

        // use authority address as suggested fee recipient
        let authority_pub_key = secp256k1::PublicKey::from_secret_key(&self.secp, &self.sk);
        let suggested_fee_recipient = public_key_to_address(authority_pub_key);

        let payload_attributes = PayloadAttributes {
            timestamp: utils::unix_timestamp(),
            prev_randao: B256::ZERO, // only relevant for PoS
            suggested_fee_recipient,
            withdrawals: None,              // only relevant for PoS
            parent_beacon_block_root: None, // only relevant for PoS
        };

        let payload_attr = EthPayloadBuilderAttributes::new(best_hash, payload_attributes);

        // start new payload
        let payload_id =
            engine_util::start_new_payload::<EthEngineTypes>(&self.payload_builder, payload_attr)
                .await;

        if payload_id.is_err() {
            warn!(target: "consensus::authority", "Failed to start new payload");
            return;
        }

        let payload_id = payload_id.expect("payload id exists");

        // retry if best_transactions is empty bc it could be a race condition
        let mut retries = 0;
        let mut delay = tokio::time::Duration::from_secs(1);
        let max_retries = 5;
        let mut best_transactions: Result<EthBuiltPayload, BestTransactionsError> =
            Err(BestTransactionsError::PayloadEmpty);
        loop {
            // get payload by id
            // TODO replate this with async/await
            let transactions = engine_util::best_transactions_from_payload::<EthEngineTypes>(
                &self.payload_builder,
                payload_id,
            )
            .await;

            if transactions.is_ok() {
                best_transactions = transactions;
                break;
            }

            retries += 1;
            if retries >= max_retries {
                break;
            }

            // Exponential backoff
            delay *= 2;
            tokio::time::sleep(delay).await;
        }

        if best_transactions.is_err() {
            warn!(target: "consensus::authority", "Failed to get best transactions from payload");
            return;
        }

        let (transactions, senders): (Vec<_>, Vec<_>) = best_transactions
            .expect("best transactions exists")
            .block()
            .body
            .clone()
            .into_iter()
            .map(|tx| {
                let recovered = tx.clone().try_into_ecrecovered().expect("valid tx");
                let signer = recovered.signer();
                (tx, signer)
            })
            .unzip();

        let recent_bitcoin_block_header = *self.bitcoin_block_header.read().await;
        info!("recent_bitcoin_block_header: {:?}", recent_bitcoin_block_header);

        if recent_bitcoin_block_header.is_none() {
            warn!(target: "consensus::authority", "Failed to get recent bitcoin block header, async bitcoin worker is probably down");
            return;
        }

        let mut storage = self.storage.write().await;
        // retrieve aggregate key
        let secp_pk = match storage.aggregate_public_key {
            Some(pk) => pk,
            None => {
                warn!(target: "consensus::authority", "Failed to get aggregate public key from cache. DKG is probably not finished yet. Skipping block production");
                return;
            }
        };
        let botanix_consensus_pkg = BotanixConsensusPackage {
            recent_header: recent_bitcoin_block_header.expect("valid header and height tuple"),
            aggregate_public_key: secp_pk,
            btc_network: self.btc_network,
        };
        let authority_signers = storage.authorities.clone();

        // Build and execute current block template
        let (bundle_state, block, gas_used) = match storage.build_and_execute(
            transactions.clone(),
            self.chain_spec.clone(),
            Some(botanix_consensus_pkg.clone()),
            // TODO(armins) read vote in as param
            &None,
            &self.sk,
            &self.secp,
            self.evm_config.clone(),
        ) {
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to execute block");
                drop(storage);
                return;
            }
        };
        drop(storage);

        // Process Botanix specific logs and get current block pegouts
        let is_testnet = is_testnet(self.chain_spec.chain().id());
        let current_block_pegouts = match crate::utils::process_receipts(
            &mut self.btc_server.clone(),
            &bundle_state,
            botanix_consensus_pkg.recent_header.1,
            is_testnet,
            self.btc_network,
        )
        .await
        {
            Ok(pegouts) => pegouts,
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                return;
            }
        };

        let storage = self.storage.read().await;
        // If end of epoch, process pegouts
        let mut epoch_witness: Option<Vec<Witness>> = None;
        if block.header.is_poa_epoch() {
            // get pegouts up to best block
            let mut pegouts =
                match crate::utils::epoch_pegouts(best_block, &storage.client, self.btc_network)
                    .await
                {
                    Ok(epoch_pegouts) => epoch_pegouts,
                    Err(e) => {
                        error!(target: "consensus::authority", ?e, "Failed to fetch pegouts");
                        return;
                    }
                };
            // add current block pegouts
            pegouts.extend(current_block_pegouts);

            // send pegouts
            if !pegouts.is_empty() {
                info!(target: "consensus::authority", "Sending pegouts: {:?}", pegouts);

                let signing_session_id = crate::utils::generate_signing_session_id(&best_hash.0, &storage.authority).map_err(|e| {
                    error!(target: "consensus::authority", ?e, "Failed to generate signing session id");
                    return;
                }).expect("valid signing session id");

                match crate::utils::get_psbt(&mut self.btc_server, &pegouts, &signing_session_id)
                    .await
                {
                    Ok(psbt_payload) => self
                        .frost_task_tx
                        .send(FrostNotificationMessage::InitiateSigning(FrostNotification {
                            signing_session_id: psbt_payload.signing_session_id,
                            psbt: psbt_payload.psbt,
                        }))
                        .expect("send frost task message"),

                    Err(e) => {
                        error!(target: "consensus::authority", ?e, "Failed to get psbt");
                        return;
                    }
                }

                // wait until the psbt is finalized
                let witness_data = match tokio::time::timeout(
                    Duration::from_secs(60),
                    self.frost_task_rx.recv(),
                )
                .await
                {
                    Ok(Some(FrostNotificationMessage::FinalizedSignature(message))) => {
                        let psbt_result =
                            Psbt::deserialize(message.psbt.as_slice()).expect("valid psbt");
                        let witness_data = get_witness_data_from_psbt(psbt_result);
                        witness_data
                    }
                    Err(e) => {
                        error!(target: "consensus::authority", "Failed to get finalized psbt from frost task, error: {:?}", e);
                        return;
                    }
                    _ => {
                        warn!(target: "consensus::authority", "Recieved unknown message from frost task");
                        return;
                    }
                };
                epoch_witness = Some(witness_data);
                // TODO(scott): check psbt matches expected session id
            }
        }
        drop(storage);

        let mut storage = self.storage.write().await;
        let new_header = match storage.build_and_validate_header(
            &bundle_state,
            block,
            gas_used,
            Some(botanix_consensus_pkg),
            // TODO(armins) read vote in as param
            &None,
            &self.sk,
            &self.secp,
            &authority_signers,
            &epoch_witness,
        ) {
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to build and validate header");
                return;
            }
        };

        // Seal the block
        let block = Block {
            header: new_header.clone().unseal(),
            body: transactions,
            ommers: vec![],
            withdrawals: None,
        };
        let sealed_block = block.clone().seal_slow();
        let sealed_block_with_senders =
            SealedBlockWithSenders::new(sealed_block.clone(), senders.clone())
                .expect("senders are valid");

        // update canon chain for rpc
        match storage
            .client
            .insert_block(sealed_block_with_senders.clone(), SkipStateRootValidation)
        {
            Ok(_) => {}
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to insert block");
                return;
            }
        }
        storage.client.set_canonical_head(sealed_block.header.clone());
        storage.client.set_safe(sealed_block.header.clone());
        storage.client.set_finalized(sealed_block.header.clone());

        match engine_util::send_fork_choice_update_payload(
            new_header.hash(),
            self.to_engine.clone(),
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                // This should fail if the insert was successful
                error!(target: "consensus::authority", ?e, "Failed to send fork choice update");
                return;
            }
        }
        drop(storage);

        // Notify peers
        let new_block = NewBlock { block, td: Uint::ZERO };
        let block_hash = new_block.clone().block.hash_slow();
        self.network_handle.announce_block(new_block, block_hash);
    }
}
