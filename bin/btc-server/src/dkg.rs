use crate::{App, DbError, Error};
use std::collections::BTreeMap;

use frost_secp256k1_tr as frost;

#[derive(Debug, Error)]
pub enum DKGError {
    #[error("already have key package")]
    AlreadyHaveKeyPackage,
    #[error("missing round1 dkg package")]
    MissingRound1DkgPackage,
    #[error("invalid frost peer id")]
    InvalidFrostPeerId,
    #[error("invalid round2 dkg payload missing package")]
    InvalidRound2DkgPayloadMissingPackage,
    #[error("cannot add own dkg package")]
    CannotAddOwnDkgPackage,
    #[error("dkg max signers reached")]
    DkgMaxSignersReached,
    #[error("internal FROST error: {0}")]
    FrostError(#[from] frost::Error),
    #[error("internal DB error")]
    DbError(#[from] DbError),
}

impl App {
    pub(crate) async fn get_round2_dkg(
        &self,
    ) -> Result<BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>, DKGError> {
        // Already have done dkg
        // This function shold error
        if self.db.get_key_package()?.is_some() {
            return Err(DKGError::AlreadyHaveKeyPackage);
        }

        if let Some(round1_dkg) = self.frost_round1_dkg.clone() {
            // Retrieve round 1 packages from peers
            // Here we dont check we have enough that should be done by the frost lib
            // So we just propogate the error
            let round1_packages = self.db.get_round1_dkg_packages()?;

            let (round2_secret_package, round2_packages) =
                frost::keys::dkg::part2(round1_dkg.0.clone(), &round1_packages)?;
            self.frost_round2_dkg.lock().await.replace(round2_secret_package.clone());

            Ok(round2_packages)
        } else {
            return Err(DKGError::MissingRound1DkgPackage);
        }
    }

    pub(crate) fn get_round1_dkg(&self) -> Result<frost::keys::dkg::round1::Package, DKGError> {
        // Already have done dkg
        // This function shold error
        if self.db.get_key_package()?.is_some() {
            return Err(DKGError::AlreadyHaveKeyPackage);
        }
        if let Some(round1_dkg) = self.frost_round1_dkg.clone() {
            Ok(round1_dkg.1)
        } else {
            return Err(DKGError::MissingRound1DkgPackage);
        }
    }

    pub(crate) async fn add_round2_dkg(
        &self,
        frost_id: frost::Identifier,
        packages: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>,
    ) -> Result<(), DKGError> {
        if self.db.get_key_package()?.is_some() {
            return Err(DKGError::AlreadyHaveKeyPackage);
        }
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(DKGError::InvalidFrostPeerId);
        }
        for (id, package) in packages.iter() {
            // Look for our package and store it
            if self.identifier == *id {
                if self.db.add_round2_dkg(frost_id, package.clone())? {
                    self.db.flush()?;
                    debug!("Stored round2 dkg from peer: {:?}", frost_id);
                } else {
                    warn!("Duplicate round2 dkg from peer: {:?}", frost_id);
                }
                // If we have a max_signers round2 packages we can generate and save the key package
                let round2_packages = self.db.get_round2_dkg_packages()?;
                if round2_packages.len() as u16 == self.max_signers - 1 {
                    let round1_packages = self.db.get_round1_dkg_packages()?;
                    if let Some(round2_secret) = self.frost_round2_dkg.lock().await.clone() {
                        let pk_res = frost::keys::dkg::part3(
                            &round2_secret,
                            &round1_packages,
                            &round2_packages,
                        )?;

                        self.db.set_key_package(pk_res.0.clone())?;
                        self.db.set_pubkey_package(pk_res.1.clone())?;
                        self.db.flush()?;
                    }
                }

                return Ok(());
            }
        }
        return Err(DKGError::InvalidRound2DkgPayloadMissingPackage);
    }

    pub(crate) fn add_round1_dkg(
        &self,
        frost_id: frost::Identifier,
        dkg_round1: frost::keys::dkg::round1::Package,
    ) -> Result<(), DKGError> {
        if self.db.get_key_package()?.is_some() {
            return Err(DKGError::AlreadyHaveKeyPackage);
        }
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(DKGError::CannotAddOwnDkgPackage);
        }

        if self.frost_round1_dkg.as_ref().take().expect("valid dkg round1").1 == dkg_round1 {
            return Err(DKGError::CannotAddOwnDkgPackage);
        }
        // Should not add if we have max signers
        if self.db.get_round1_dkg_packages()?.len() as u16 == self.max_signers - 1 {
            return Err(DKGError::DkgMaxSignersReached);
        }

        if self.db.add_round1_dkg(frost_id, dkg_round1)? {
            self.db.flush()?;
            debug!("Stored round1 dkg from peer: {:?}", frost_id);
        } else {
            warn!("Duplicate round1 dkg from peer: {:?}", frost_id);
        }

        Ok(())
    }
}
