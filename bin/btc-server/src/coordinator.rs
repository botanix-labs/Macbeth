use std::{collections::HashMap, time::SystemTime};

use bdk::{
    miniscript::psbt::Error as PsbtError,
    wallet::coin_selection::{CoinSelectionAlgorithm, Error as BdkCoinselectionError},
};
use bitcoin::{
    hashes::{sha256, Hash},
    psbt::{ExtractTxError, Psbt},
    secp256k1::PublicKey,
    Address, Amount, BlockHash, FeeRate, OutPoint, ScriptBuf, TxOut,
};
use bitcoincore_rpc::RpcApi;
use client::SigningStatus;
use frost_secp256k1_tr as frost;
use reth_btc_wallet::{
    psbt::{PsbtExt as BtcPsbtExt, PsbtInputExt, PsbtOutputExt},
    transaction::CalculateSighashError,
    TAPROOT_KEYSPEND_SATISFACTION_WEIGHT,
};

use crate::{
    database::{Error as DbError, Utxo},
    pegout_id::PegoutId,
    util::{
        validate_psbt, OutPointExt, ValidatePSBTError, VerifyingKeyExt, VerifyingKeyExtError,
        NO_FLAGS, ROUND1, ROUND1_TRANSITION, ROUND2,
    },
    App, Error,
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
    Db(#[from] DbError),
    #[error("PSBT finalization failed : {0:?}")]
    PbstFinalizationFailed(Vec<PsbtError>),
    #[error("Invalid resulting transaction")]
    InvaildResultingTx,
    #[error("Failed parse out to sign package: {0}")]
    PsbtToSigningPackageConversionError(
        #[from] reth_btc_wallet::psbt::PsbtToSigningPackageConversionError,
    ),
    #[error("Could not find psbt")]
    CouldNotFindPsbt,
    #[error("Failed to broadcast tx: {0}")]
    FailedToBroadcastTx(bitcoincore_rpc::Error),
    #[error("Could not find participant information")]
    CouldNotFindParticipantInformation(),
    #[error("Failed to validate psbt: {0}")]
    FailedToValidatePsbt(#[from] ValidatePSBTError),
    #[error("extract tx error: {0}")]
    ExtractTxError(#[from] ExtractTxError),
    #[error("pegout mgr sync: {0}")]
    PegoutMgrSync(#[from] crate::pegout_scheduler::SyncError),
    #[error("utxo merkle root mismatch: expected {expected}, actual {actual:?}")]
    UtxoMerkleRootMismatch { expected: sha256::Hash, actual: sha256::Hash },
}

impl<BitcoindClient> App<BitcoindClient>
where
    BitcoindClient: RpcApi + Send + Sync + 'static,
{
    pub(crate) fn add_pegins(&self, utxos: &[&Utxo]) -> Result<(), CoordinatorError> {
        self.db.store_utxos(utxos)?;
        self.db.update_utxo_merkle_root()?;
        self.db.flush()?;
        Ok(())
    }

    pub(crate) fn add_round1_signing(
        &self,
        signing_session_id: &[u8; 32],
        frost_id: frost::Identifier,
        psbt: &Psbt,
    ) -> Result<(), CoordinatorError> {
        self.db.get_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;
        validate_psbt(psbt, ROUND1, self.min_signers, &self.db)?;

        info!("psbt() = {}", psbt);

        for input in &psbt.inputs {
            let sc = input.signing_commitments(frost_id);
            info!("sc.keys() = {:?}", sc);
            info!("frost id: {:?}", frost_id);

            if sc.is_none() {
                return Err(CoordinatorError::CouldNotFindParticipantInformation());
            }
        }

        // TODO Need to check this psbt affect the other inputs and outputs
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
        // validate PSBT
        validate_psbt(psbt, ROUND2, self.min_signers, &self.db)?;

        self.db.update_psbt(signing_session_id, psbt)?;
        self.db.flush()?;
        debug!("Stored round2 signing from peer: {:?}", frost_id);

        Ok(())
    }

    pub(crate) fn get_gateway_address(
        &self,
        eth_tweak: &[u8; 20],
    ) -> Result<(PublicKey, PublicKey, Address), CoordinatorError> {
        // try to get pk package from db in case we already did dkg round 3
        if let Some(pk_package) = self.db.get_public_key_package()? {
            let agg_key = pk_package
                .verifying_key()
                .to_secp_pk()
                .map_err(CoordinatorError::FailedToConvertVerifyingKeyToSecpPk)?;
            let tweaked_key = pk_package
                .verifying_key()
                .get_tweaked(Some(eth_tweak.as_slice()))
                .to_secp_pk()
                .map_err(CoordinatorError::FailedToConvertVerifyingKeyToSecpPk)?;
            let gateway_address =
                reth_btc_wallet::address::generate_taproot_address(&tweaked_key, self.btc_network);

            return Ok((agg_key, tweaked_key, gateway_address));
        }
        Err(CoordinatorError::MissingKeyPackage)
    }

    pub(crate) fn get_public_key(&self) -> Result<frost::VerifyingKey, CoordinatorError> {
        // try to get pk package from db in case we already did dkg round 3
        if let Some(pk_package) = self.db.get_public_key_package()? {
            return Ok(pk_package.verifying_key().to_owned());
        }

        Err(CoordinatorError::MissingKeyPackage)
    }

    pub(crate) async fn make_tx(
        &self,
        outputs: Vec<(TxOut, Option<PegoutId>)>,
        fee_rate: FeeRate,
        change_script: ScriptBuf,
        checkpoint_block: BlockHash,
        utxo_merkle_root: sha256::Hash,
    ) -> Result<Psbt, CoordinatorError> {
        // We take this lock so another call doesn't do this same
        // process while we're doing it.
        let _tx_lock = self.tx_lock.lock();

        // Sync the pegout scheduler and check we have the same UTXO view.
        self.sync_pegout_scheduler(checkpoint_block).await?;
        let our_utxo_merkle = self.db.get_utxo_merkle_root()?.unwrap_or(sha256::Hash::all_zeros());
        if utxo_merkle_root != our_utxo_merkle {
            return Err(CoordinatorError::UtxoMerkleRootMismatch {
                expected: utxo_merkle_root,
                actual: our_utxo_merkle,
            });
        }

        // collect all database utxos in a hashmap
        let utxos: HashMap<OutPoint, Utxo> =
            self.db.iter_utxos().try_fold(HashMap::new(), |mut map, r| {
                let utxo = r?; // Directly propagate the error with `?`
                map.insert(utxo.outpoint, utxo);
                Ok::<HashMap<bitcoin::OutPoint, Utxo>, DbError>(map)
            })?;
        // Filter the ones that are still pending and conflict with pending txs.
        let tracked_inputs = self.pegout_scheduler.lock().await.tracked_inputs();
        let available_utxos = utxos
            .into_iter()
            .filter(|(p, _u)| !tracked_inputs.contains(p))
            .collect::<HashMap<_, _>>();

        let to_bdk = |u: &Utxo| {
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
        };

        // Now we're going to hijack BDK coin selection real quick..
        let bdk_utxos = available_utxos.values().map(to_bdk).collect::<Vec<_>>();
        let coin_select = bdk::wallet::coin_selection::BranchAndBoundCoinSelection::new(0);
        let target_amount = outputs.iter().map(|o| o.0.value).sum::<Amount>();

        // Try once with finalized, then add pending and try again.
        let selection = coin_select
            .coin_select(
                vec![],
                bdk_utxos,
                fee_rate,
                target_amount.to_sat(),
                &change_script, // drain_script
            )
            .map_err(CoordinatorError::CoinSelection)?;

        let selected = selection
            .selected
            .iter()
            .map(|s| available_utxos.get(&OutPoint::from_bdk(s.outpoint())))
            .filter_map(|s| if s.is_some() { s } else { None })
            .collect::<Vec<_>>();
        let change = match selection.excess {
            bdk::wallet::coin_selection::Excess::Change { amount, .. } => Some(TxOut {
                script_pubkey: change_script.clone(),
                value: Amount::from_sat(amount),
            }),
            bdk::wallet::coin_selection::Excess::NoChange { .. } => None,
        };

        let pegout_ids = outputs
            .into_iter()
            .map(|(txout, pegout_id)| {
                if let Some(pegout_id) = pegout_id {
                    (txout, Some(pegout_id.as_bytes()))
                } else {
                    (txout, None)
                }
            })
            .collect::<Vec<_>>();

        let psbt = reth_btc_wallet::transaction::create_psbt(
            selected
                .iter()
                .map(|s| reth_btc_wallet::transaction::Input {
                    outpoint: s.outpoint,
                    output: s.output.clone(),
                    eth_address: s.eth_address,
                })
                .collect(),
            pegout_ids,
            change,
        );

        // Sanity check that we created a valid PSBT
        // This should not fail
        validate_psbt(&psbt, NO_FLAGS, self.min_signers, &self.db)?;

        Ok(psbt)
    }

    /// If no Err is return the original psbt served to this function is good to go out to the
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

        if let Some(psbt) = self.db.get_psbt(signing_session_id)? {
            for input in &psbt.inputs {
                let sc = input.all_signing_commitments();
                info!("sc.len() = {}", sc.len());
                if sc.len() < self.min_signers as usize {
                    return Err(CoordinatorError::NotEnoughSigners);
                }
            }

            // TODO (armins) verify that the psbt is in a valid state for end of round 1
            validate_psbt(&psbt, ROUND1_TRANSITION, self.min_signers, &self.db)?;
            return Ok(psbt);
        }

        Err(CoordinatorError::CouldNotFindPsbt)
    }

    /// Returns finalized and ready to broadcast tx
    pub(crate) async fn finalize_signing(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<Psbt, CoordinatorError> {
        // Lock here to prevent a make_tx that uses utxos that will be removed
        let _tx_lock = self.tx_lock.lock().await;
        let mut psbt =
            self.db.get_psbt(signing_session_id)?.ok_or(CoordinatorError::CouldNotFindPsbt)?;

        let pk_package =
            self.db.get_public_key_package()?.ok_or(CoordinatorError::MissingKeyPackage)?;
        // Get signing packages for this signing session
        let signing_packages = psbt
            .signing_packages()
            .map_err(CoordinatorError::PsbtToSigningPackageConversionError)?;

        for (index, psbt_input) in psbt.inputs.iter_mut().enumerate() {
            let signing_package = signing_packages.get(index).expect("valid index").clone();
            let partial_sig = psbt_input.all_partial_signatures();
            let agg_sig = frost::aggregate(&signing_package, &partial_sig, &pk_package)?;

            // Skipping first byte which is encoding the parity of the y cord of R
            // We only use x-only elements. So we can skip this byte. FROST library only produces
            // x-only keys / points TODO (armins) remove the unwrap here
            let secp_sig =
                bitcoin::secp256k1::schnorr::Signature::from_slice(&agg_sig.serialize()[1..])
                    .unwrap();

            // Verify signature -- redundant check finalize psbt already checks this
            if let Some(e) = psbt_input.eth_address() {
                pk_package.verifying_key().verify(
                    signing_package.message(),
                    &agg_sig,
                    Some(e.clone().as_slice()),
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
        if let Err(errs) =
            miniscript::psbt::PsbtExt::finalize_mut(&mut psbt, bitcoin::secp256k1::SECP256K1)
        {
            error!("Had {} PSBT finalization errors:", errs.len());
            for e in &errs {
                error!("PSBT finalization error: {}", e);
            }
            return Err(CoordinatorError::PbstFinalizationFailed(errs));
        }

        // Finally we should remove the utxos from the db and add the change one
        let tx = match miniscript::psbt::PsbtExt::extract(&psbt, bitcoin::secp256k1::SECP256K1) {
            Ok(tx) => tx,
            Err(e) => return Err(CoordinatorError::PbstFinalizationFailed(vec![e])),
        };

        let secp_pk = pk_package.verifying_key().to_secp_pk()?;
        let change_script =
            reth_btc_wallet::address::generate_taproot_change_scriptpubkey(&secp_pk);

        let pegout_ids = psbt
            .outputs
            .iter()
            .map(|o| o.pegout_id())
            .filter(|o| o.is_some())
            .map(|o| PegoutId::from_bytes(&o.unwrap()).expect("valid pegout id"))
            .collect::<Vec<_>>();
        let tx_timestamp = SystemTime::now(); // We're signing it for the first time now.
        let pegout_reqs = {
            let mut pegout_reqs = Vec::new();
            for pegout_id in pegout_ids.iter() {
                pegout_reqs.push(
                    self.db
                        .get_pending_pegout(pegout_id)?
                        .ok_or(CoordinatorError::CouldNotFindPsbt)?,
                );
            }
            pegout_reqs
        };
        // TODO adding tracked and remove pending should be a atomic
        // After signing these pegouts are no longer pending, instead they are tracked in the case
        // of a reorg or mempool drop
        self.add_tracked_tx(tx, &pegout_reqs, tx_timestamp).await?;
        self.db.remove_pending_pegout(&pegout_ids)?;
        self.db.flush()?;

        // Lets broadcast the tx
        let tx_id = match self.bitcoind_client.send_raw_transaction(&psbt.clone().extract_tx()?) {
            Ok(tx_id) => Ok(Some(tx_id)),
            Err(err) => {
                let err_msg = err.to_string();
                if err_msg.contains("already in chain") {
                    Ok(None)
                } else {
                    error!("Failed to broadcast tx: {}", err);
                    Err(CoordinatorError::FailedToBroadcastTx(err))
                }
            }
        }?;

        if let Some(tx_id) = tx_id {
            info!("Broadcasted tx: {:?}", tx_id);
        } else {
            info!("Transaction already broadcasted and in pool");
        }

        Ok(psbt)
    }

    /// Returns signing status
    pub(crate) async fn get_signing_status(
        &self,
        signing_session_id: &[u8; 32],
    ) -> Result<SigningStatus, CoordinatorError> {
        self.db
            .get_signing_status(signing_session_id)
            .map_err(|_| CoordinatorError::CouldNotFindPsbt)
    }

    /// Returns signing status
    pub(crate) async fn get_session_ids(
        &self,
        max_requested_results: u32,
    ) -> Result<Vec<[u8; 32]>, CoordinatorError> {
        let signing_sessions = self.db.get_session_ids(max_requested_results)?;
        Ok(signing_sessions)
    }
}
