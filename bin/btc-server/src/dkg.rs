use std::collections::BTreeMap;

use crate::{rpc, App, Error};

use frost_secp256k1_tr as frost;

impl App {
    pub(crate) fn get_round2_dkg(
        &self,
    ) -> Result<BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>, Error> {
        // Already have done dkg
        // This function shold error
        if self.db.get_key_package()?.is_some() {
            return Err(Error::AlreadyHaveKeyPackage);
        }

        if let Some(round1_dkg) = self.frost_round1_dkg.clone() {
            // Retrieve round 1 packages from peers
            // Here we dont check we have enough that should be done by the frost lib
            // So we just propogate the error
            let round1_packages = self.db.get_round1_dkg_packages()?;

            let (round2_secret_package, round2_packages) =
                frost::keys::dkg::part2(round1_dkg.0.clone(), &round1_packages)?;
            self.frost_round2_dkg.lock().unwrap().replace(round2_secret_package.clone());

            Ok(round2_packages)
        } else {
            return Err(Error::MissingRound1DkgPackage);
        }
    }

    pub(crate) fn get_round1_dkg(&self) -> Result<frost::keys::dkg::round1::Package, Error> {
        // Already have done dkg
        // This function shold error
        if self.db.get_key_package()?.is_some() {
            return Err(Error::AlreadyHaveKeyPackage);
        }
        if let Some(round1_dkg) = self.frost_round1_dkg.clone() {
            Ok(round1_dkg.1)
        } else {
            return Err(Error::MissingRound1DkgPackage);
        }
    }

    pub(crate) fn add_round2_dkg(
        &self,
        frost_id: frost::Identifier,
        packages: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package>,
    ) -> Result<(), Error> {
        if self.db.get_key_package()?.is_some() {
            return Err(Error::AlreadyHaveKeyPackage);
        }
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(Error::InvalidFrostPeerId);
        }
        for (id, package) in packages.iter() {
            // Look for our package and store it
            if self.identifier == *id {
                if self.db.add_round2_dkg(frost_id, package.clone()).map_err(Error::Db)? {
                    self.db.flush().map_err(Error::Db)?;
                    debug!("Stored round2 dkg from peer: {:?}", frost_id);
                } else {
                    warn!("Duplicate round2 dkg from peer: {:?}", frost_id);
                }
                // If we have a max_signers round2 packages we can generate and save the key package
                let round2_packages = self.db.get_round2_dkg_packages()?;
                if round2_packages.len() as u16 == self.max_signers - 1 {
                    let round1_packages = self.db.get_round1_dkg_packages()?;
                    if let Some(round2_secret) = self.frost_round2_dkg.lock().unwrap().clone() {
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
        return Err(Error::InvalidRound2DkgPayloadMissingPackage);
    }

    pub(crate) fn add_round1_dkg(
        &self,
        frost_id: frost::Identifier,
        dkg_round1: frost::keys::dkg::round1::Package,
    ) -> Result<(), Error> {
        if self.db.get_key_package()?.is_some() {
            return Err(Error::AlreadyHaveKeyPackage);
        }
        // Can't add our selves
        if frost_id == self.identifier {
            return Err(Error::CannotAddOwnDkgPackage);
        }

        if self.frost_round1_dkg.as_ref().take().expect("valid dkg round1").1 == dkg_round1 {
            return Err(Error::CannotAddOwnDkgPackage);
        }
        // Should not add if we have max signers
        if self.db.get_round1_dkg_packages()?.len() as u16 == self.max_signers - 1 {
            return Err(Error::DkgMaxSignersReached);
        }

        if self.db.add_round1_dkg(frost_id, dkg_round1).map_err(Error::Db)? {
            self.db.flush().map_err(Error::Db)?;
            debug!("Stored round1 dkg from peer: {:?}", frost_id);
        } else {
            warn!("Duplicate round1 dkg from peer: {:?}", frost_id);
        }

        Ok(())
    }
}
