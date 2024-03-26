use std::collections::BTreeMap;
use std::str::FromStr;

use base64::decode as base64_decode;
use bitcoin::consensus::encode as btcencode;
use bitcoin::psbt::Psbt;
use bitcoin::{FeeRate, OutPoint, TxOut};
use bitcoincore_rpc::{json::EstimateMode, RpcApi};
use frost_secp256k1_tr as frost;
use reth_primitives::hex::decode as hex_decode;
use tonic;
use tonic::metadata::BinaryMetadataKey;
use util::{parse_eth_address, VerifyingKeyExt};

use crate::database::Utxo;
use crate::{rpc, util, App, SECP};

const JWT_HEADER_KEY: &'static str = "jwt-auth";

macro_rules! badarg {
    ($($arg:tt)*) => {{
        tonic::Status::invalid_argument(format!($($arg)*))
    }};
}

macro_rules! already_exists {
    ($($arg:tt)*) => {{
        tonic::Status::already_exists(format!($($arg)*))
    }};
}

macro_rules! internal {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        error!("INTERNAL ERROR: {}", msg);
        tonic::Status::internal(format!("internal error: {}", msg))
    }};
}

macro_rules! unauthenticated {
    ($($arg:tt)*) => {{
        tonic::Status::unauthenticated(format!($($arg)*))
    }};
}

trait ToStatus<T> {
    fn to_status(self) -> Result<T, tonic::Status>;
}

impl<T> ToStatus<T> for Result<T, crate::Error> {
    fn to_status(self) -> Result<T, tonic::Status> {
        self.map_err(|e| internal!("{}", e))
    }
}

impl App {
    fn validate_jwt<T>(&self, request: &tonic::Request<T>) -> Result<(), tonic::Status> {
        if let Some(jwt_secret) = self.jwt_secret.as_ref() {
            let key = BinaryMetadataKey::from_static(JWT_HEADER_KEY);
            if let Some(metadata_value) = request.metadata().get_bin(key) {
                let jwt_request_token_received = metadata_value.as_encoded_bytes();
                let jwt_token_base64_decoded =
                    base64_decode(jwt_request_token_received).map_err(|e| {
                        error!("Failed to base64 decode request metadata: {}", e);
                        badarg!("Failed to base64 decode request metadata: {}", e)
                    })?;
                let jwt_token_hex_decoded = hex_decode(jwt_token_base64_decoded).map_err(|e| {
                    error!("Failed to hex decode jwt value: {}", e);
                    badarg!("Failed to hex decode jwt value: {}", e)
                })?;
                let jwt_stringified = String::from_utf8(jwt_token_hex_decoded).map_err(|e| {
                    error!("Failed to utf8 decode jwt value: {}", e);
                    badarg!("Failed to utf8 decode jwt value: {}", e)
                })?;

                if jwt_secret.validate(jwt_stringified).is_err() {
                    error!("Request authentication failed");
                    unauthenticated!("Request authentication failed");
                }
            }
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl rpc::BtcServer for App {
    // Saves peg'd in UTXO
    async fn notify_pegin(
        &self,
        req: tonic::Request<rpc::NotifyPeginRequest>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let txid = req.utxo_txid.parse().map_err(|e| badarg!("bad txid: {}", e))?;
        let outpoint = OutPoint::new(txid, req.utxo_vout);

        let eth_addr = parse_eth_address(req.eth_address).map_err(|e| {
            error!("Failed to parse eth address: {}", e);
            badarg!("Failed to parse eth address: {}", e)
        })?;
        let utxo = Utxo::new(
            outpoint,
            btcencode::deserialize(&req.output).map_err(|e| badarg!("bad txout format: {}", e))?,
            Some(eth_addr),
        );

        self.add_pegin(&utxo).map_err(|e| internal!("Failed to add pegin: {}", e))?;

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn finalize_signing(
        &self,
        req: tonic::Request<rpc::FinalizeSigningRequest>,
    ) -> Result<tonic::Response<rpc::FinalizeSigningResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;

        let psbt = self
            .finalize_signing(&signing_session_id)
            .await
            .map_err(|e| internal!("Failed to finalize signing: {}", e))?;

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        // let res = tonic::Response::new(rpc::FinalizeSigningResponse { transaction: psbt_bytes });
        let res = tonic::Response::new(rpc::FinalizeSigningResponse { psbt: psbt_bytes });

        Ok(res)
    }

    async fn get_psbt(
        &self,
        req: tonic::Request<rpc::MakeTxRequest>,
    ) -> Result<tonic::Response<rpc::SignPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;

        let bitcoind_rpc = self.bitcoind_client.as_ref().expect("bitcoind client");
        let fee_res = bitcoind_rpc.estimate_smart_fee(1, Some(EstimateMode::Conservative));
        let mut fee_rate = self.fall_back_fee_rate;
        if let Ok(fee) = fee_res {
            if let Some(f) = fee.fee_rate {
                fee_rate = FeeRate::from_sat_per_kwu(f.to_sat() / 4);
            }
        }

        debug!("Cord Fee rate: {:?}", fee_rate);

        let outputs = req
            .outputs
            .into_iter()
            .map(|o| {
                let script_pubkey_result = bitcoin::Address::from_str(&o.address)
                    .map_err(|e| internal!("invalid address: {}", e))?
                    .assume_checked()
                    .script_pubkey();

                Ok(TxOut { script_pubkey: script_pubkey_result, value: o.value })
            })
            .collect::<Result<Vec<TxOut>, tonic::Status>>()?;

        // TODO this should live in coordinator.rs
        let pk_package = self
            .db
            .get_key_package()?
            .ok_or_else(|| internal!("missing key package, run the dkg process first"))?;

        let secp_pk = pk_package
            .verifying_key()
            .to_secp_pk()
            .map_err(|e| internal!("Failed to convert verifying key to secp pk: {}", e))?;
        let change_script =
            reth_btc_wallet::address::generate_taproot_change_scriptpubkey(&SECP, &secp_pk);

        let psbt = self
            .make_tx(outputs, fee_rate, change_script)
            .map_err(|e| internal!("Failed to make tx: {}", e))?;

        // Save psbt to db
        self.db.update_psbt(&signing_session_id, &psbt)?;
        self.db.flush()?;

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;
        let res = tonic::Response::new(rpc::SignPayload {
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        });
        Ok(res)
    }

    async fn get_to_sign_package(
        &self,
        req: tonic::Request<rpc::ToSignRequest>,
    ) -> Result<tonic::Response<rpc::SignPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;
        let psbt = self
            .get_to_sign(&signing_session_id)
            .map_err(|e| internal!("Failed to get to sign: {}", e))?;

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;
        let res = tonic::Response::new(rpc::SignPayload {
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        });
        Ok(res)
    }

    async fn new_round1_signing_package(
        &self,
        req: tonic::Request<rpc::Round1SigningPackage>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;
        let frost_id = util::deserialize_frost_peer_id(req.identifier).map_err(|e| {
            error!("Failed to parse frost peer id: {}", e);
            badarg!("Failed to parse frost peer id: {}", e)
        })?;

        let psbt = Psbt::deserialize(req.psbt.as_slice())
            .map_err(|e| internal!("Failed to deserialize psbt: {}", e))?;

        self.add_round1_signing(&signing_session_id, frost_id, &psbt).map_err(|e| {
            error!("Failed to add round1 signing: {}", e);
            badarg!("Failed to add round1 signing")
        })?;

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn new_round2_signing_package(
        &self,
        req: tonic::Request<rpc::Round2SigningPackage>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;
        let frost_id = util::deserialize_frost_peer_id(req.identifier).map_err(|e| {
            error!("Failed to parse frost peer id: {}", e);
            badarg!("Failed to parse frost peer id: {}", e)
        })?;

        let psbt = Psbt::deserialize(req.psbt.as_slice())
            .map_err(|e| internal!("Failed to deserialize psbt: {}", e))?;

        self.add_round2_signing(&signing_session_id, frost_id, &psbt).map_err(|e| {
            error!("Failed to add round2 signing: {}", e);
            badarg!("Failed to add round2 signing")
        })?;

        Ok(tonic::Response::new(rpc::Empty {}))
    }

    async fn get_public_key(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetPublicKeyResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let pk = self.get_public_key().map_err(|e| internal!("Failed to get public key: {}", e))?;
        let pk = hex::encode(pk.serialize());

        return Ok(tonic::Response::new(rpc::GetPublicKeyResponse { publickey: pk }));
    }

    async fn get_gateway_address(
        &self,
        req: tonic::Request<rpc::GetGatewayAddressRequest>,
    ) -> Result<tonic::Response<rpc::GetGatewayAddressResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let eth_address = parse_eth_address(req.eth_address).map_err(|e| {
            error!("Failed to parse eth address: {}", e);
            badarg!("Failed to parse eth address: {}", e)
        })?;
        let pk_packages = self
            .get_gateway_address(&eth_address)
            .map_err(|e| internal!("Failed to get public key: {}", e))?;
        let pk = hex::encode(pk_packages.0.serialize());
        let pk_tweaked = hex::encode(pk_packages.1.serialize());
        let address = pk_packages.2.to_string();

        return Ok(tonic::Response::new(rpc::GetGatewayAddressResponse {
            publickey: pk,
            tweaked_public_key: pk_tweaked,
            gateway_address: address,
        }));
    }

    /// Adds round2 packages received from a peer
    async fn new_round2_dkg_package(
        &self,
        req: tonic::Request<rpc::DkgPayload>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let frost_id = util::deserialize_frost_peer_id(req.identifier).map_err(|e| {
            error!("Failed to parse frost peer id: {}", e);
            badarg!("Failed to parse frost peer id: {}", e)
        })?;
        let packages: BTreeMap<frost::Identifier, frost::keys::dkg::round2::Package> =
            serde_json::from_slice(req.payload.as_slice()).map_err(|e| {
                error!("Failed to deserialize round2 dkg package: {}", e);
                badarg!("Failed to deserialize round2 dkg package: {}", e)
            })?;

        self.add_round2_dkg(frost_id, packages).await.map_err(|e| {
            error!("Failed to add round2 dkg: {}", e);
            badarg!("Failed to add round2 dkg")
        })?;
        Ok(tonic::Response::new(rpc::Empty {}))
    }

    /// Generates a hashmap of round2 packages for sending to all other peers (needs round 1
    /// packages)
    async fn get_round2_dkg_package(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        let round2_packages = self
            .get_round2_dkg()
            .await
            .map_err(|e| internal!("Failed to get round2 dkg package: {}", e))?;
        let json = serde_json::to_string(&round2_packages).unwrap();
        let res = rpc::DkgPayload {
            identifier: self.identifier.serialize().to_vec(),
            payload: json.as_bytes().to_vec(),
        };
        Ok(tonic::Response::new(res))
    }

    /// Adds round 1 pkg received from another peer to our own state
    async fn new_round1_dkg_package(
        &self,
        req: tonic::Request<rpc::DkgPayload>,
    ) -> Result<tonic::Response<rpc::Empty>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let frost_id =
            crate::util::deserialize_frost_peer_id(req.identifier.clone()).map_err(|e| {
                error!("Failed to parse frost peer id: {}", e);
                badarg!("Failed to parse frost peer id: {}", e)
            })?;

        let dkg_round1 = frost::keys::dkg::round1::Package::deserialize(req.payload.as_slice())
            .map_err(|e| {
                error!("Failed to deserialize round1 dkg package: {}", e);
                badarg!("Failed to deserialize round1 dkg package: {}", e)
            })?;

        self.add_round1_dkg(frost_id, dkg_round1)
            .map_err(|_e| internal!("Failed to add round1 dkg"))?;
        Ok(tonic::Response::new(rpc::Empty {}))
    }

    /// Gets round 1 pkg we have generated (to be sent to another peer) - default when we start the
    /// btc server
    async fn get_round1_dkg_package(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        let round1_dkg_package = self
            .get_round1_dkg()
            .map_err(|e| internal!("Failed to get round1 dkg package: {}", e))?;

        let res = rpc::DkgPayload {
            identifier: self.identifier.serialize().to_vec(),
            payload: round1_dkg_package
                .serialize()
                .map_err(|e| internal!("Failed to serialize round1 dkg package: {}", e))?
                .to_vec(),
        };

        Ok(tonic::Response::new(res))
    }

    /// Gets round 1 pkgs we have collected so far - includes our own package
    async fn get_round1_dkg_packages(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::DkgPayload>, tonic::Status> {
        self.validate_jwt(&req)?;
        if self.db.get_public_key_package()?.is_some() {
            warn!("recieved notification about round 2 DKG while having key package");
            return Err(already_exists!("already have key package"));
        }

        let round1_packages = self
            .db
            .get_round1_dkg_packages()
            .map_err(|e| internal!("Failed to get round1 dkg packages: {}", e))?;

        let json = serde_json::to_string(&round1_packages).unwrap();
        let res = rpc::DkgPayload {
            identifier: self.identifier.serialize().to_vec(),
            payload: json.as_bytes().to_vec(),
        };
        Ok(tonic::Response::new(res))
    }

    /// Endpoint responds with a nonce commitments for a ONE particular signings session
    async fn get_round1_signing_package(
        &self,
        req: tonic::Request<rpc::Round1SigningPackageRequest>,
    ) -> Result<tonic::Response<rpc::Round1SigningPackage>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;

        let mut psbt = Psbt::deserialize(req.psbt.as_slice())
            .map_err(|e| internal!("Failed to deserialize psbt: {}", e))?;

        let bitcoind = self.bitcoind_client.as_ref().expect("bitcoind client");
        self.get_round1_signing_package(&mut psbt, &signing_session_id, bitcoind)
            .await
            .map_err(|e| internal!("Failed to get round1 signing package: {}", e))?;

        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;

        let res = rpc::Round1SigningPackage {
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        };

        Ok(tonic::Response::new(res))
    }

    async fn get_round2_signing_package(
        &self,
        req: tonic::Request<rpc::SignPayload>,
    ) -> Result<tonic::Response<rpc::Round2SigningPackage>, tonic::Status> {
        self.validate_jwt(&req)?;
        let req = req.into_inner();
        let signing_session_id =
            util::parse_signing_session_id(&req.signing_session_id).map_err(|e| {
                error!("Failed to parse signing session id: {}", e);
                badarg!("Failed to parse signing session id: {}", e)
            })?;
        let mut psbt = Psbt::deserialize(req.psbt.as_slice())
            .map_err(|e| internal!("Failed to deserialize psbt: {}", e))?;
        let _partial_signature = self
            .get_round2_signing_package(&mut psbt)
            .await
            .map_err(|e| internal!("Failed to get round2 signing package: {}", e))?;
        let psbt_bytes = hex::decode(psbt.serialize_hex())
            .map_err(|e| internal!("Failed to serialize psbt: {}", e))?;
        let res = rpc::Round2SigningPackage {
            identifier: self.identifier.serialize().to_vec(),
            psbt: psbt_bytes,
            signing_session_id: signing_session_id.to_vec(),
        };

        Ok(tonic::Response::new(res))
    }

    async fn signer_finalize(
        &self,
        req: tonic::Request<rpc::FinalizeSignerRequest>,
    ) -> Result<tonic::Response<rpc::FinalizeSigningResponse>, tonic::Status> {
        let req = req.into_inner();
        let fee_res = self
            .bitcoind_client
            .as_ref()
            .expect("instatiated bitcoin rpc")
            .estimate_smart_fee(1, Some(EstimateMode::Conservative));
        let mut fee_rate = self.fall_back_fee_rate;
        if let Ok(fee) = fee_res {
            if let Some(f) = fee.fee_rate {
                fee_rate = FeeRate::from_sat_per_kwu(f.to_sat() / 4);
            }
        }
        let outputs_result: Result<Vec<TxOut>, tonic::Status> = req
            .outputs
            .into_iter()
            .map(|o| {
                let script_pubkey_result = bitcoin::Address::from_str(&o.address)
                    .map_err(|e| internal!("invalid address: {}", e))?
                    .assume_checked()
                    .script_pubkey();

                Ok(TxOut { script_pubkey: script_pubkey_result, value: o.value })
            })
            .collect();

        // Witnesses can be get big. Remove this clone()
        let witnesses = req.witness;
        let psbt = self.finalize_signer(outputs_result?, fee_rate, witnesses).map_err(|e| {
            internal!("Failed to finalize signer: {}", e)
        })?;
        let psbt_bytes = hex::decode(psbt.serialize_hex()).map_err(|e| {
            internal!("Failed to serialize psbt: {}", e)
        })?;

        let res = tonic::Response::new(rpc::FinalizeSigningResponse {
            psbt: bitcoin::consensus::encode::serialize(&psbt_bytes),
        });
        Ok(res)
    }

    async fn get_all_utxos(
        &self,
        req: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetAllUtxosResponse>, tonic::Status> {
        self.validate_jwt(&req)?;
        let db_utxos =
            self.db.get_all_utxos().map_err(|e| internal!("Failed to get utxos: {}", e))?;
        let utxos = db_utxos.into_iter().map(|utxo| utxo.into()).collect::<Vec<rpc::Utxo>>();
        let res = rpc::GetAllUtxosResponse { utxos };

        Ok(tonic::Response::new(res))
    }

    async fn remove_utxo(
        &self,
        request: tonic::Request<rpc::RemoveUtxoRequest>,
    ) -> Result<tonic::Response<rpc::RemoveUtxoResponse>, tonic::Status> {
        self.validate_jwt(&request)?;
        let req = request.into_inner();

        let txid = bitcoin::Txid::from_str(&req.txid)
            .map_err(|e| tonic::Status::invalid_argument(format!("Invalid txid: {}", e)))?;

        let outpoint = bitcoin::OutPoint::new(txid, req.vout);

        match self.db.remove_utxo(outpoint) {
            Ok(_) => Ok(tonic::Response::new(rpc::RemoveUtxoResponse {
                success: true,
                message: "UTXO removed successfully".to_string(),
            })),
            Err(e) => Err(internal!("Failed to remove UTXO: {}", e)),
        }
    }
    // Gets the merkle root of the utxo set
    async fn get_utxo_merkle_root(
        &self,
        request: tonic::Request<rpc::Empty>,
    ) -> Result<tonic::Response<rpc::GetUtxoMerkleRootResponse>, tonic::Status> {
        self.validate_jwt(&request)?;
        match self.db.get_utxo_merkle_root() {
            Ok(Some(merkle_root)) => {
                // Successfully found the merkle root, return it
                let response = rpc::GetUtxoMerkleRootResponse { merkle_root: merkle_root.to_vec() };
                Ok(tonic::Response::new(response))
            }
            Ok(None) => Err(tonic::Status::not_found("UTXO Merkle root not found.")),
            // An error occurred while accessing the database
            Err(e) => Err(internal!("Failed to retrieve UTXO Merkle root: {}", e)),
        }
    }
}
