use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use crate::{database, rpc, util::OutPointExt, App, Error};

use bdk::wallet::coin_selection::CoinSelectionAlgorithm;
use bitcoin::{consensus::Encodable, psbt::Psbt, FeeRate, OutPoint, ScriptBuf, TxOut};
use frost_secp256k1_tr as frost;
use reth_btc_wallet::TAPROOT_KEYSPEND_SATISFACTION_WEIGHT;

impl App {
    pub(crate) fn add_round2_signing(
        &self,
        payload: rpc::Round2SigningPackage,
    ) -> Result<(), Error> {
        self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        let frost_id = crate::util::deserialize_frost_peer_id(payload.identifier.clone())?;
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(Error::InvalidFrostPeerId);
        }

        let signature_share: [u8; 32] = payload.payload.as_slice().try_into().map_err(|e| {
            error!("Failed to deserialize round2 signing payload: {}", e);
            Error::InvalidRound2SigningPayload()
        })?;

        let partial_sig = frost::round2::SignatureShare::deserialize(signature_share)
            .map_err(|e| Error::InvalidRoundDkgPayload(e))?;

        // Checks if we have enough partial signatures
        let partial_sigs = self.db.get_round2_signing_packages().map_err(Error::Db)?;
        println!("Partial sigs: {:?}", partial_sigs);
        if partial_sigs.len() >= self.min_signers as usize {
            return Err(Error::AlreadyHaveQuorumOfPartialSignatures())
        }

        let psbt = Psbt::deserialize(payload.psbt.as_slice()).map_err(|e| {
            error!("Failed to deserialize psbt: {}", e);
            Error::PbstError(e)
        })?;
        let txid = psbt.extract_tx().txid();

        if self.db.add_round2_signing(txid, frost_id, partial_sig).map_err(Error::Db)? {
            self.db.flush().map_err(Error::Db)?;
            debug!("Stored round2 signing from peer: {:?}", frost_id);
        } else {
            warn!("Duplicate round2 signing from peer: {:?}", frost_id);
        }

        Ok(())
    }

    pub(crate) fn add_round1_signing(
        &self,
        payload: rpc::Round1SigningPackage,
    ) -> Result<(), Error> {
        self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        let frost_id = crate::util::deserialize_frost_peer_id(payload.identifier.clone())?;
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(Error::InvalidFrostPeerId);
        }

        let signing_round1 =
            frost::round1::SigningCommitments::deserialize(payload.payload.as_slice())
                .map_err(|e| Error::InvalidRoundDkgPayload(e))?;

        // Note: There doesn't need to be a check for a quorum of round1 signing packages
        // The more the better in the case one is unresponsive
        // the frost lib will check if we have enough when we create the signing package

        if self.db.add_round1_signing(frost_id, signing_round1).map_err(Error::Db)? {
            self.db.flush().map_err(Error::Db)?;
            debug!("Stored round1 signing from peer: {:?}", frost_id);
        } else {
            warn!("Duplicate round1 signing from peer: {:?}", frost_id);
        }

        Ok(())
    }

    pub(crate) fn get_public_key(&self) -> Result<frost::VerifyingKey, Error> {
        // try to get pk package from db incase we already did dkg round 3
        if let Some(pk_package) = self.db.get_public_key_package()? {
            return Ok(pk_package.verifying_key().to_owned());
        }

        let round1_packages = self.db.get_round1_dkg_packages()?;
        let round2_packages = self.db.get_round2_dkg_packages()?;
        if let Some(round2_secret) = self.frost_round2_dkg.lock().unwrap().clone() {
            let pk_res =
                frost::keys::dkg::part3(&round2_secret, &round1_packages, &round2_packages)?;

            self.db.set_key_package(pk_res.0.clone())?;
            self.db.set_pubkey_package(pk_res.1.clone())?;
            self.db.flush()?;
            return Ok(pk_res.1.verifying_key().to_owned());
        } else {
            return Err(Error::InvalidRound2DkgPayloadMissingPackage)
        }
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
                    utxo: bdk::Utxo::Local(bdk::LocalUtxo {
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

        let mut psbt = reth_btc_wallet::transaction::create_psbt(
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
        secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All>,
        outputs: Vec<TxOut>,
        fee_rate: FeeRate,
    ) -> Result<(frost::SigningPackage, Psbt), Error> {
        let pk_package = self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        let signing_commitments = self.db.get_round1_signing_packages()?;
        if signing_commitments.len() < self.min_signers as usize {
            return Err(Error::NotEnoughSigners)
        }

        let pk = hex::encode(pk_package.verifying_key().serialize());
        let secp_pk = bitcoin::secp256k1::PublicKey::from_str(pk.as_str()).expect("pk");
        let change_script =
            reth_btc_wallet::address::generate_taproot_change_scriptpubkey(secp, &secp_pk);

        let psbt = self.make_tx(outputs, fee_rate, change_script)?;
        let txid = psbt.clone().extract_tx().txid();

        let mut raw_tx: Vec<u8> = Vec::new();
        psbt.clone().extract_tx().consensus_encode(&mut raw_tx)?;
        // TODO (armins) calc sighash here
        let signing_package = frost::SigningPackage::new(signing_commitments, raw_tx.as_slice());

        // Lastly save this to sign package to the db
        self.db.add_signing_package(txid, signing_package.clone())?;
        Ok((signing_package, psbt))
    }

    pub(crate) fn finalize_signing(&self, psbt: &Psbt) -> Result<Psbt, Error> {
        let txid = psbt.clone().extract_tx().txid();

        let pk_package = self.db.get_public_key_package()?.ok_or(Error::MissingKeyPackage)?;

        let partial_sigs = self
            .db
            .get_round2_signing_package_txid(txid)?
            .ok_or(Error::MissingRound2SigningPackage)?;

        let signing_package =
            self.db.get_signing_package(txid)?.ok_or(Error::MissingSigningPackage)?;

        let agg_sig = frost::aggregate(&signing_package, &partial_sigs, &pk_package)?;

        // Verify signature -- redundant check
        pk_package.verifying_key().verify(signing_package.message(), &agg_sig)?;

         // TODO (armins) add agg signature to psbt
        // if let Err(err) =
        //     reth_btc_wallet::transaction::sign_psbt(&SECP, &self.key.secret_key(), &mut psbt)
        // {
        //     error!("Failed to sign psbt {:?}", err);
        //     return Err(Error::FailedToSignPbst)
        // }
        // TODO Add signature to psbt and finalize
        // TODO remove utxos being spent from db
        Ok(psbt.clone())
    }
}
