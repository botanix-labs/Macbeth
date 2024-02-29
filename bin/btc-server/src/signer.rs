use crate::DbError;
use crate::{App, Error};
use bitcoin::psbt::Psbt;
use frost_secp256k1_tr as frost;
use rand::thread_rng;
use reth_btc_wallet::transaction::ETH_ADDRESS_FIELD;

#[derive(Debug)]
pub enum SigningError {
    Round1(SigningRound1Error),
    Round2(SigningRound2Error),
}

impl From<SigningError> for Error {
    fn from(e: SigningError) -> Error {
        match e {
            SigningError::Round1(e) => Error::Signing(SigningError::Round1(e)),
            SigningError::Round2(e) => Error::Signing(SigningError::Round2(e)),
        }
    }
}

#[derive(Debug, Error)]
pub enum SigningRound1Error {
    #[error("already in signing session")]
    AlreadyInSigningSession,
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid number of signing nonces requested")]
    InvalidNumberOfNoncesRequested,
    #[error("internal DB error")]
    DbError(#[from] DbError),
}

#[derive(Debug, Error)]
pub enum SigningRound2Error {
    #[error("missing key package")]
    MissingKeyPackage,
    #[error("invalid signing package")]
    InvalidSigningPackage(&'static str),
    #[error("internal FROST error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("missing round 1 signing nonces")]
    MissingRound1SigningNonce,
    #[error("internal DB error")]
    DbError(#[from] DbError),
}

impl From<SigningRound1Error> for SigningError {
    fn from(e: SigningRound1Error) -> Self {
        SigningError::Round1(e)
    }
}

impl From<SigningRound2Error> for SigningError {
    fn from(e: SigningRound2Error) -> Self {
        SigningError::Round2(e)
    }
}

impl App {
    pub(crate) fn get_round1_signing_package(
        &self,
        number_of_nonces: u32,
        _signing_session_id: &[u8; 32],
    ) -> Result<Vec<frost::round1::SigningCommitments>, SigningRound1Error> {
        if number_of_nonces == 0 || number_of_nonces > 15 {
            return Err(SigningRound1Error::InvalidNumberOfNoncesRequested);
        }
        // Check if have already provided nonces for the current session
        if self.frost_round1_signing_nonces.lock().unwrap().is_some() {
            return Err(SigningRound1Error::AlreadyInSigningSession);
        }

        let key_package =
            self.db.get_key_package()?.ok_or(SigningRound1Error::MissingKeyPackage)?;
        // Get our secret package
        let secret = key_package.signing_share();
        let mut nonces = vec![];

        let mut rng = thread_rng();
        // Order here is important for both the signer and cordinator
        // Each nonce pair is commitment to a input of the tx
        // When the signing package is produced the signer should be careful to
        // Verify that the nonce pairs are in the same order as the inputs
        for _ in 0..number_of_nonces {
            let nonce_pkg = frost::round1::commit(secret, &mut rng);
            nonces.push(nonce_pkg);
        }

        let signing_commitments =
            nonces.iter().map(|nonce| nonce.1).collect::<Vec<frost::round1::SigningCommitments>>();
        let signing_nonces = nonces
            .iter()
            .map(|nonce| nonce.0.clone())
            .collect::<Vec<frost::round1::SigningNonces>>();

        self.frost_round1_signing_nonces.lock().unwrap().replace(signing_nonces.clone());

        Ok(signing_commitments)
    }

    pub(crate) fn get_round2_signing_package(
        &self,
        signing_packages: &mut Vec<frost::SigningPackage>,
        psbt: &Psbt,
    ) -> Result<Vec<frost::round2::SignatureShare>, SigningRound2Error> {
        // Important note here is that we never re-use the same nonce pairs for a different signing
        // request Should always generate new ones or if we are in a signing session refuse
        // to provide new ones
        let key_package =
            self.db.get_key_package()?.ok_or(SigningRound2Error::MissingKeyPackage)?;
        let tx = psbt.clone().extract_tx();
        let num_inputs = tx.input.len();
        // The number of inputs on the psbt should match the number of signing packages
        if num_inputs != signing_packages.len() {
            return Err(SigningRound2Error::InvalidSigningPackage(
                "Number of inputs does not match number of signing packages",
            ));
        }

        // Get signing nonces from round 1
        let signing_nonces = self
            .frost_round1_signing_nonces
            .lock()
            // TODO (armins) remove unwrap
            .unwrap()
            .clone()
            .ok_or(SigningRound2Error::MissingRound1SigningNonce)?;

        if signing_nonces.len() != num_inputs {
            return Err(SigningRound2Error::InvalidSigningPackage(
                "Number of signing nonces does not match number of inputs",
            ));
        }

        // TODO verify psbt
        // TODO verify message
        // TODO verify that my nonce commitments are being used
        // TODO need to sign for each input SIG_HASH_SINGLE

        // TODO(armmins) this code needs to handle signing for multiple inputs
        // Get a parital sig for each input
        let mut partial_sigs = vec![];
        for (index, (signing_package, _txin)) in
            signing_packages.iter_mut().zip(tx.input.iter()).enumerate()
        {
            // get the eth tweak from the psbt unknown fields
            let eth_tweak = psbt.inputs.get(index).unwrap().unknown.get(&ETH_ADDRESS_FIELD.clone());
            if let Some(e) = eth_tweak {
                signing_package.set_addtional_tweak(e.clone());
            };

            partial_sigs.push(frost::round2::sign(
                &signing_package,
                &signing_nonces.get(index).expect("valid index"),
                &key_package,
            )?);
        }

        // Clear the signing nonces
        // This finalizes the signing session
        self.frost_round1_signing_nonces.lock().unwrap().take();
        Ok(partial_sigs)
    }
}
