use crate::{engine_util::{self, BestTransactionsError}, task::BlockProductionTask};
use reth_consensus_common::utils;
use reth_eth_wire::NewBlock;
use reth_interfaces::blockchain_tree::{
    BlockValidationKind::SkipStateRootValidation, BlockchainTreeEngine,
};
use reth_node_api::{ConfigureEvmEnv, EngineTypes};
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_primitives::{public_key_to_address, Block, SealedBlockWithSenders, B256};
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
    EvmConfig: ConfigureEvmEnv + Clone + Unpin + Send + Sync + 'static,
{
    pub(crate) async fn try_build_block(&mut self) {
        // Check if we are in_turn
        let is_inturn = self.epoch_manager.poll().await;

        if !is_inturn {
            info!(target: "consensus::authority", "Not in turn, skipping");
            return;
        }

        let mut storage = self.storage.write().await;
        let (_best_block, best_hash) =
            storage.get_best_block_and_hash().expect("best block exists");

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
        let mut best_transactions: Result<EthBuiltPayload, BestTransactionsError> = Err(BestTransactionsError::PayloadEmpty);
        loop {
            // get payload by id
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
        };

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
            self.evm_config.clone(),
        ) {
            Ok(ret) => ret,
            Err(err) => {
                error!(target: "consensus::authority", ?err, "failed to execute block");
                drop(storage);
                return;
            }
        };

        // Process Botanix specific logs
        match crate::utils::process_receipts(
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
                return;
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
                return;
            }
        }
        storage.client.set_canonical_head(sealed_block.header.clone());
        storage.client.set_safe(sealed_block.header.clone());
        storage.client.set_finalized(sealed_block.header.clone());
        drop(storage);

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

        // Notify peers
        let new_block = NewBlock { block, td: Uint::ZERO };
        let block_hash = new_block.clone().block.hash_slow();
        self.network_handle.announce_block(new_block, block_hash);

        // TODO (scott) Process pegouts
        // access pegouts from cache (need to add) or if cache empty
        // bc busted when node went offline,
        // use utils method to get all pegouts from epoch
    }
}
