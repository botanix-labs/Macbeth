use crate::config::Config;
use crate::error::{EmergencyToolError, Result};
use bitcoin::{
    psbt::Psbt, 
    Address, Amount, OutPoint, ScriptBuf, TxIn, TxOut, Txid,
};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, str::FromStr};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Utxo {
    pub txid: Txid,
    pub vout: u32,
    pub amount: Amount,
}

#[derive(Serialize, Debug)]
pub struct ExcludedUtxoReport {
    pub timestamp: String,
    pub consensus_threshold: u8,
    pub total_members: usize,
    pub required_votes: usize,
    pub excluded_utxos: Vec<ExcludedUtxoInfo>,
    pub summary: ExcludedSummary,
}

#[derive(Serialize, Debug)]
pub struct ExcludedUtxoInfo {
    pub utxo: Utxo,
    pub votes: usize,
    pub required_votes: usize,
    pub reporting_members: Vec<String>, // member indices who reported this UTXO
}

#[derive(Serialize, Debug)]
pub struct ExcludedSummary {
    pub total_excluded_utxos: usize,
    pub total_excluded_value: u64, // in satoshis
    pub exclusion_reasons: HashMap<String, usize>,
}

pub struct BtcClient {
    client: Client,
}

impl BtcClient {
    pub fn new(config: &Config) -> Result<Self> {
        // Validate config parameters
        if config.bitcoind_rpc_url.is_empty() {
            return Err(EmergencyToolError::EmptyBitcoinRpcUrl);
        }
        if config.bitcoind_rpc_user.is_empty() {
            return Err(EmergencyToolError::EmptyBitcoinRpcUser);
        }
        if config.bitcoind_rpc_pass.is_empty() {
            return Err(EmergencyToolError::EmptyBitcoinRpcPass);
        }

        let rpc = Client::new(
            &config.bitcoind_rpc_url,
            Auth::UserPass(
                config.bitcoind_rpc_user.clone(),
                config.bitcoind_rpc_pass.clone(),
            ),
        ).map_err(|_| EmergencyToolError::BitcoinRpcClientCreationFailed)?;
        
        Ok(Self { client: rpc })
    }

    /// Checks if this node is configured as the coordinator
    pub fn is_coordinator(&self, config: &Config) -> Result<bool> {
        // Validate federation config exists
        if !config.federation_config_path.exists() {
            return Err(EmergencyToolError::FederationConfigNotFound {
                path: config.federation_config_path.clone(),
            });
        }

        // Determine coordinator identifier (defaults to 0)
        let coordinator_id = config.coordinator.unwrap_or(0);
        
        let is_coord = config.identifier == coordinator_id;
        if is_coord {
            println!("  Node {} is the designated coordinator", config.identifier);
        } else {
            println!("  Node {} is NOT the coordinator (coordinator is {})", config.identifier, coordinator_id);
        }
        
        Ok(is_coord)
    }

    pub async fn get_utxos(&self) -> Result<Vec<Utxo>> {
        let utxos = self.client.list_unspent(None, None, None, None, None)
            .map_err(|_| EmergencyToolError::UtxoRetrievalFailed)?;
        
        if utxos.is_empty() {
            println!("  Warning: No UTXOs found");
        }
        
        let utxos = utxos
            .into_iter()
            .map(|u| Utxo {
                txid: u.txid,
                vout: u.vout,
                amount: u.amount,
            })
            .collect();
        Ok(utxos)
    }

    pub async fn compute_safe_subset(
        &self,
        all_member_utxos: Vec<Vec<Utxo>>,
        member_labels: Vec<String>,
        consensus_threshold: u8,
        excluded_report_path: Option<&Path>,
    ) -> Result<Vec<Utxo>> {
        // Validate inputs
        if all_member_utxos.is_empty() {
            return Err(EmergencyToolError::NoMemberUtxoSets);
        }
        
        if member_labels.len() != all_member_utxos.len() {
            return Err(EmergencyToolError::MemberLabelsMismatch {
                labels_count: member_labels.len(),
                sets_count: all_member_utxos.len(),
            });
        }
        
        if consensus_threshold == 0 || consensus_threshold > 100 {
            return Err(EmergencyToolError::InvalidConsensusThreshold { threshold: consensus_threshold });
        }

        println!("  Computing safe subset with {}% threshold", consensus_threshold);
        
        let total_members = all_member_utxos.len();
        let required_votes = ((total_members as f64 * consensus_threshold as f64) / 100.0).ceil() as usize;
        
        if required_votes > total_members {
            return Err(EmergencyToolError::RequiredVotesExceedsMembers {
                required: required_votes,
                total: total_members,
            });
        }
        
        println!("  Computing consensus from {} members, requiring {} votes ({}% threshold)", 
                total_members, required_votes, consensus_threshold);

        // Count votes for each UTXO and track which members reported each UTXO
        let mut utxo_votes: HashMap<Utxo, usize> = HashMap::new();
        let mut utxo_reporters: HashMap<Utxo, Vec<String>> = HashMap::new();
        
        let mut total_utxos_discovered = 0;
        for (i, member_utxos) in all_member_utxos.iter().enumerate() {
            if member_utxos.is_empty() {
                println!("  Warning: Member {} reported no UTXOs", member_labels[i]);
            }
            total_utxos_discovered += member_utxos.len();
            
            for utxo in member_utxos {
                // Validate UTXO data
                if utxo.amount.to_sat() == 0 {
                    println!("  Warning: UTXO {}:{} has zero value, excluding from consensus", utxo.txid, utxo.vout);
                    continue;
                }
                
                *utxo_votes.entry(utxo.clone()).or_insert(0) += 1;
                utxo_reporters.entry(utxo.clone()).or_default().push(member_labels[i].clone());
            }
        }

        if total_utxos_discovered == 0 {
            return Err(EmergencyToolError::NoValidUtxosDiscovered);
        }

        // Separate consensus and excluded UTXOs using proper vote counting
        let mut consensus_utxos = Vec::new();
        let mut excluded_utxos = Vec::new();
        let mut total_excluded_value: u64 = 0;
        let mut exclusion_reasons = HashMap::new();

        for (utxo, votes) in utxo_votes {
            if votes >= required_votes {
                println!("  UTXO {}:{} has {} votes - INCLUDED", utxo.txid, utxo.vout, votes);
                consensus_utxos.push(utxo);
            } else {
                println!("  UTXO {}:{} has {} votes - EXCLUDED (need {})", utxo.txid, utxo.vout, votes, required_votes);
                
                let reporting_members = utxo_reporters.get(&utxo).cloned().unwrap_or_default();
                excluded_utxos.push(ExcludedUtxoInfo {
                    utxo: utxo.clone(),
                    votes,
                    required_votes,
                    reporting_members,
                });
                
                total_excluded_value += utxo.amount.to_sat();
                let reason = format!("insufficient_votes_{}_of_{}", votes, required_votes);
                *exclusion_reasons.entry(reason).or_insert(0) += 1;
            }
        }

        if consensus_utxos.is_empty() {
            return Err(EmergencyToolError::NoConsensusAchieved {
                excluded_count: excluded_utxos.len(),
            });
        }

        // Create proper excluded report with vote analysis
        if let Some(path) = excluded_report_path {
            let excluded_summary = ExcludedSummary {
                total_excluded_utxos: excluded_utxos.len(),
                total_excluded_value,
                exclusion_reasons,
            };

            let excluded_report = ExcludedUtxoReport {
                timestamp: chrono::Utc::now().to_rfc3339(),
                consensus_threshold,
                total_members,
                required_votes,
                excluded_utxos,
                summary: excluded_summary,
            };
            
            let report_json = serde_json::to_string_pretty(&excluded_report)
                .map_err(|_| EmergencyToolError::ExcludedReportSerializationFailed)?;
            std::fs::write(path, report_json)
                .map_err(|_| EmergencyToolError::ExcludedReportWriteFailed { path: path.to_path_buf() })?;
            println!("  Excluded UTXO report saved to: {}", path.display());
        }
        
        println!("  Safe subset contains {} UTXOs", consensus_utxos.len());
        Ok(consensus_utxos)
    }

    pub async fn construct_sweep_psbt(
        &self, 
        utxos: Vec<Utxo>,
        destination: &str, 
        output_path: &Path,
    ) -> Result<()> {
        // Validate inputs
        if utxos.is_empty() {
            return Err(EmergencyToolError::NoUtxosProvided);
        }

        if destination.is_empty() {
            return Err(EmergencyToolError::EmptyDestination);
        }

        // Parse and validate destination address
        let dest_address = Address::from_str(destination)
            .map_err(|_| EmergencyToolError::InvalidDestinationAddress { 
                address: destination.to_string() 
            })?
            .assume_checked(); // Assume the network is correct

        // Validate output path directory exists
        if let Some(parent) = output_path.parent() {
            if !parent.exists() {
                return Err(EmergencyToolError::OutputDirectoryNotFound {
                    path: parent.to_path_buf(),
                });
            }
        }

        // Calculate total value and validate UTXOs
        let mut total_input_value = Amount::ZERO;
        for utxo in &utxos {
            if utxo.amount.to_sat() == 0 {
                return Err(EmergencyToolError::ZeroValueUtxo {
                    txid: utxo.txid.to_string(),
                    vout: utxo.vout,
                });
            }
            total_input_value += utxo.amount;
        }
        
        println!("  Found {} UTXOs with total value: {}", utxos.len(), total_input_value);

        // Estimate fee (using a conservative rate)
        let fee_rate_sat_per_vb = 10.0; // 10 sat/vB
        let estimated_size = utxos.len() * 68 + 34 + 10; // Rough estimate for P2WPKH inputs + 1 output + overhead
        let estimated_fee = Amount::from_sat((estimated_size as f64 * fee_rate_sat_per_vb) as u64);

        if total_input_value <= estimated_fee {
            let shortage = (estimated_fee - total_input_value).to_sat();
            return Err(EmergencyToolError::InsufficientFunds {
                total_input: total_input_value.to_sat(),
                estimated_fee: estimated_fee.to_sat(),
                shortage,
            });
        }

        let output_value = total_input_value - estimated_fee;
        
        // Validate output value is above dust threshold
        let dust_threshold = Amount::from_sat(546); // Standard dust threshold
        if output_value < dust_threshold {
            return Err(EmergencyToolError::OutputBelowDustThreshold {
                output_value: output_value.to_sat(),
                dust_threshold: dust_threshold.to_sat(),
            });
        }

        // Create inputs for the transaction
        let mut psbt_inputs = Vec::new();
        for utxo in &utxos {
            psbt_inputs.push(TxIn {
                previous_output: OutPoint::new(utxo.txid, utxo.vout),
                script_sig: ScriptBuf::new(),
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: bitcoin::Witness::new(),
            });
        }

        // Create the single output to destination
        let tx_out = TxOut {
            value: output_value,
            script_pubkey: dest_address.script_pubkey(),
        };

        // Create the transaction
        let tx = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
            input: psbt_inputs,
            output: vec![tx_out],
        };

        // Create PSBT
        let mut psbt = Psbt::from_unsigned_tx(tx)
            .map_err(|_| EmergencyToolError::PsbtCreationFailed)?;

        // For each input, we need to add the witness UTXO data
        // Since we're doing an emergency sweep, we'll leave the witness UTXO fields empty
        // as they will need to be filled by the actual signing process that has access 
        // to the wallet state and proper script pubkeys
        for i in 0..utxos.len() {
            // The signing process will need to populate these fields with the correct script pubkeys
            // For now, we just ensure the PSBT structure is correct
            psbt.inputs[i].witness_utxo = None;
        }

        // Save PSBT to file
        let psbt_bytes = psbt.serialize();
        std::fs::write(output_path, &psbt_bytes)
            .map_err(|_| EmergencyToolError::PsbtWriteFailed { path: output_path.to_path_buf() })?;

        println!("  Sweep PSBT created:");
        println!("    Total input value: {}", total_input_value);
        println!("    Estimated fee: {}", estimated_fee);
        println!("    Output value: {}", output_value);
        println!("    Destination: {}", destination);
        println!("    Note: Witness UTXO data must be populated during signing");

        Ok(())
    }
} 