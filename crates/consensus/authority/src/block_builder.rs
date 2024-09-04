use std::time::Duration;

use bitcoin::{
    hashes::{sha256, Hash},
    psbt::Psbt,
    Witness,
};
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_botanix_lib::mint_validation::{try_parse_burn_event, try_parse_mint_event};
use reth_btc_wallet::bitcoind::BitcoindFactory;
use reth_consensus_common::utils;

use reth_eth_wire::NewBlock;

use reth_network::frost::manager::ToFrostManager;
use reth_node_api::EngineTypes;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{header_ext::HeaderExt, Address, Block, SealedBlockWithSenders, B256};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::engine::PayloadAttributes;
use ruint::Uint;
use tracing::{error, info, trace, warn};

use crate::{
    engine_util,
    excecution_utils::authority_execution_utils::{
        build_and_execute, build_and_validate_completed_header,
    },
    frost_task::{FrostNotification, FrostNotificationMessage},
    pbft_task::{PbftFinalizationNotification, PbftNotification, PbftNotificationMessage},
    task::BlockProductionTask,
    utils::{call_notify_pegin, get_witness_data_from_psbt, is_active_sync_in_progress},
    utxo_sync::UTXOSync,
};

impl<EF, BF, DB, Engine: reth_node_api::EngineTypes, ToFrostMan>
    BlockProductionTask<EF, BF, DB, Engine, ToFrostMan>
where
    DB: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
    EF: ExecutorFactory + Clone + 'static,
    BF: BitcoindFactory + Clone + 'static,
    Engine: EngineTypes + 'static,
    ToFrostMan: ToFrostManager + Clone + 'static,
{
    pub(crate) async fn try_build_block(&mut self) {
        // ensure the node is not syncing
        if is_active_sync_in_progress(&self.network_handle) {
            warn!(target: "consensus::authority", "Node is still syncing, block builder task is awaiting fully synced status ...");
            tokio::time::sleep(Duration::from_millis(500)).await;
            return;
        }

        if let Err(utxo_sync_err) = self.utxo_sync.sync_utxo_set().await {
            error!(target: "consensus::authority", ?utxo_sync_err, "Failed to sync utxo set");
            tokio::time::sleep(Duration::from_secs(5)).await;
            return;
        };

        // Check if we are in_turn
        let is_inturn = self.epoch_manager.poll().await;

        if !is_inturn {
            trace!(target: "consensus::authority", "Not in turn, skipping");
            return;
        }
        let guard = self.storage.inner.read().await;
        let client = guard.client.clone();
        let bitcoin_network = guard.btc_network.clone();
        let chain_spec = guard.chain_spec.clone();
        drop(guard);

        let best_block = client.best_block_number().expect("best block number exists");
        let best_hash =
            client.block_hash(best_block).expect("best block hash exists").unwrap_or_else(|| {
                panic!("best block hash is valid");
            });

        let payload_attributes = PayloadAttributes {
            timestamp: utils::unix_timestamp(),
            prev_randao: B256::ZERO, // only relevant for PoS
            suggested_fee_recipient: Address::ZERO, /* fees are handled in processor.rs before
                                      * the bundle state is created */
            withdrawals: None,              // only relevant for PoS
            parent_beacon_block_root: None, // only relevant for PoS
        };

        let payload_attr = EthPayloadBuilderAttributes::new(best_hash, payload_attributes);

        // start new payload
        let payload_id = engine_util::start_new_payload(&self.payload_builder, payload_attr).await;

        if payload_id.is_err() {
            warn!(target: "consensus::authority", "Failed to start new payload");
            return;
        }

        let payload_id = payload_id.expect("payload id exists");
        let best_transactions = match tokio::time::timeout(
            Duration::from_secs(1),
            engine_util::best_transactions_from_payload(&self.payload_builder, payload_id),
        )
        .await
        {
            Ok(transactions) => {
                if transactions.is_ok() {
                    transactions
                } else {
                    // retry once since payload might not be ready yet
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let transactions = engine_util::best_transactions_from_payload(
                        &self.payload_builder,
                        payload_id,
                    )
                    .await;

                    if transactions.is_ok() {
                        transactions
                    } else {
                        warn!(target: "consensus::authority", "Retry failed to get best transactions from payload");
                        return;
                    }
                }
            }
            Err(e) => {
                warn!(target: "consensus::authority", ?e, "Timeout: Failed to get best transactions from payload");
                return;
            }
        };

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

        let storage = self.storage.inner.write().await;
        // retrieve aggregate key
        let agg_pk = match storage.aggregate_public_key {
            Some(pk) => pk,
            None => {
                warn!(target: "consensus::authority", "Failed to get aggregate public key from cache. DKG is probably not finished yet. Skipping block production");
                return;
            }
        };
        let bitcoin_checkpoint =
            recent_bitcoin_block_header.expect("valid header and height tuple");
        let authority_signers = storage.authorities.clone();

        // Build and execute current block template
        let (bundle_state, block, gas_used) = match build_and_execute(
            transactions.clone(),
            chain_spec.clone(),
            &self.sk,
            storage.evm_config,
            &client,
            &storage.bitcoind_factory,
            bitcoin_network,
            &bitcoin_checkpoint.0.block_hash(),
            &agg_pk,
        ) {
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to execute block");
                drop(storage);
                return;
            }
        };
        drop(storage);

        // Process pegins and pegouts from the [Minting] contract.
        let mut block_pegouts = Vec::new();
        for (idx, receipts) in bundle_state.receipts().iter().enumerate() {
            for receipt in receipts {
                if idx == 0 && receipt.is_none() {
                    break; // Prunning block, skip
                }
                if let Some(receipt) = receipt {
                    if !receipt.success {
                        continue;
                    }
                    for log in &receipt.logs {
                        // Mint event should have already been validated during evm execution (in
                        // processor.rs)
                        let pegin_match = try_parse_mint_event(log).expect("passed EVM check");
                        if let Some(pegin_data) = pegin_match {
                            info!(target: "consensus::authority", "Parsing and sending minting event to btc_server");
                            //TODO(stevenroose) should this happen here?
                            if let Err(e) =
                                call_notify_pegin(&mut self.btc_server, &pegin_data.meta).await
                            {
                                error!(target: "consensus::authority", ?e, "failed to notify btc_server of pegin");
                                return;
                            }
                            info!(target: "consensus::authority", "notifying btc server about pegin utxo");
                        }

                        let pegout_match =
                            try_parse_burn_event(log, bitcoin_network).expect("passed EVM check");
                        if let Some(pegout) = pegout_match {
                            block_pegouts.push(pegout);
                        }
                    }
                }
            }
        }

        // Retrieve the current UTXO commitment
        let utxo_commitment = match self.btc_server.get_utxo_merkle_root(client::Empty {}).await {
            Ok(h) => sha256::Hash::from_slice(&h.merkle_root).expect("valid utxo commitment"),
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to get utxo commitment");
                return;
            }
        };

        let storage = self.storage.inner.read().await;
        // If end of epoch, process pegouts
        let mut epoch_witness: Option<Vec<Witness>> = None;
        if block.header.is_poa_epoch() {
            // get pegouts up to best block
            let mut pegouts =
                match crate::utils::epoch_pegouts(best_block, &client, bitcoin_network).await {
                    Ok(epoch_pegouts) => epoch_pegouts,
                    Err(e) => {
                        error!(target: "consensus::authority", ?e, "Failed to fetch pegouts");
                        return;
                    }
                };
            // add current block pegouts
            pegouts.extend(block_pegouts);

            // send pegouts
            if !pegouts.is_empty() {
                info!(target: "consensus::authority", "Sending pegouts: {:?}", pegouts);

                let signing_session_id = crate::utils::generate_signing_session_id().map_err(|e| {
                    error!(target: "consensus::authority", ?e, "Failed to generate signing session id");
                    e
                }).expect("valid signing session id");

                let bitcoin_checkpoint = self
                    .bitcoin_block_header
                    .read()
                    .await
                    .expect("no bitcoin checkpoint in block creation procedure");
                match crate::utils::call_get_psbt(
                    &mut self.btc_server,
                    &pegouts,
                    &signing_session_id,
                    bitcoin_checkpoint.0.block_hash(),
                    utxo_commitment,
                )
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

                let witness_data = match tokio::time::timeout(
                    Duration::from_secs(
                        chain_spec
                            .leader_selection_window
                            .expect("to be defined for poa consensus") /
                            3,
                    ),
                    self.frost_task_rx.recv(),
                )
                .await
                {
                    Ok(Some(FrostNotificationMessage::FinalizedSignature(message))) => {
                        let psbt_result =
                            Psbt::deserialize(message.psbt.as_slice()).expect("valid psbt");
                        get_witness_data_from_psbt(psbt_result)
                    }
                    Err(e) => {
                        error!(target: "consensus::authority", "Failed to get finalized psbt from frost task, error: {:?}", e);
                        // Attempt to abort the current signing session
                        // We should panic if we cannot abort the signing session and be as loud as
                        // possible
                        self.btc_server.abort_signing(client::Empty {}).await.expect("valid abort");
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

        info!(target: "consensus::authority", "UTXO commitment: {:?}", utxo_commitment);

        let storage = self.storage.inner.write().await;
        let new_header = match build_and_validate_completed_header(
            &bundle_state,
            block,
            gas_used,
            &bitcoin_checkpoint.0.block_hash(),
            &self.sk,
            &authority_signers,
            &epoch_witness,
            utxo_commitment,
            &self.consensus,
            &client,
            &agg_pk,
            &storage.genesis_authorities,
        ) {
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to build and validate header");
                return;
            }
        };

        // Seal the block
        let mut block_to_commit = Block {
            header: new_header.clone().unseal(),
            body: transactions,
            ommers: vec![],
            withdrawals: None,
            requests: None,
        };
        // Propose block to network for commitments
        self.pbft_task_tx
            .send(PbftNotificationMessage::ProposeBlock(PbftNotification {
                block: block_to_commit.clone().seal_slow(),
            }))
            .expect("send pbft task message");
        // Wait for commitments before we can commit to this block
        info!(target: "consensus::authority", "Waiting for commitments...");

        drop(storage);
        match tokio::time::timeout(
            // Lets await another third of the block time for the PBFT commitments
            Duration::from_secs(
                chain_spec.leader_selection_window.expect("to be defined for poa consensus") / 3,
            ),
            self.pbft_task_rx.recv(),
        )
        .await
        {
            Ok(Some(PbftNotificationMessage::CommitmentsReceived(notif))) => {
                info!(target: "consensus::authority", "Commitments received");
                let PbftFinalizationNotification { block_witness } = notif;
                block_to_commit.header.add_block_witness(block_witness).unwrap();
            }
            Err(e) => {
                error!(target: "consensus::authority", "Timeout: Failed to get commitments from peer, error: {:?}", e);
                return;
            }
            msg => {
                warn!(target: "consensus::authority", "Recieved unknown message from pbft task: {:?}", msg);
                return;
            }
        };

        let sealed_block = block_to_commit.clone().seal_slow();
        let commited_header = sealed_block.header();

        // TODO(armins) assert that block hash has not changed after adding witness
        let sealed_block_with_senders =
            SealedBlockWithSenders::new(sealed_block.clone(), senders.clone())
                .expect("senders are valid");

        // update canon chain
        match client.insert_block(sealed_block_with_senders.clone(), reth_blockchain_tree_api::BlockValidationKind::Exhaustive) {
            Ok(_) => {}
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to insert block");
                return;
            }
        }
        client.set_canonical_head(sealed_block.header.clone());
        client.set_safe(sealed_block.header.clone());
        client.set_finalized(sealed_block.header.clone());

        match engine_util::send_fork_choice_update_payload(
            commited_header.hash_slow(),
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

        // Notify peers
        let new_block = NewBlock { block: block_to_commit.clone(), td: Uint::ZERO };
        let block_hash = new_block.clone().block.hash_slow();
        self.network_handle.announce_block(new_block, block_hash);
    }
}
