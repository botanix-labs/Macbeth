use crate::{rpc, App, Error};
use bitcoin::psbt::Psbt;
use frost_secp256k1_tr as frost;
use rand::thread_rng;

impl App {
    pub(crate) fn get_round1_signing_package(
        &self,
    ) -> Result<frost::round1::SigningCommitments, Error> {
        let key_package = self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;
        // Get our secret package
        let secret = key_package.signing_share();

        let mut rng = thread_rng();
        let (signing_nonces, signing_commitments) = frost::round1::commit(secret, &mut rng);
        let _res = rpc::Round1SigningPackage {
            identifier: self.identifier.serialize().to_vec(),
            payload: signing_commitments.serialize()?.to_vec(),
        };

        self.frost_round1_signing_nonces.lock().unwrap().replace(signing_nonces.clone());

        Ok(signing_commitments)
    }

    pub(crate) fn get_round2_signing_package(
        &self,
        signing_package: frost::SigningPackage,
        _psbt: Psbt,
    ) -> Result<frost::round2::SignatureShare, Error> {
        // Important note here is that we never re-use the same nonce pairs for a different signing
        // request Should always generate new ones or if we are in a signing session refuse
        // to provide new ones
        let key_package = self.db.get_key_package()?.ok_or(Error::MissingKeyPackage)?;

        // Get signing nonces from round 1
        let signing_nonces = self
            .frost_round1_signing_nonces
            .lock()
            // TODO (armins) remove unwrap
            .unwrap()
            .clone()
            .ok_or(Error::MissingRound1SigningNonce)?;

        // TODO verify psbt
        // TODO verify message
        // TODO need to sign for each input SIG_HASH_SINGLE
        let partial_sig = frost::round2::sign(&signing_package, &signing_nonces, &key_package)?;

        Ok(partial_sig)
    }
}
