use crate::{engine_util, task::BlockProductionTask};
use reth_eth_wire::NewBlock;
use reth_interfaces::blockchain_tree::{
    BlockValidationKind::SkipStateRootValidation, BlockchainTreeEngine,
};
use reth_payload_builder::{PayloadBuilderAttributes, PayloadId};
use reth_primitives::{public_key_to_address, Block, SealedBlockWithSenders, B256};
use reth_provider::{BlockReaderIdExt, CanonChainTracker, Chain, StateProviderFactory};
use reth_rpc_types::engine::PayloadAttributes;
use ruint::Uint;
use std::sync::Arc;
use tracing::{error, info};

impl<Client> BlockProductionTask<Client>
where
    Client: BlockReaderIdExt
        + StateProviderFactory
        + CanonChainTracker
        + BlockchainTreeEngine
        + Clone
        + 'static,
{
    pub(crate) async fn try_build_block(&mut self) {
        // Check if we are in_turn
        let is_inturn = self.epoch_manager.poll().await;

        if !is_inturn {
            info!(target: "consensus::authority", "Not in turn, skipping");
            return
        }

        // TODO (scott) why do we do this since we just add it back at the end?
        let events = self.pipe_line_events.take();

        let mut storage = self.storage.write().await;
        let (_best_block, best_hash) =
            storage.get_best_block_and_hash().expect("best block exists");

        // use authority address as suggested fee recipient
        let authority_pub_key = secp256k1::PublicKey::from_secret_key(&self.secp, &self.sk);
        let suggested_fee_recipient = public_key_to_address(authority_pub_key);

        let attr_result = PayloadBuilderAttributes::try_new(
            best_hash,
            PayloadAttributes {
                timestamp: 0u64,
                prev_randao: B256::ZERO,
                suggested_fee_recipient,
                withdrawals: None,
                parent_beacon_block_root: None,
            },
        );
        if let Err(err) = attr_result {
            error!("Failed to create payload attributes, err: {:?}", err);
            return
        }

        let attr = attr_result.expect("valid payload attributes");
        let mut id: PayloadId = attr.id;
        let recv = self.payload_store.send_new_payload(attr);

        match recv.await {
            Ok(res) => match res {
                Ok(payload_id) => {
                    info!("Payload builder sent new payload, {:?}", payload_id);
                    id = payload_id;
                }
                Err(err) => {
                    error!("Payload builder failed to send new payload, err: {:?}", err);
                }
            },
            Err(err) => {
                error!("Payload builder failed to send new payload, err: {:?}", err);
            }
        }

        let mut best_txs = vec![];
        match self.payload_store.best_payload(id).await {
            Some(binding) => {
                let payload = binding.unwrap();
                best_txs = payload.block().clone().body;
            }
            None => {
                println!("No payload found");
                // Handle the case when no payload is found if needed
            }
        }

        let (transactions, senders): (Vec<_>, Vec<_>) = best_txs
            .into_iter()
            .map(|tx| {
                let recovered = tx.clone().try_into_ecrecovered().unwrap();
                let signer = recovered.signer();
                (tx, signer)
            })
            .unzip();

        if transactions.is_empty() {
            return
        }

        let recent_bitcoin_block_header = *self.bitcoin_block_header.read().await;
        let authority_signers = storage.authorities.clone();

        // Build and execute current block template
        let (new_header, bundle_state) = match storage.build_and_execute(
            transactions.clone(),
            self.chain_spec.clone(),
            recent_bitcoin_block_header,
            // TODO(armins) read vote in as param
            &None,
            &self.sk,
            &self.secp,
            &authority_signers,
        ) {
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to execute block");
                drop(storage);
                return
            }
        };

        // Process Botanix specific logs
        match crate::utils::process_reciepts(
            &self.bitcoin_block_source,
            &mut self.btc_server.clone(),
            &bundle_state,
            false,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                error!(target: "consensus::authority", ?e, "Failed to process botanix log");
                return
            }
        }

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
                return
            }
        }
        storage.client.set_canonical_head(sealed_block.header.clone());
        storage.client.set_safe(sealed_block.header.clone());
        storage.client.set_finalized(sealed_block.header.clone());
        drop(storage);

        match engine_util::send_fork_choice_update_payload(new_header.hash, self.to_engine.clone())
            .await
        {
            Ok(_) => {}
            Err(e) => {
                // This should fail if the insert was successful
                error!(target: "consensus::authority", ?e, "Failed to send fork choice update");
                return
            }
        }

        // Notify peers
        let new_block = NewBlock { block, td: Uint::ZERO };
        let block_hash = sealed_block.hash;
        self.network_handle.announce_block(new_block, block_hash);

        self.pipe_line_events = events;
    }
}
