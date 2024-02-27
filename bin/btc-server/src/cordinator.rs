use std::collections::BTreeMap;
use std::{collections::HashMap, str::FromStr};

use crate::{database, util::OutPointExt, App, Error};

use bdk::wallet::coin_selection::CoinSelectionAlgorithm;

use bitcoin::Transaction;
use bitcoin::{hashes::Hash, psbt::Psbt, FeeRate, OutPoint, ScriptBuf, TxOut};
use frost_secp256k1_tr as frost;
use miniscript::psbt::PsbtExt;
use reth_btc_wallet::transaction::ETH_ADDRESS_FIELD;
use reth_btc_wallet::TAPROOT_KEYSPEND_SATISFACTION_WEIGHT;

impl App {
    pub(crate) fn add_round1_signing(
        &self,
        signing_session_id: &[u8; 32],
        frost_id: frost::Identifier,
        signing_commitments: Vec<frost::round1::SigningCommitments>,
    ) -> Result<(), Error> {
        self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(Error::InvalidFrostPeerId);
        }

        // Note: There doesn't need to be a check for a quorum of round1 signing packages
        // The more the better in the case one is unresponsive
        // the frost lib will check if we have enough when we create the signing package
        if self
            .db
            .add_round1_signing(&signing_session_id, frost_id, signing_commitments.clone())
            .map_err(Error::Db)?
        {
            self.db.flush().map_err(Error::Db)?;
            debug!("Stored round1 signing from peer: {:?}", frost_id);
        } else {
            warn!("Duplicate round1 signing from peer: {:?}", frost_id);
        }

        Ok(())
    }

    pub(crate) fn add_round2_signing(
        &self,
        signing_session_id: &[u8; 32],
        frost_id: frost::Identifier,
        partial_sigs: Vec<frost::round2::SignatureShare>,
    ) -> Result<(), Error> {
        self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(Error::InvalidFrostPeerId);
        }

        // Checks if we have enough partial signatures
        let _existing_sigs =
            self.db.get_round2_signing_packages(signing_session_id).map_err(Error::Db)?;

        if self
            .db
            .add_round2_signing(signing_session_id, &frost_id, &partial_sigs)
            .map_err(Error::Db)?
        {
            self.db.flush().map_err(Error::Db)?;
            debug!("Stored round2 signing from peer: {:?}", frost_id);
        } else {
            warn!("Duplicate round2 signing from peer: {:?}", frost_id);
        }

        Ok(())
    }

    pub(crate) fn get_public_key(
        &self,
        eth_tweak: &[u8; 20],
    ) -> Result<frost::VerifyingKey, Error> {
        // try to get pk package from db incase we already did dkg round 3
        if let Some(pk_package) = self.db.get_public_key_package()? {
            return Ok(pk_package
                .verifying_key()
                .to_owned()
                .get_tweaked(Some(eth_tweak.as_slice())));
        }

        Err(Error::MissingKeyPackage)
    }

    pub(crate) fn make_tx(
        &self,
        outputs: Vec<TxOut>,
        fee_rate: FeeRate,
        change_script: ScriptBuf,
    ) -> Result<Psbt, Error> {
        // We take this lock so another call doesn't do this same
        // process while we're doing it.
        let _tx_lock = self.tx_lock.lock();

        // collect all database utxos in a hashmap
        let utxos = self
            .db
            .iter_utxos()
            .fold::<Result<_, database::Error>, _>(Ok(HashMap::new()), |mut ret, r| {
                if let Ok(ref mut map) = ret {
                    let utxo = r?;
                    map.insert(utxo.outpoint, utxo);
                }
                ret
            })
            .map_err(Error::Db)?;

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
            .map_err(Error::CoinSelection)?;
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

        // Signing
        // TODO(armins) Replace this once we have frost signing working
        // if let Err(err) =
        //     reth_btc_wallet::transaction::sign_psbt(&SECP, &self.key.secret_key(), &mut psbt)
        // {
        //     error!("Failed to sign psbt {:?}", err);
        //     return Err(Error::FailedToSignPbst)
        // }

        // try finalize tx
        // if let Err(errs) = psbt.finalize_mut(&SECP) {
        //     error!("Had {} PSBT finalization errors:", errs.len());
        //     for e in &errs {
        //         error!("  PSBT finalization error: {}", e);
        //     }
        //     return Err(Error::PbstFinalizationFailed(errs))
        // }
        // could do this once we are confident our code works and we don't
        // want to do the effort of tx verification
        // let tx = psbt.clone().extract_tx();
        // let tx = psbt.extract(&SECP).map_err(|_| Error::InvaildResultingTx)?;

        // then we should remove the utxos from the db and add the change one
        // let txid = tx.txid();
        // TODO (armins) when should this be done?
        // After a batched pegout tx is confirmed?
        // self.db
        //     .add_remove_utxos(
        //         selected.iter().map(|u| u.outpoint),
        //         change
        //             .map(|utxo| Utxo {
        //                 outpoint: OutPoint::new(txid, 1),
        //                 output: utxo,
        //                 eth_address: None,
        //             })
        //             .iter(),
        //     )
        //     .map_err(Error::Db)?;
    }

    pub(crate) fn get_to_sign(
        &self,
        outputs: Vec<TxOut>,
        fee_rate: FeeRate,
        signing_session_id: &[u8; 32],
    ) -> Result<(Vec<frost::SigningPackage>, Psbt), Error> {
        let pk_package = self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        let signing_commitments = self.db.get_round1_signing_packages(&signing_session_id)?;
        if signing_commitments.len() < self.min_signers as usize {
            return Err(Error::NotEnoughSigners);
        }

        let pk = hex::encode(pk_package.verifying_key().serialize());
        let secp_pk = bitcoin::secp256k1::PublicKey::from_str(pk.as_str()).expect("pk");
        let change_script = reth_btc_wallet::address::generate_taproot_scriptpubkey(&secp_pk);
        let psbt = self.make_tx(outputs, fee_rate, change_script)?;

        // signers need to sign for each input individually
        let mut signing_packages = Vec::new();
        for (index, _input) in psbt.inputs.iter().enumerate() {
            let sighash =
                reth_btc_wallet::transaction::calculate_sighash(&psbt, index).map_err(|e| {
                    error!("Failed to calculate sighash: {}", e);
                    Error::FailedToCalculateSighash
                })?;
            // Get the signing commitments for just this input
            let mut sc = BTreeMap::new();
            for (frost_id, signing_commitment) in signing_commitments.iter() {
                sc.insert(
                    *frost_id,
                    signing_commitment.get(index).ok_or(Error::NotEnoughSigners)?.clone(),
                );
            }
            let signing_package =
                frost::SigningPackage::new(sc, sighash.to_raw_hash().to_byte_array().as_slice());
            // Note that the tweaks should be explicitly verified by the signers before signing
            // Instead we can add it to the psbt as a proprietary field for each input
            // Lastly save this to sign package to the db
            signing_packages.push(signing_package);
        }

        self.db.add_signing_package(signing_session_id, signing_packages.clone())?;
        self.db.flush()?;
        Ok((signing_packages, psbt))
    }

    /// Retruns finalized and ready to braodcast tx
    pub(crate) fn finalize_signing(
        &self,
        secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All>,
        psbt: &mut Psbt,
        signing_session_id: &[u8; 32],
    ) -> Result<Transaction, Error> {
        let tx = psbt.clone().extract_tx();
        let pk_package = self.db.get_public_key_package()?.ok_or(Error::MissingKeyPackage)?;
        let partial_sigs = self.db.get_round2_signing_packages(signing_session_id)?;
        // Check that the inputs match the number of partial sigs
        if tx.input.len() != partial_sigs.len() {
            // TODO(armins) better error variant
            return Err(Error::InvalidSigningPackage("Number of inputs does not match"));
        }
        // Get signing packages for this signing session
        let signing_packages = self.db.get_signing_package(signing_session_id)?;

        if signing_packages.len() != tx.input.len() {
            return Err(Error::InvalidSigningPackage(
                "Number of inputs does not match signing packages",
            ));
        }

        for (index, psbt_input) in psbt.inputs.iter_mut().enumerate() {
            let mut signing_package = signing_packages.get(index).expect("valid index").clone();
            let eth_tweak = psbt_input.unknown.get(&ETH_ADDRESS_FIELD.clone());
            if let Some(e) = eth_tweak {
                signing_package.set_addtional_tweak(e.clone());
            };
            let partial_sig = partial_sigs.get(index).expect("valid index");
            let agg_sig = frost::aggregate(&signing_package, &partial_sig, &pk_package)?;

            // Skipping first byte which is encoding the parity of the y cord of R
            // We only use x-only elements. So we can skip this byte. FROST library only produces x-only keys / points
            // TODO (armins) remove the unwrap here
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
        if let Err(errs) = psbt.finalize_mut(secp) {
            error!("Had {} PSBT finalization errors:", errs.len());
            for e in &errs {
                error!("  PSBT finalization error: {}", e);
            }
            return Err(Error::PbstFinalizationFailed(errs));
        }
        // could do this once we are confident our code works and we don't
        // want to do the effort of tx verification
        // let tx = psbt.clone().extract_tx();
        let tx = psbt.extract(secp).map_err(|_| Error::InvaildResultingTx)?;

        Ok(tx)
    }
}
