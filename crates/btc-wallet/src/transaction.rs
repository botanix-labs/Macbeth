use bitcoin::{
    psbt::{self, PartiallySignedTransaction},
    secp256k1::{KeyPair, SecretKey},
    sighash::TapSighashType,
    taproot::{LeafVersion, TapLeafHash},
    OutPoint, TxOut,
};

use crate::address::{
    build_safe_spend_path_scriptbuf, generate_taproot_spend_info, generate_tweaked_secret_key,
};

const USER_ETH_ADDRESS_FIELD: u8 = 1;

static ETH_ADDRESS_FIELD: psbt::raw::ProprietaryKey = psbt::raw::ProprietaryKey {
    prefix: Vec::new(),
    subtype: USER_ETH_ADDRESS_FIELD,
    key: Vec::new(),
};

/// Utxo DTO struct
pub struct Input {
    pub outpoint: OutPoint,
    pub output: TxOut,
    pub eth_address: Option<[u8; 20]>,
}

/// Create psbt with proprietary tweak fields
pub fn create_psbt(
    inputs: Vec<Input>,
    outputs: Vec<TxOut>,
    change: Option<TxOut>,
) -> PartiallySignedTransaction {
    let tx = bitcoin::Transaction {
        version: 2,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: inputs
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint,
                sequence: bitcoin::Sequence::MAX,
                script_sig: bitcoin::ScriptBuf::new(),
                witness: Default::default(),
            })
            .collect(),
        output: {
            let mut ret = outputs;
            if let Some(change) = change.clone() {
                ret.push(change);
            }
            ret
        },
    };

    // Create PSBT
    let mut psbt = PartiallySignedTransaction::from_unsigned_tx(tx).expect("tx is unsigned");
    for (psbt, utxo) in psbt.inputs.iter_mut().zip(inputs.iter()) {
        psbt.witness_utxo = Some(utxo.output.clone());
        // store the user tweak if used
        if utxo.eth_address.is_some() {
            psbt.proprietary.insert(
                ETH_ADDRESS_FIELD.clone(),
                utxo.eth_address.expect("have eth address").to_vec(),
            );
        }
    }

    psbt
}

#[derive(Debug)]
pub enum SignPsbtError {
    NonceProvidedMissingEthTweak,
    FailedToGetTaprootInfo(bitcoin::taproot::Error),
}

pub fn sign_psbt(
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All>,
    secret_key: &SecretKey,
    psbt: &mut PartiallySignedTransaction,
) -> Result<(), SignPsbtError> {
    let mut sighashcache = bitcoin::sighash::SighashCache::new(&psbt.unsigned_tx);
    for i in 0..psbt.inputs.len() {
        let input = &psbt.inputs[i];
        // Get address tweaks if applicaple
        let eth_address_tweak = input.proprietary.get(&ETH_ADDRESS_FIELD);
        let aggregate_pk = secret_key.public_key(&secp);
        let mut internal_sk = secret_key.clone();

        // Not signing change
        // So we need to tweak the key before signing
        if eth_address_tweak.is_some() {
            let eth_address = ethers::types::Address::from_slice(
                &eth_address_tweak.expect("eth address tweak").as_slice(),
            );

            internal_sk = generate_tweaked_secret_key(&eth_address, &aggregate_pk, &secret_key);
        }

        let internal = KeyPair::from_secret_key(&secp, &internal_sk);
        let taproot_spend_info = generate_taproot_spend_info(&secp, &internal.public_key())
            .map_err(|e| SignPsbtError::FailedToGetTaprootInfo(e))?;
        let keypair = bitcoin::key::TapTweak::tap_tweak(
            internal.clone(),
            &secp,
            taproot_spend_info.merkle_root(),
        );
        let signature = {
            let prevouts =
                psbt.inputs.iter().map(|i| i.witness_utxo.as_ref().unwrap()).collect::<Vec<_>>();
            let sighash = sighashcache
                .taproot_signature_hash(
                    i,
                    &psbt::Prevouts::All(&prevouts),
                    None, // annex
                    None, // leaf_hash_code_separator
                    TapSighashType::All,
                )
                .expect("error calculating taproot keyspend sighash");
            let msg = bitcoin::secp256k1::Message::from_slice(&sighash[..]).expect("sane sighash");
            let sig = secp.sign_schnorr(&msg, &keypair.to_inner());
            bitcoin::taproot::Signature { sig, hash_ty: TapSighashType::All }
        };
        // modify the psbt input by placing the signature
        psbt.inputs.get_mut(i).unwrap().sighash_type = Some(TapSighashType::All.into());
        psbt.inputs.get_mut(i).unwrap().tap_internal_key = Some(internal.x_only_public_key().0);
        psbt.inputs.get_mut(i).unwrap().tap_key_sig = Some(signature);
        psbt.inputs.get_mut(i).unwrap().tap_merkle_root = taproot_spend_info.merkle_root()
    }

    Ok(())
}

#[derive(Debug)]
pub enum SignSafeSpendPathError {
    NonceProvidedMissingEthTweak,
    FailedToGetTaprootInfo(bitcoin::taproot::Error),
}

pub fn sign_spend_path(
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::All>,
    secret_key: &SecretKey,
    psbt: &mut PartiallySignedTransaction,
) {
    let mut sighashcache = bitcoin::sighash::SighashCache::new(&psbt.unsigned_tx);
    for i in 0..psbt.inputs.len() {
        let input = &psbt.inputs[i];
        // Get address tweaks if applicaple
        let eth_address_tweak = input.proprietary.get(&ETH_ADDRESS_FIELD);
        let aggregate_pk = secret_key.public_key(&secp);
        let mut internal_sk = secret_key.clone();

        // need to tweak the key before signing
        if eth_address_tweak.is_some() {
            let eth_address = ethers::types::Address::from_slice(
                &eth_address_tweak.expect("eth address tweak").as_slice(),
            );

            internal_sk = generate_tweaked_secret_key(&eth_address, &aggregate_pk, &secret_key);
        }

        let internal_key = KeyPair::from_secret_key(&secp, &internal_sk);
        let taproot_spend_info = generate_taproot_spend_info(&secp, &internal_key.public_key())
            .map_err(|e| SignPsbtError::FailedToGetTaprootInfo(e))
            .unwrap();

        let keypair = bitcoin::key::TapTweak::tap_tweak(
            internal_key.clone(),
            &secp,
            taproot_spend_info.merkle_root(),
        );
        let ssp_scriptbuf = build_safe_spend_path_scriptbuf().unwrap();
        let control_block = taproot_spend_info
            .control_block(&(ssp_scriptbuf, LeafVersion::TapScript))
            .expect("valid control block");
        let tap_leaf_hash = TapLeafHash::from_script(&ssp_scriptbuf, LeafVersion::TapScript);

        let signature = {
            let prevouts =
                psbt.inputs.iter().map(|i| i.witness_utxo.as_ref().unwrap()).collect::<Vec<_>>();
            let sighash = sighashcache
                .taproot_signature_hash(
                    i,
                    &psbt::Prevouts::All(&prevouts),
                    None, // always leave annex None
                    // https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki#cite_note-5
                    Some((tap_leaf_hash, 0u32)),
                    TapSighashType::All,
                )
                .expect("error calculating taproot keyspend sighash");
            let msg = bitcoin::secp256k1::Message::from_slice(&sighash[..]).expect("sane sighash");
            let sig = secp.sign_schnorr(&msg, &keypair.to_inner());
            bitcoin::taproot::Signature { sig, hash_ty: TapSighashType::All }
        };
        // modify the psbt input by placing the signature
        psbt.inputs.get_mut(i).unwrap().sighash_type = Some(TapSighashType::All.into());
        psbt.inputs.get_mut(i).unwrap().tap_internal_key = Some(internal_key.x_only_public_key().0);

        // TODO include the control block in the psbt
        psbt.inputs.get_mut(i).unwrap().tap_key_sig = Some(signature);
        psbt.inputs.get_mut(i).unwrap().tap_merkle_root = taproot_spend_info.merkle_root()
    }
}
