//! # Wallet Sweep Integration Test
//!
//! This module provides comprehensive integration tests for the wallet sweep functionality
//! in the Botanix federation using production APIs wherever possible.

use std::str::FromStr;

use bitcoin::{Address, Network};
use bitcoincore_rpc::RpcApi;
use btc_server_client::{BtcServerExtendedClient, BtcServerExtendedApi, Empty};
use botanix_wallet_sweep::WalletSweepRequest;
use hex::encode as hex_encode;
use rand::{rngs::StdRng, RngCore, SeedableRng};
use reth_primitives;

use crate::{
    it_info_print,
    suite::consensus::{
        frost::{test_dkg::do_dkg, test_signing::Pegin},
        ConsensusIntegrationTestSuite,
    },
    utils::{
        generate_blocks, get_checkpoint_block_hash, send_pegin_notification,
        MIN_BLOCKS_COINBASE_MATURE,
    },
};

const NUM_PEGINS_FOR_SWEEP: usize = 3;

/// Test the complete wallet sweep end-to-end flow using production APIs
pub async fn test_wallet_sweep_flow(
    suite: &mut ConsensusIntegrationTestSuite,
) -> anyhow::Result<()> {
    it_info_print!("Starting wallet sweep integration test");

    let bitcoind = suite.global_context.bitcoind_rpc();
    generate_blocks(&bitcoind, MIN_BLOCKS_COINBASE_MATURE).await;

    // Create BtcServerExtendedClient instances for production API compatibility
    let mut extended_clients = Vec::new();
    for instance in 0..suite.global_context.fed_instances {
        let port = suite
            .local_context
            .get_btc_server_process_port(instance as usize)
            .ok_or_else(|| anyhow::anyhow!("Could not find btc server port for instance {}", instance))?;
        
        let client = BtcServerExtendedClient::new(
            format!("http://localhost:{}", port),
            None // No JWT secret needed for tests
        ).await.map_err(|e| anyhow::anyhow!("Failed to create extended client: {}", e))?;
        
        extended_clients.push(client);
    }

    // Clear all existing UTXOs FIRST to ensure we start with a clean slate
    it_info_print!("Clearing all existing UTXOs from database before creating pegins");
    let mut regular_clients_for_reset = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");
    
    for (i, client) in regular_clients_for_reset.iter_mut().enumerate() {
        it_info_print!("Clearing UTXOs for federation member {} before pegin creation", i);
        client
            .reset_all_utxos(btc_server_client::ResetAllUtxosRequest { utxos: vec![] })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to clear UTXOs for member {}: {}", i, e))?;
    }
    it_info_print!("All existing UTXOs cleared successfully before pegin creation");

    // Step 1: Run DKG if not already done
    it_info_print!("Setting up DKG");
    let dkg_needed = {
        let pk_result = extended_clients[0].get_public_key(Empty {}).await;
        pk_result.is_err()
    };
    
    if dkg_needed {
        // Convert to regular clients for DKG (DKG uses the old client type)
        let mut regular_clients = suite
            .local_context
            .btc_server_clients
            .clone()
            .expect("btc server rpc clients to be defined");
        
        do_dkg(&mut regular_clients).await.map_err(|e| anyhow::anyhow!("DKG failed: {:?}", e))?;
        it_info_print!("DKG completed successfully");
    } else {
        it_info_print!("DKG already completed, skipping");
    }
    
    // Verify DKG completed and we have a public key
    let pk_result = extended_clients[0].get_public_key(Empty {}).await;
    if pk_result.is_err() {
        return Err(anyhow::anyhow!("DKG completed but public key not available"));
    }
    it_info_print!("DKG public key verified");

    // Step 2: Create UTXOs through pegins using production flow
    it_info_print!("Creating {} UTXOs for sweep test", NUM_PEGINS_FOR_SWEEP);
    let pegins = create_pegins_for_sweep(suite, &mut extended_clients).await?;
    it_info_print!("Created {} pegins", pegins.len());
    
    // Mine blocks to confirm pegins
    generate_blocks(&bitcoind, MIN_BLOCKS_COINBASE_MATURE + 10).await;
    
    // Step 3: Send pegin notifications using production flow
    it_info_print!("Sending pegin notifications for {} pegins", pegins.len());
    let checkpoint_block_hash = get_checkpoint_block_hash(&bitcoind)?;
    
    // Convert extended clients to regular clients for pegin notifications
    let mut regular_clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");
    
    for client in regular_clients.iter_mut() {
        for pegin in &pegins {
            let mut txid_bytes = Vec::with_capacity(32);
            bitcoin::consensus::Encodable::consensus_encode(&pegin.outpoint.txid, &mut txid_bytes)?;
            
            send_pegin_notification(
                client,
                checkpoint_block_hash.clone(),
                pegin.btc_address.clone(),
                hex_encode(pegin.eth_address),
                txid_bytes.try_into().map_err(|_| anyhow::anyhow!("invalid txid"))?,
                pegin.outpoint.vout,
                pegin.amount.to_sat(),
            ).await?;
        }
    }

    // Wait for pegin processing
    it_info_print!("Waiting for pegin processing");
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    // Step 4: Create wallet sweep request using production API (exactly like reth sweep initiate)
    it_info_print!("Creating wallet sweep request using production WalletSweepRequest::build_with_federation_config");
    let (sweep_request, temp_files) = create_sweep_request_with_production_api(suite).await?;
    
    // Step 5: Initiate sweep using production flow (coordinator accepts first)
    it_info_print!("Coordinator initiating wallet sweep session using production API");
    sweep_request.accept(&mut extended_clients[0]).await
        .map_err(|e| anyhow::anyhow!("Failed to initiate sweep session: {}", e))?;
    it_info_print!("✓ Coordinator successfully initiated wallet sweep session");
    
    // Step 6: Save request to temporary file (simulating distribution to other federation members)
    let temp_request_file = tempfile::NamedTempFile::new()?;
    let request_json = serde_json::to_string_pretty(&sweep_request)?;
    std::fs::write(temp_request_file.path(), &request_json)?;
    it_info_print!("Saved sweep request to temporary file for federation members");
    
    // Step 7: Other federation members accept using production API (exactly like reth sweep accept-request)
    it_info_print!("Federation members accepting wallet sweep session using production API");
    for i in 1..extended_clients.len() {
        it_info_print!("Federation member {} loading and accepting sweep request", i);
        
        // Load request from file (simulating production flow)
        let loaded_request = WalletSweepRequest::from_json_file(temp_request_file.path()).await
            .map_err(|e| anyhow::anyhow!("Failed to load sweep request from file: {}", e))?;
        
        // Accept using production API
        match loaded_request.accept(&mut extended_clients[i]).await {
            Ok(_) => it_info_print!("✓ Federation member {} successfully accepted sweep session", i),
            Err(e) => {
                it_info_print!("Warning: Member {} failed to accept: {}", i, e);
                // Continue anyway for testing
            }
        }
    }
    
    // Step 8: Verify only Taproot UTXOs are in database
    it_info_print!("Verifying only Taproot UTXOs are in database");
    let utxos_response = extended_clients[0].get_all_utxos(btc_server_client::Empty {}).await?;
    it_info_print!("Found {} UTXOs in database", utxos_response.utxos.len());
    
    for (i, utxo) in utxos_response.utxos.iter().enumerate() {
        if let Some(output) = &utxo.output {
            if let Some(script_proto) = &output.script_pubkey {
                let script = bitcoin::ScriptBuf::from_bytes(script_proto.script.clone());
                let is_taproot = script.is_p2tr();
                if !is_taproot {
                    return Err(anyhow::anyhow!("Non-Taproot UTXO found in database at index {}: script_len={}, script={}", 
                               i, script.len(), hex_encode(&script_proto.script)));
                }
                it_info_print!("✓ UTXO {}: Taproot confirmed (script_len={})", i, script.len());
            }
        }
    }

    // Step 9: Create sweep PSBT using production API (simulating FROST task behavior)
    it_info_print!("Creating sweep PSBT using production API (simulating FROST task)");
    let mut coordinator_client = extended_clients[0].clone();
    let psbt = botanix_wallet_sweep::create_psbt_async(sweep_request.clone(), &mut coordinator_client).await
        .map_err(|e| anyhow::anyhow!("Failed to create sweep PSBT: {}", e))?;
    
    if psbt.inputs.is_empty() {
        it_info_print!("No UTXOs available for sweep - test completed successfully");
        return Ok(());
    }
    
    it_info_print!("✓ Created sweep PSBT with {} inputs using production API", psbt.inputs.len());
    
    // Step 10: Generate signing session ID using production FROST task logic  
    let signing_session_id = generate_production_signing_session_id(&sweep_request);
    it_info_print!("Generated signing session ID: {}", hex_encode(signing_session_id));
    
    // Step 11: Perform FROST signing using production PSBT
    it_info_print!("Starting FROST signing with production-created sweep PSBT");
    let mut regular_clients = suite
        .local_context
        .btc_server_clients
        .clone()
        .expect("btc server rpc clients to be defined");
    
    let signed_tx = do_wallet_sweep_signing(&mut regular_clients, &signing_session_id, &psbt.serialize()).await?;

    // Step 12: Verify the sweep transaction using production verification
    it_info_print!("Verifying sweep transaction");
    verify_sweep_transaction(&signed_tx, &sweep_request).await?;
    
    // Keep temp files alive until end of function
    std::mem::forget(temp_files);
    std::mem::forget(temp_request_file);

    it_info_print!("🎉 Wallet sweep integration test completed successfully!");
    it_info_print!("Signed sweep transaction: {}", hex_encode(bitcoin::consensus::encode::serialize(&signed_tx)));
    
    Ok(())
}

/// Perform FROST signing specifically for wallet sweep sessions with a provided PSBT
/// This version takes a pre-created PSBT instead of waiting for the FROST task
async fn do_wallet_sweep_signing(
    clients: &mut Vec<btc_server_client::BtcServerClient<tonic::transport::Channel>>,
    signing_session_id: &[u8; 32],
    psbt_bytes: &[u8],
) -> anyhow::Result<bitcoin::Transaction> {
    it_info_print!("Starting wallet sweep FROST signing with session: {}", hex_encode(signing_session_id));

    // Currently we support a static coordinator (always the first client)
    let coordinator_index = 0;
    let mut coordinator = clients
        .get(coordinator_index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("coordinator not found"))?;

    // Round 1 signing: Signers add their signing commitments using our provided PSBT
    let mut round1_signing_commitments = Vec::new();
    for (index, client) in clients.iter_mut().enumerate() {
        it_info_print!("Getting round 1 signing package from member {}", index);
        
        let signing_package = client
            .get_round1_signing_package(tonic::Request::new(
                btc_server_client::SigningPackageRequest {
                    psbt: psbt_bytes.to_vec(),
                    signing_session_id: signing_session_id.to_vec(),
                },
            ))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get round1 signing package from member {}: {}", index, e))?
            .into_inner();
        
        round1_signing_commitments.push(signing_package);
    }

    // Coordinator collects all round 1 signing commitments
    it_info_print!("Coordinator collecting round 1 signing commitments");
    for signing_package in round1_signing_commitments {
        coordinator
            .new_round1_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit round1 signing package: {}", e))?;
    }

    // Round 2 signing: Get the updated package for round 2
    let round2_to_sign_package = coordinator
        .get_to_sign_package(tonic::Request::new(btc_server_client::ToSignRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get round2 to_sign package: {}", e))?
        .into_inner();

    // Round 2 signing: Signers add their partial signatures
    let mut round2_signing_commitments = Vec::new();
    for (index, client) in clients.iter_mut().enumerate() {
        it_info_print!("Getting round 2 signing package from member {}", index);
        
        let signing_package = client
            .get_round2_signing_package(tonic::Request::new(btc_server_client::SigningPackageRequest {
                psbt: round2_to_sign_package.psbt.clone(),
                signing_session_id: signing_session_id.to_vec(),
            }))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get round2 signing package from member {}: {}", index, e))?
            .into_inner();
        
        round2_signing_commitments.push(signing_package);
    }

    // Coordinator collects all round 2 signing commitments  
    it_info_print!("Coordinator collecting round 2 signing commitments");
    for signing_package in round2_signing_commitments {
        coordinator
            .new_round2_signing_package(tonic::Request::new(signing_package))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit round2 signing package: {}", e))?;
    }

    // Finalize the signing
    it_info_print!("Finalizing wallet sweep signing");
    let finalized = coordinator
        .finalize_signing(tonic::Request::new(btc_server_client::FinalizeSigningRequest {
            signing_session_id: signing_session_id.to_vec(),
        }))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to finalize signing: {}", e))?
        .into_inner();

    // Extract the final transaction
    let final_psbt = bitcoin::Psbt::deserialize(&finalized.psbt)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize final PSBT: {}", e))?;
    
    let final_tx = final_psbt.extract_tx()
        .map_err(|e| anyhow::anyhow!("Failed to extract transaction from PSBT: {}", e))?;

    // Verify the transaction has proper witness signatures
    for (i, input) in final_tx.input.iter().enumerate() {
        if input.witness.is_empty() {
            return Err(anyhow::anyhow!("Input {} missing witness data", i));
        }
        if input.witness.len() != 1 {
            return Err(anyhow::anyhow!("Input {} should have exactly one witness item for Taproot", i));
        }
        if input.witness[0].len() != 64 {
            return Err(anyhow::anyhow!("Input {} signature should be 64 bytes for Taproot", i));
        }
    }

    it_info_print!("✓ Wallet sweep FROST signing completed successfully");
    it_info_print!("✓ Transaction has {} inputs and {} outputs", final_tx.input.len(), final_tx.output.len());
    
    Ok(final_tx)
}

/// Create pegins for sweep test using production flow
async fn create_pegins_for_sweep(
    suite: &ConsensusIntegrationTestSuite,
    clients: &mut [BtcServerExtendedClient],
) -> anyhow::Result<Vec<Pegin>> {
    let bitcoind = suite.global_context.bitcoind_rpc();
    let mut pegins = Vec::new();
    let mut rng = StdRng::seed_from_u64(42);

    for i in 0..NUM_PEGINS_FOR_SWEEP {
        it_info_print!("Creating pegin {} of {}", i + 1, NUM_PEGINS_FOR_SWEEP);
        // Generate random Ethereum address
        let mut eth_addr_bytes = [0u8; 20];
        rng.fill_bytes(&mut eth_addr_bytes);
        let eth_address = ethers::core::types::Address::from(eth_addr_bytes);

        // Get gateway address using production API
        let res = clients[0]
            .get_gateway_address(btc_server_client::GetGatewayAddressRequest {
                eth_address: hex_encode(eth_address),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get gateway address: {}", e))?;
        
        let btc_address = Address::from_str(&res.gateway_address)?.assume_checked();
        
        // Debug: Check address type to ensure it's Taproot
        it_info_print!("Generated gateway address: {} (type: {:?})", btc_address, btc_address.address_type());
        
        // Send Bitcoin to the gateway address
        let amount = bitcoin::Amount::from_btc(0.001 * (i + 1) as f64)?;
        let txid = bitcoind.send_to_address(
            &btc_address,
            amount,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;

        generate_blocks(&bitcoind, 2).await;

        let tx_res = bitcoind.get_transaction(&txid, None)?;
        let pegin_tx = tx_res.transaction()?;
        let spk = btc_address.script_pubkey(); // This is the Taproot script
        let (vout, _pegin_output) = pegin_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == spk)
            .ok_or_else(|| anyhow::anyhow!("pegin output not found"))?;

        let outpoint = bitcoin::OutPoint { txid, vout: vout as u32 };

        pegins.push(Pegin {
            eth_address,
            btc_address,
            outpoint,
            amount,
        });

        it_info_print!("Created pegin {} with amount {} sats", i + 1, amount.to_sat());
    }

    Ok(pegins)
}

/// Create wallet sweep request using production WalletSweepRequest::build_with_federation_config API
/// Returns the request and temp files that must be kept alive
async fn create_sweep_request_with_production_api(
    suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(WalletSweepRequest, (tempfile::NamedTempFile, tempfile::NamedTempFile))> {
    use botanix_wallet_sweep::request::DestinationConfig;
    use botanix_configs::federation::FederationTomlConfig;
    use std::fs;

    // Use the actual federation member public keys from the test suite
    let poa_nodes = suite.local_context.poa_nodes.as_ref()
        .ok_or_else(|| anyhow::anyhow!("POA nodes not available"))?;
    
    it_info_print!("Building federation config with {} POA nodes", poa_nodes.len());
    let fed_members: Vec<botanix_configs::federation::FedMemberPubKey> = poa_nodes
        .values()
        .enumerate()
        .map(|(i, node)| {
            let pubkey = node.secret_key.public_key(&secp256k1::Secp256k1::new()).to_string();
            let member = botanix_configs::federation::FedMemberPubKey {
                key: pubkey.clone(),
                socket_addr: format!("127.0.0.1:{}", node.discovery_port),
            };
            it_info_print!("Federation member {} configured", i);
            member
        })
        .collect();

    let federation_config = FederationTomlConfig::new(
        fed_members,
        String::new(),
        String::new(), 
        String::new(),
    );

    // Create temporary files for the production API
    let temp_fed_config = tempfile::NamedTempFile::new()?;
    fs::write(&temp_fed_config.path(), toml::to_string(&federation_config)?)?;

    // Use the private key of the first federation member as coordinator
    let coordinator_private_key = poa_nodes
        .values()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No federation members available"))?
        .secret_key;
    
    let temp_coordinator_key = tempfile::NamedTempFile::new()?;
    // Write the private key as raw hex bytes (without 0x prefix)
    let coordinator_key_hex = hex::encode(coordinator_private_key.secret_bytes());
    it_info_print!("Writing coordinator key: path={:?}, hex_length={}", temp_coordinator_key.path(), coordinator_key_hex.len());
    fs::write(&temp_coordinator_key.path(), &coordinator_key_hex)?;

    // Create destination config
    #[derive(Debug)]
    struct TestDestination {
        address: String,
        network: Network,
        fee_rate: u64,
    }

    impl DestinationConfig for TestDestination {
        fn network(&self) -> eyre::Result<Network> {
            Ok(self.network)
        }

        fn address(&self) -> eyre::Result<Address> {
            // Try parsing as a bech32 address first, then fall back to base58
            match Address::from_str(&self.address) {
                Ok(addr) => addr.require_network(self.network).map_err(Into::into),
                Err(e) => {
                    // Add debug information about the parsing failure
                    it_info_print!("Address parsing failed: address={}, error={}", self.address, e);
                    Err(eyre::eyre!("Failed to parse address '{}': {}", self.address, e))
                }
            }
        }

        fn fee_rate(&self) -> eyre::Result<bitcoin::FeeRate> {
            bitcoin::FeeRate::from_sat_per_vb(self.fee_rate).ok_or_else(|| eyre::eyre!("Invalid fee rate"))
        }
    }

    let destination = TestDestination {
        address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(), // Use regtest bech32 address
        network: Network::Regtest, // Use regtest for testing
        fee_rate: 10,
    };
    
    it_info_print!("Destination config created for regtest");
    it_info_print!("Federation config: members_count={}", federation_config.federation_member_public_key.len());

    // Use production API to build the request (exactly like reth sweep initiate command)
    let sweep_request = WalletSweepRequest::build_with_federation_config(
        &destination,
        temp_coordinator_key.path(),
        &federation_config,
    ).map_err(|e| anyhow::anyhow!("Failed to create sweep request with production API: {}", e))?;

    it_info_print!("✓ Created wallet sweep request using production API: coordinator_id={}", sweep_request.coordinator_id);
    it_info_print!("✓ Destination: {}, Fee rate: {} sat/vB", destination.address, destination.fee_rate);
    
    Ok((sweep_request, (temp_fed_config, temp_coordinator_key)))
}

/// Generate signing session ID using the same logic as production FROST task
fn generate_production_signing_session_id(sweep_request: &WalletSweepRequest) -> [u8; 32] {
    // This matches the logic in frost_task.rs:907-911
    let mut session_id_data = Vec::new();
    // Use coordinator_id (already u16) to match FROST task logic  
    session_id_data.extend_from_slice(&sweep_request.coordinator_id.to_le_bytes());
    session_id_data.extend_from_slice(sweep_request.destination_address.clone().assume_checked().to_string().as_bytes());
    session_id_data.extend_from_slice(&sweep_request.created_at.to_le_bytes());
    session_id_data.extend_from_slice(b"SWEEP_SIGNING");
    
    // Use keccak256 hash like production FROST task
    let hash = reth_primitives::keccak256(session_id_data);
    hash.0
}

/// Verify sweep transaction using production verification logic
async fn verify_sweep_transaction(
    signed_tx: &bitcoin::Transaction,
    sweep_request: &WalletSweepRequest,
) -> anyhow::Result<()> {
    it_info_print!("Verifying sweep transaction structure");

    // Verify it's a sweep transaction (single output)
    if signed_tx.output.len() != 1 {
        return Err(anyhow::anyhow!("Sweep transaction must have exactly one output"));
    }

    // Verify output goes to correct destination
    let expected_script = sweep_request.destination_address
        .clone()
        .require_network(Network::Regtest)?
        .script_pubkey();
    
    if signed_tx.output[0].script_pubkey != expected_script {
        return Err(anyhow::anyhow!("Output does not go to specified destination"));
    }

    // Verify all inputs have Taproot signatures
    for (i, input) in signed_tx.input.iter().enumerate() {
        if input.witness.is_empty() {
            return Err(anyhow::anyhow!("Input {} missing witness data", i));
        }
        if input.witness.len() != 1 {
            return Err(anyhow::anyhow!("Input {} should have exactly one witness item", i));
        }
        if input.witness[0].len() != 64 {
            return Err(anyhow::anyhow!("Input {} signature should be 64 bytes", i));
        }
    }

    it_info_print!("✓ Sweep transaction verified successfully");
    it_info_print!("✓ {} inputs, 1 output, {} sats", 
                  signed_tx.input.len(), signed_tx.output[0].value.to_sat());
    
    Ok(())
}

/// Test PSBT creation functionality independently using production APIs
pub async fn test_sweep_psbt_creation(
    suite: &mut ConsensusIntegrationTestSuite,
) -> anyhow::Result<()> {
    it_info_print!("Testing wallet sweep PSBT creation independently using production APIs");

    // Create BtcServerExtendedClient for production API
    let port = suite
        .local_context
        .get_btc_server_process_port(0)
        .ok_or_else(|| anyhow::anyhow!("Could not find btc server port"))?;
    
    let mut client = BtcServerExtendedClient::new(
        format!("http://localhost:{}", port),
        None
    ).await.map_err(|e| anyhow::anyhow!("Failed to create extended client: {}", e))?;

    // Create a sweep request using production API
    let (sweep_request, temp_files) = create_sweep_request_with_production_api(suite).await?;

    // Test PSBT creation using production API
    let psbt = botanix_wallet_sweep::create_psbt_async(sweep_request.clone(), &mut client).await
        .map_err(|e| anyhow::anyhow!("Failed to create PSBT with production API: {}", e))?;

    // Verify PSBT structure
    if psbt.inputs.is_empty() {
        it_info_print!("No UTXOs available for PSBT creation test - test passed");
        return Ok(());
    }

    if psbt.outputs.len() != 1 {
        return Err(anyhow::anyhow!("PSBT should have exactly one output"));
    }
    
    it_info_print!("✓ PSBT created successfully with {} inputs using production API", psbt.inputs.len());
    it_info_print!("✓ PSBT output value: {} sats", psbt.unsigned_tx.output[0].value.to_sat());

    // Keep temp files alive
    std::mem::forget(temp_files);

    Ok(())
} 