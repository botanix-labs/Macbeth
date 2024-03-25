use std::collections::HashMap;

use bdk::{
    miniscript::psbt::Error as PsbtError,
    wallet::coin_selection::{CoinSelectionAlgorithm, Error as BdkCoinselectionError},
};

use bitcoin::{psbt::Psbt, Address, FeeRate, OutPoint, ScriptBuf, TxOut};
use frost_secp256k1_tr as frost;
use miniscript::psbt::PsbtExt;
use reth_btc_wallet::transaction::{CalculateSighashError, ETH_ADDRESS_FIELD};

use reth_btc_wallet::TAPROOT_KEYSPEND_SATISFACTION_WEIGHT;
use secp256k1::PublicKey;

use crate::{
    database::{self, Utxo},
    merkle,
    util::{
        self, retrieve_all_partial_signatures, retrieve_all_signing_commitments, OutPointExt,
        VerifyingKeyExt, VerifyingKeyExtError,
    },
    App, Error, SECP,
};

#[derive(Debug, Error)]
pub enum CoordinatorError {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("not enough signers")]
    NotEnoughSigners,
    #[error("invalid signing package: {0}")]
    InvalidSigningPackage(&'static str),
    #[error("failed to convert verifying key to secp pk")]
    FailedToConvertVerifyingKeyToSecpPk(#[from] VerifyingKeyExtError),
    #[error("Coin Selection error: {0}")]
    CoinSelection(#[from] BdkCoinselectionError),
    #[error("Failed to calculate sighash: {0}")]
    FailedToCalculateSighash(#[from] CalculateSighashError),
    #[error("Pbst error: {0}")]
    Pbst(#[from] PsbtError),
    #[error("internal FROST error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("internal DB error")]
    Db(#[from] database::Error),
    #[error("PSBT finalization failed : {0:?}")]
    PbstFinalizationFailed(Vec<PsbtError>),
    #[error("Invalid resulting transaction")]
    InvaildResultingTx,
    #[error("Failed parse out to sign package: {0}")]
    PsbtToSigningPackageConversionError(#[from] crate::util::PsbtToSigningPackageConversionError),
    #[error("Could not find psbt")]
    CouldNotFindPsbt,
    #[error("Could not find partial signatures: {0}")]
    CouldNotFindPartialSignatures(#[from] crate::util::RetrieveUnknownKeyError),
}

impl App {
    pub(crate) fn add_pegin(&self, utxo: &Utxo) -> Result<(), CoordinatorError> {
        if self.db.store_utxo(utxo)? {
            self.db.flush()?;
            debug!("Stored utxo {}", utxo.outpoint);

            // Hash the new UTXO
            let utxo_hash = merkle::hash_utxo(utxo);

            // Retrieve all UTXOs, hash them
            let utxos = self.db.get_all_utxos()?;
            let mut utxo_hashes: Vec<[u8; 32]> = utxos.iter().map(merkle::hash_utxo).collect();
            utxo_hashes.push(utxo_hash); // Include the new UTXO hash

            // Construct the Merkle tree from hashes
            let utxo_hashes_vec_u8: Vec<Vec<u8>> =
                utxo_hashes.iter().map(|hash| hash.to_vec()).collect();
            let merkle_tree = merkle::construct_merkle_tree(&utxo_hashes_vec_u8);

            // Store the new Merkle root in the database
            let merkle_root = merkle_tree.root().expect("Merkle tree should have a root");
            self.db
                .store_utxo_merkle_root(&merkle_root)
                .map_err(|e| CoordinatorError::Db(database::Error::from(e)))?;
        } else {
            warn!("Duplicate utxo {}", utxo.outpoint);
        }
        Ok(())
    }

    pub(crate) fn add_round1_signing(
        &self,
        signing_session_id: &[u8; 32],
        frost_id: frost::Identifier,
        psbt: &Psbt,
    ) -> Result<(), CoordinatorError> {
        self.db.get_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(CoordinatorError::InvalidFrostPeerId);
        }

        // TODO (armins) need to verify here that the psbt is in a valid round 1 state

        // Note: There doesn't need to be a check for a quorum of round1 signing packages
        // The more the better in the case one is unresponsive
        // the frost lib will check if we have enough when we create the signing package
        self.db.update_psbt(signing_session_id, psbt)?;
        self.db.flush()?;
        debug!("Stored round1 signing from peer: {:?}", frost_id);

        Ok(())
    }

    pub(crate) fn add_round2_signing(
        &self,
        signing_session_id: &[u8; 32],
        frost_id: frost::Identifier,
        psbt: &Psbt,
    ) -> Result<(), CoordinatorError> {
        self.db.get_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(CoordinatorError::InvalidFrostPeerId);
        }
        // TODO (armins) need to verify here that the psbt is in a valid round 2 state
        // TODO Checks if we have enough partial signatures

        self.db.update_psbt(signing_session_id, psbt)?;
        self.db.flush()?;

        Ok(())
    }

    pub(crate) fn get_gateway_address(
        &self,
        eth_tweak: &[u8; 20],
    ) -> Result<(PublicKey, PublicKey, Address), CoordinatorError> {
        // try to get pk package from db incase we already did dkg round 3
        if let Some(pk_package) = self.db.get_public_key_package()? {
            let agg_key = pk_package.verifying_key().to_secp_pk().map_err(|e| {
                CoordinatorError::FailedToConvertVerifyingKeyToSecpPk(VerifyingKeyExtError::from(e))
            })?;
            let tweaked_key = pk_package
                .verifying_key()
                .get_tweaked(Some(eth_tweak.as_slice()))
                .to_secp_pk()
                .map_err(|e| {
                    CoordinatorError::FailedToConvertVerifyingKeyToSecpPk(
                        VerifyingKeyExtError::from(e),
                    )
                })?;
            let gateway_address =
                reth_btc_wallet::address::generate_taproot_address(&tweaked_key, self.network);

            return Ok((agg_key, tweaked_key, gateway_address));
        }
        Err(CoordinatorError::MissingKeyPackage)
    }

    pub(crate) fn get_public_key(&self) -> Result<frost::VerifyingKey, CoordinatorError> {
        // try to get pk package from db incase we already did dkg round 3
        if let Some(pk_package) = self.db.get_public_key_package()? {
            return Ok(pk_package.verifying_key().to_owned());
        }

        Err(CoordinatorError::MissingKeyPackage)
    }

    pub(crate) fn make_tx(
        &self,
        outputs: Vec<TxOut>,
        fee_rate: FeeRate,
        change_script: ScriptBuf,
    ) -> Result<Psbt, CoordinatorError> {
        // We take this lock so another call doesn't do this same
        // process while we're doing it.
        let _tx_lock = self.tx_lock.lock();

        // collect all database utxos in a hashmap
        let utxos = self.db.iter_utxos().fold::<Result<_, database::Error>, _>(
            Ok(HashMap::new()),
            |mut ret, r| {
                if let Ok(ref mut map) = ret {
                    let utxo = r?;
                    map.insert(utxo.outpoint, utxo);
                }
                ret
            },
        )?;

        // Now we're going to hijack BDK coin selection real quick..
        let bdk_utxos = utxos
            .values()
            .map(|u| {
                bdk::WeightedUtxo {
                    satisfaction_weight: TAPROOT_KEYSPEND_SATISFACTION_WEIGHT.to_wu() as usize,
                    utxo: bdk::Utxo::Local(bdk::LocalOutput {
                        outpoint: u.outpoint.to_bdk(),
                        txout: bdk::bitcoin::TxOut {
                            script_pubkey: u.output.script_pubkey.to_bytes().into(),
                            value: u.output.value,
                        },
                        keychain: bdk::KeychainKind::External,
                        is_spent: false,
                        derivation_index: 0, // we're not using this
                        // Also not used
                        confirmation_time: bdk::chain::ConfirmationTime::Confirmed {
                            height: 1,
                            time: 1,
                        },
                    }),
                }
            })
            .collect::<Vec<_>>();
        let coin_select = bdk::wallet::coin_selection::BranchAndBoundCoinSelection::new(0);
        let target_amount = outputs.iter().map(|o| o.value).sum();
        let selection = coin_select
            .coin_select(
                vec![],
                bdk_utxos,
                bdk::FeeRate::from_sat_per_vb(fee_rate.to_sat_per_vb_ceil() as f32),
                target_amount,
                change_script.clone().as_script(), // drain_script
            )
            .map_err(CoordinatorError::CoinSelection)?;
        let selected = selection
            .selected
            .iter()
            .map(|u| utxos.get(&OutPoint::from_bdk(u.outpoint())))
            .filter_map(|s| if s.is_some() { s } else { None })
            .collect::<Vec<_>>();
        let change = match selection.excess {
            bdk::wallet::coin_selection::Excess::Change { amount, .. } => {
                Some(TxOut { script_pubkey: change_script.clone(), value: amount })
            }
            _ => None,
        };

        let psbt = reth_btc_wallet::transaction::create_psbt(
            selected
                .iter()
                .map(|s| reth_btc_wallet::transaction::Input {
                    outpoint: s.outpoint,
                    output: s.output.clone(),
                    eth_address: s.eth_address,
                })
                .collect(),
            outputs,
            change.clone(),
        );

        Ok(psbt)
    }

    /// If no Err is return the orignial psbt served to this function is good to go out to the
    /// signers nothing needs to be added to it as the signers all provided their signing
    /// commitments already and the coordinator just need to verify them  
    pub(crate) fn get_to_sign(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<Psbt, CoordinatorError> {
        let _pk_package = self.db.get_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;

        // Note that the tweaks and signing commitments should be explicitly verified by the signers
        // before signing Instead we can add it to the psbt as a proprietary field for each
        // input Lastly save this to sign package to the db

        if let Some(psbt) = self.db.get_psbt(&signing_session_id)? {
            let signing_commitments = retrieve_all_signing_commitments(&psbt)?;
            for sc in signing_commitments.iter() {
                if sc.len() < self.min_signers as usize {
                    return Err(CoordinatorError::NotEnoughSigners);
                }
            }

            // TODO (armins) verify that the psbt is in a valid state for end of round 1
            return Ok(psbt);
        }

        Err(CoordinatorError::CouldNotFindPsbt)
    }

    /// Retruns finalized and ready to braodcast tx
    pub(crate) async fn finalize_signing(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<Psbt, CoordinatorError> {
        // Lock here to prevent a make_tx that uses utxos that will be removed
        let _tx_lock = self.tx_lock.lock().await;
        let mut psbt =
            self.db.get_psbt(signing_session_id)?.ok_or(CoordinatorError::CouldNotFindPsbt)?;
        let partial_sigs = retrieve_all_partial_signatures(&psbt)?;

        let tx = psbt.clone().extract_tx();
        let pk_package =
            self.db.get_public_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;
        // Check that the inputs match the number of partial sigs
        if tx.input.len() != partial_sigs.len() {
            // TODO(armins) better error variant
            return Err(CoordinatorError::InvalidSigningPackage("Number of inputs does not match"));
        }
        // Get signing packages for this signing session
        let signing_packages = util::psbt_to_signing_packages(&psbt).map_err(|e| {
            CoordinatorError::PsbtToSigningPackageConversionError(
                crate::util::PsbtToSigningPackageConversionError::from(e),
            )
        })?;

        for (index, psbt_input) in psbt.inputs.iter_mut().enumerate() {
            let signing_package = signing_packages.get(index).expect("valid index").clone();
            let eth_tweak = psbt_input.unknown.get(&ETH_ADDRESS_FIELD.clone());
            let partial_sig = partial_sigs.get(index).expect("valid index");
            let agg_sig = frost::aggregate(&signing_package, &partial_sig, &pk_package)?;

            // Skipping first byte which is encoding the parity of the y cord of R
            // We only use x-only elements. So we can skip this byte. FROST library only produces
            // x-only keys / points TODO (armins) remove the unwrap here
            let secp_sig =
                bitcoin::secp256k1::schnorr::Signature::from_slice(&agg_sig.serialize()[1..])
                    .unwrap();

            // Verify signature -- redundant check finalize psbt already checks this
            if let Some(e) = eth_tweak {
                pk_package.verifying_key().verify(
                    signing_package.message(),
                    &agg_sig,
                    Some(&e.clone().as_slice()),
                )?;
            } else {
                pk_package.verifying_key().verify(signing_package.message(), &agg_sig, None)?;
            }
            // Note: we don't need to add the internal key here for a key spend path
            // as the output key is derived from the scriptpubkey
            let hash_ty = bitcoin::sighash::TapSighashType::All;
            let sighash_type = bitcoin::psbt::PsbtSighashType::from(hash_ty);
            psbt_input.sighash_type = Some(sighash_type);
            psbt_input.tap_key_sig = Some(bitcoin::taproot::Signature { sig: secp_sig, hash_ty });
        }
        if let Err(errs) = psbt.finalize_mut(&SECP) {
            error!("Had {} PSBT finalization errors:", errs.len());
            for e in &errs {
                error!("  PSBT finalization error: {}", e);
            }
            return Err(CoordinatorError::PbstFinalizationFailed(errs));
        }

        // Finally we should remove the utxos from the db and add the change one
        let secp_pk = pk_package.verifying_key().to_secp_pk()?;
        let (change_outputs, selected_inputs) = util::add_remove_utxo_from_psbt(&psbt, &secp_pk);
        self.db.add_remove_utxos(selected_inputs.into_iter(), change_outputs.into_iter())?;
        self.db.flush()?;
        Ok(psbt)
    }
}
