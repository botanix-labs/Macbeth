#[cfg(test)]
mod integration_tests {
    use std::{collections::HashSet, str::FromStr, sync::Arc};

    use crate::test_utils::{
        create_psbt, create_random_pegout_id, create_tx, eth_vector_to_fixed_bytes, get_change,
        random_p2wpkh_script, store_pending_pegout, trusted_dealer_setup,
    };
    use bitcoin::{
        blockdata::{script::Script, transaction::TxOut},
        hashes::Hash,
        Amount, OutPoint, Psbt, ScriptBuf, Txid,
    };
    use frost_secp256k1_tr as frost;
    use rand::{thread_rng, Rng};
    use tokio::sync::Mutex;
    use tonic::{Code, Request};

    use crate::{
        frost_id,
        wallet::{
            psbt::{PsbtInputExt, PsbtOutputExt},
            util::VerifyingKeyExt,
        },
    };

    use crate::{
        database::Utxo,
        pegout_scheduler::pegout_id::PegoutId,
        rpc::{self, BtcServer},
        util::{has_conflicting_input, validate_outputs},
    };

    // NOTE: reminder for the tests that frost identifiers start indexing at 1
    /**
     * Round 2 DKG Tests
     */

    #[tokio::test]
    async fn should_fail_dkg_when_round1_pkg_is_stale() {
        // DKG should fail all together if you are using a round1 secret that does not belong to
        // this DKG round
        let rng: rand::prelude::ThreadRng = thread_rng();
        // We essentially need to emulate dkg here
        let mut app1 = setup();
        let mut app2 = setup();
        let mut app3 = setup();
        let mut round1_dkgs = vec![];
        // Reminder that frost ids index at 1
        for index in 1..(app1.max_signers + 1) {
            round1_dkgs.push(
                frost::keys::dkg::part1(
                    frost::Identifier::try_from(index).expect("valid id"),
                    app1.max_signers,
                    app1.min_signers,
                    rng.clone(),
                )
                .unwrap(),
            );
        }
        // 1st participant round 1
        app1.identifier = frost_id!(1);
        app1.frost_round1_dkg = Arc::new(Mutex::new(Some(round1_dkgs[0].clone())));
        app1.add_round1_dkg(frost_id!(2), round1_dkgs[1].clone().1)
            .await
            .expect("valid round1 dkg");
        app1.add_round1_dkg(frost_id!(3), round1_dkgs[2].clone().1)
            .await
            .expect("valid round1 dkg");
        let _p1_dkg2 = app1.get_round2_dkg().await.expect("valid round 2 transition");

        // 2nd participant round 1
        app2.frost_round1_dkg = Arc::new(Mutex::new(Some(round1_dkgs[1].clone())));
        app2.identifier = frost_id!(2);
        app2.add_round1_dkg(frost_id!(1), round1_dkgs[0].clone().1)
            .await
            .expect("valid round1 dkg");
        app2.add_round1_dkg(frost_id!(3), round1_dkgs[2].clone().1)
            .await
            .expect("valid round1 dkg");
        let _p2_dkg2 = app2.get_round2_dkg().await.expect("valid round 2 transition");

        // 3rd participant round 1
        app3.frost_round1_dkg = Arc::new(Mutex::new(Some(round1_dkgs[2].clone())));
        app3.identifier = frost_id!(3);
        app3.add_round1_dkg(frost_id!(1), round1_dkgs[0].clone().1)
            .await
            .expect("valid round1 dkg");
        app3.add_round1_dkg(frost_id!(2), round1_dkgs[1].clone().1)
            .await
            .expect("valid round1 dkg");
        let p3_dkg2 = app3.get_round2_dkg().await.expect("valid round 2 transition");

        // Now lets re-generate the round 1 package for the first participant
        let new_round1_pkg =
            frost::keys::dkg::part1(frost_id!(1), app1.max_signers, app1.min_signers, rng.clone())
                .unwrap();
        app1.frost_round1_dkg = Arc::new(Mutex::new(Some(new_round1_pkg)));

        // Round 2 dkg for 2'nd participant
        // However adding the shares from participant 1 to others should fail
        let new_p1_dkg2 = app1.get_round2_dkg().await.unwrap();

        let p2_share = new_p1_dkg2.get(&frost_id!(2)).unwrap();
        let _ =
            app2.add_round2_dkg(frost_id!(1), p2_share.clone()).await.expect("valid round2 dkg");
        let p2_share = p3_dkg2.get(&frost_id!(2)).unwrap();
        let res = app2.add_round2_dkg(frost_id!(3), p2_share.clone()).await;
        // This will fail because some round2 shares have been generated with different round1
        // coefficients
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().to_string(), "internal FROST error: Invalid secret share.");
    }

    // Signing tests
    #[tokio::test]
    async fn should_fail_to_get_to_sign_package_without_dkg() {
        let app = setup();
        let signing_session_id = [0u8; 32];
        let mut psbt = create_psbt(1, 1, None);
        let tx = psbt.clone().unsigned_tx;
        // Add the utxo
        let utxo = Utxo::new(tx.input[0].previous_output, tx.output[0].clone(), None);
        app.add_pegins(&[&utxo]).expect("valid pegin utxo");

        let nonce_commits = app.get_round1_signing_package(&mut psbt, &signing_session_id).await;
        assert!(nonce_commits.is_err());
        assert_eq!(nonce_commits.err().unwrap().to_string(), "missing key package");
    }

    // TODO re-enable this test once we have a limit on max number of inputs
    // #[tokio::test]
    // async fn should_fail_when_requesting_too_many_nonces() {
    //     let app = setup();
    //     let signing_session_id = [0u8; 32];
    //     let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
    //     let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
    //         .expect("valid key package");
    //     app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package).expect("set public key package");
    //     app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package).expect("set key package");

    //     let mut psbt = create_psbt(100);
    //     let mock_bitcoind = MockBitcoind::new();

    //     let nonce_commits =
    //         app.get_round1_signing_package(&mut psbt, &signing_session_id, &mock_bitcoind).await;
    //     assert!(nonce_commits.is_err());
    //     assert_eq!(
    //         nonce_commits.err().unwrap().to_string(),
    //         "invalid number of signing nonces requested"
    //     );
    // }

    #[tokio::test]
    async fn should_get_round1_nonce_commitments() {
        let mut app = setup();
        let signing_session_id = [0u8; 32];
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package).expect("set key package");

        let pegout_id_1 = store_pending_pegout(&app.db);

        let mut psbt = create_psbt(1, 1, Some(get_change(&app.db)));
        psbt.outputs[0].set_pegout_id(pegout_id_1.as_bytes());

        let tx = psbt.clone().extract_tx().expect("valid tx");
        // Add the utxo
        let utxo = Utxo::new(
            tx.input[0].previous_output,
            psbt.inputs[0].witness_utxo.clone().expect("some"),
            None,
        );
        app.add_pegins(&[&utxo]).expect("valid pegin utxo");
        app.get_round1_signing_package(&mut psbt, &signing_session_id)
            .await
            .expect("valid nonce commits request");
        let sc1 = psbt.inputs[0].all_signing_commitments();
        assert_eq!(sc1.len(), 1);

        // Should not be able to get a new set of nonces
        let res = app.get_round1_signing_package(&mut psbt, &signing_session_id).await;
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().to_string(), "already in signing session");

        // Ensure you get a different set of nonces on a new signing session
        app.frost_round1_nonces = Arc::new(Mutex::new(None));
        let signing_session_id = [1u8; 32];

        let pegout_id = store_pending_pegout(&app.db);

        let mut psbt = create_psbt(1, 1, Some(get_change(&app.db)));
        // include all pending pegouts in psbt to pass validation
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());
        psbt.outputs[1].set_pegout_id(pegout_id_1.as_bytes());
        let tx = psbt.clone().extract_tx().expect("valid tx");
        let utxo = Utxo::new(
            tx.input[0].previous_output,
            psbt.inputs[0].witness_utxo.clone().expect("some"),
            None,
        );
        app.add_pegins(&[&utxo]).expect("valid pegin utxo");
        app.get_round1_signing_package(&mut psbt, &signing_session_id)
            .await
            .expect("valid nonce commits request");

        let sc2 = psbt.inputs[0].all_signing_commitments();
        assert_eq!(sc2.len(), 1);

        assert_eq!(sc2.len(), 1);
        assert_ne!(sc1, sc2);
    }

    #[tokio::test]
    async fn should_add_signing_package_round1() {
        let app_signer = setup();
        let app_coordinator = setup();
        let signing_session_id = [0u8; 32];
        let (shares, pk_package) =
            trusted_dealer_setup(app_signer.min_signers, app_signer.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app_signer.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app_signer.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app_signer.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        app_coordinator.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package).expect("set public key package");
        app_coordinator.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package).expect("set key package");

        let pegout_id = store_pending_pegout(&app_coordinator.db);

        let mut psbt = create_psbt(1, 1, Some(get_change(&app_coordinator.db)));
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        let tx = psbt.clone().extract_tx().expect("valid tx");
        // Add the utxo
        let utxo = Utxo::new(
            tx.input[0].previous_output,
            psbt.inputs[0].witness_utxo.clone().expect("some"),
            None,
        );
        app_signer.add_pegins(&[&utxo]).expect("valid pegin utxo");
        app_coordinator.add_pegins(&[&utxo]).expect("valid pegin utxo");

        // Should fail if there are no signing commits in the psbt
        let res =
            app_coordinator.add_round1_signing(&signing_session_id, app_signer.identifier, &psbt);
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().to_string(), "Could not find participant information");

        app_signer
            .get_round1_signing_package(&mut psbt, &signing_session_id)
            .await
            .expect("valid nonce commits request");
        psbt.inputs[0].signing_commitments(app_signer.identifier).expect("valid sc1");

        app_coordinator
            .add_round1_signing(&signing_session_id, app_signer.identifier, &psbt)
            .expect("should add signing round 1");
    }

    // TODO (armins) fix these tests!!
    // #[test]
    // fn should_not_sign_without_dkg() {
    //     let app = setup();
    //     let tx =
    //         Transaction { version: 2, lock_time: LockTime::ZERO, input: vec![], output: vec![] };
    //     let psbt = Psbt::from_unsigned_tx(tx).expect("valid tx");
    //     let mut signing_package: Vec<frost::SigningPackage> = vec![];
    //     let res = app.get_round2_signing_package(&psbt);
    //     assert!(res.is_err());
    //     assert_eq!(res.err().unwrap().to_string(), "missing key package");
    // }

    // #[test]
    // fn should_not_sign_if_signer_is_not_in_signing_set() {
    //     let app = setup();
    //     let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
    //     let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
    //         .expect("valid key package");

    //     app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package).expect("set public key package");
    //     app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package).expect("set key package");

    //     let tx = create_tx(1);
    //     let mut signing_package: Vec<frost::SigningPackage> = vec![];
    //     // TODO finish this test
    //     // for input in tx.input.iter() {
    //     //     let signing_package = S
    //     // }
    //     // let tx =
    //     //     Transaction { version: 2, lock_time: LockTime::ZERO, input: vec![], output: vec![]
    // };     // let psbt = Psbt::from_unsigned_tx(tx).expect("valid tx");
    // }
    #[tokio::test]
    async fn test_should_abort_signing() {
        let app = setup();
        let signing_session_id = [0u8; 32];
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        let pegout_id = store_pending_pegout(&app.db);

        let mut psbt = create_psbt(1, 1, Some(get_change(&app.db)));
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        // Add pegin utxo
        let tx = psbt.clone().extract_tx().expect("valid tx");
        // Add the utxo
        let utxo = Utxo::new(
            tx.input[0].previous_output,
            psbt.inputs[0].witness_utxo.clone().expect("some"),
            None,
        );
        app.add_pegins(&[&utxo]).expect("valid pegin utxo");

        app.get_round1_signing_package(&mut psbt, &signing_session_id)
            .await
            .expect("valid nonce commits request");

        let signing_nonces = app.frost_round1_nonces.lock().await.clone().unwrap();
        assert_eq!(signing_nonces.len(), 1);
        app.abort_signing().await.expect("valid abort request");

        let signing_nonces = app.frost_round1_nonces.lock().await.clone();
        assert!(signing_nonces.is_none());
    }

    #[test]
    fn validate_outputs_should_validate() {
        let app = setup();

        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");
        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        // store pending pegout
        let pegout_id = store_pending_pegout(&app.db);

        let mut psbt = create_psbt(1, 1, Some(get_change(&app.db)));
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        let response = validate_outputs(&psbt, &app.db);

        assert!(response.is_ok());
    }

    #[test]
    fn validate_outputs_should_validate_with_change_output() {
        // store agg_pk
        let app = setup();
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        // store pending pegout
        let pegout_id = PegoutId::new([1u8; 32], 0);
        let pegout_request = PegoutRequest {
            id: pegout_id,
            value: Amount::from_sat(1000),
            spk: bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
                .expect("valid address")
                .assume_checked()
                .script_pubkey(),
            botanix_height: 0,
        };
        let _ = app.db.store_pending_pegout(&pegout_request);

        let secp_pk = app
            .db
            .get_public_key_package()
            .expect("valid key package")
            .expect("key package exists")
            .verifying_key()
            .to_secp_pk()
            .expect("valid secp pk");
        let change_script =
            botanix_btc_wallet::address::generate_taproot_change_scriptpubkey(&secp_pk);
        let change = TxOut { value: Amount::from_sat(500), script_pubkey: change_script };

        let mut psbt = create_psbt(1, 1, Some(change));
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        let response = validate_outputs(&psbt, &app.db);

        assert!(response.is_ok());
    }

    #[test]
    fn validate_outputs_should_fail_with_invalid_change_output() {
        // store agg_pk
        let app = setup();
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        // store pending pegout
        let pegout_id = PegoutId::new([1u8; 32], 0);
        let pegout_request = PegoutRequest {
            id: pegout_id,
            value: Amount::from_sat(1000),
            spk: bitcoin::Address::from_str("mrpkDJFJdNGA22FaxCWw6T9oXogXfHU1rh")
                .expect("valid address")
                .assume_checked()
                .script_pubkey(),
            botanix_height: 0,
        };
        let _ = app.db.store_pending_pegout(&pegout_request);

        let change = TxOut { value: Amount::from_sat(500), script_pubkey: ScriptBuf::default() };

        let mut psbt = create_psbt(1, 1, Some(change));
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        let response = validate_outputs(&psbt, &app.db);

        assert_eq!(response.err().expect("error exists").to_string(), "invalid change output",);
    }

    #[tokio::test]
    // this test ensures a conflicting input is included in the psbt when retrying the same
    // pegout(s)
    async fn test_conflicting_input() {
        let app = setup();
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        // now generate some random utxos and save them
        for _ in 0..3 {
            let dummy_tx = create_tx(1, 1, None);
            let utxo =
                Utxo::new(dummy_tx.input[0].previous_output, dummy_tx.output[0].clone(), None);
            app.db.store_utxos(&[&utxo]).expect("Failed to store UTXO");
        }

        // create pegout
        let pegout_id = create_random_pegout_id();
        let spk = random_p2wpkh_script().as_bytes().to_vec();
        let request = Request::new(rpc::NotifyPegoutsRequest {
            pending_pegouts: vec![rpc::PendingPegout {
                pegout_id: pegout_id.as_bytes().to_vec(),
                spk: spk.clone(),
                amount: 1_000, // sats
                height: 1,
            }],
        });

        app.notify_pegouts(request).await.expect("valid pegout request");

        let pending_pegouts = app.db.get_pending_pegouts().expect("valid pending pegouts");
        let tx_out = pending_pegouts[0].txout();

        // create psbt for pending pegout
        let mut psbt = create_psbt(1, 1, None);
        psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

        // set the tx output to the pegout
        let mut tracked_tx = psbt.clone().extract_tx().expect("valid tx");
        tracked_tx.output = vec![tx_out];

        // Add the utxo
        let utxo = Utxo::new(
            tracked_tx.input[0].previous_output,
            psbt.inputs[0].witness_utxo.clone().expect("some"),
            None,
        );
        app.add_pegins(&[&utxo]).expect("valid pegin utxo");

        // track the tx
        app.add_tracked_tx(tracked_tx.clone(), &pending_pegouts, SystemTime::now())
            .await
            .expect("tx to be tracked");

        // get the tracked input
        let tracked_inputs = app.pegout_scheduler.lock().await.tracked_inputs();
        assert_eq!(tracked_inputs.len(), 1);
        let txid =
            tracked_tx.input.iter().map(|i| i.previous_output).collect::<Vec<OutPoint>>()[0].txid;
        let outpoint = OutPoint { txid, vout: 0 };
        let tracked_input = tracked_inputs.get(&outpoint).expect("tracked input exists");

        // make sure there are 2 utxos (tracked and untracked)
        let request = Request::new(rpc::Empty {});
        let response = app.get_all_utxos(request).await;
        let utxos = response.expect("utxos to exist").into_inner().utxos;
        assert_eq!(utxos.len(), 4);

        // request a psbt which should include a conflicting input (the tracked input)
        let request = Request::new(rpc::MakeTxRequest {
            signing_session_id: [0u8; 32].to_vec(),
            checkpoint_block_hash: BlockHash::all_zeros().to_byte_array().to_vec(),
        });
        let response = app.get_psbt(request).await;

        // deserialize the psbt from bytes
        let psbt_bytes = response.expect("valid psbt").into_inner().psbt;
        let psbt = Psbt::deserialize(psbt_bytes.as_slice()).expect("valid psbt");

        // assert that the psbt contains the tracked(conflicting) input
        let result =
            psbt.unsigned_tx.input.iter().find(|input| input.previous_output == *tracked_input);
        assert!(result.is_some());
    }

    #[tokio::test]
    // this test ensures conflicting inputs for all pegouts being retried are included in the psbt
    // pegout A and pegout B are both being retried so the psbt should contain inputs from both
    // previous txs
    async fn test_multiple_conflicting_inputs_for_multiple_pegouts() {
        let mut rng = thread_rng();
        let app = setup();
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        // now generate some random utxos and save them
        for _ in 0..3 {
            let txid = Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap();
            let vout = rng.gen_range(0..u32::MAX);
            let value = rng.gen_range(1..1_000_000);
            let script_bytes: Vec<u8> = (0..20).map(|_| rng.gen()).collect();
            let script = Script::from_bytes(script_bytes.as_slice());

            let utxo = Utxo::new(
                OutPoint::new(txid, vout),
                TxOut { value: Amount::from_sat(value), script_pubkey: script.into() },
                None,
            );
            app.db.store_utxos(&[&utxo]).expect("Failed to store UTXO");
        }

        // create 2 pegouts
        let mut tracked_inputs = HashSet::new();
        let mut pegouts: Vec<PegoutRequest> = vec![];
        for _ in 0..2 {
            // create pegout
            let pegout_id = create_random_pegout_id();
            let spk = random_p2wpkh_script().as_bytes().to_vec();
            let request = Request::new(rpc::NotifyPegoutsRequest {
                pending_pegouts: vec![rpc::PendingPegout {
                    pegout_id: pegout_id.as_bytes().to_vec(),
                    spk: spk.clone(),
                    amount: 1_000, // sats
                    height: 1,
                }],
            });

            app.notify_pegouts(request).await.expect("valid pegout request");

            let pending_pegouts = app.db.get_pending_pegouts().expect("valid pending pegouts");
            // store pegout id so we can add back to the db
            pegouts.push(pending_pegouts[0].clone());
            let tx_out = pending_pegouts[0].txout();
            // clear pending pegouts since we are simulating broadcasting a psbt with that pegout
            // and having a tracked tx for it
            app.db.clear_pending_pegouts().expect("pending pegouts are cleared");

            // create psbt for pending pegout
            let mut psbt = create_psbt(1, 1, None);
            psbt.outputs[0].set_pegout_id(pegout_id.as_bytes());

            // set the tx output to the pegout
            let mut tracked_tx = psbt.clone().extract_tx().expect("valid tx");
            tracked_tx.output = vec![tx_out];

            // Add the utxo
            let utxo = Utxo::new(
                tracked_tx.input[0].previous_output,
                psbt.inputs[0].witness_utxo.clone().expect("some"),
                None,
            );
            app.add_pegins(&[&utxo]).expect("valid pegin utxo");

            // track the tx
            app.add_tracked_tx(tracked_tx.clone(), &pending_pegouts, SystemTime::now())
                .await
                .expect("tx to be tracked");

            // get the tracked input
            let txid =
                tracked_tx.input.iter().map(|i| i.previous_output).collect::<Vec<OutPoint>>()[0]
                    .txid;
            let outpoint = OutPoint { txid, vout: 0 };
            let inputs = app.pegout_scheduler.lock().await.tracked_inputs();
            tracked_inputs.insert(inputs.get(&outpoint).expect("tracked input exists").clone());
        }
        assert_eq!(tracked_inputs.len(), 2);

        // add the pegout requests back to the db
        app.db
            .store_pending_pegouts_atomically(pegouts.iter().collect::<Vec<_>>().as_ref())
            .expect("valid pegout requests");

        // add additional utxos that are not part of a tracked tx
        for _ in 0..5 {
            let psbt = create_psbt(1, 1, Some(get_change(&app.db)));
            let tx = psbt.clone().extract_tx().expect("valid tx");
            // Add the utxo
            let utxo = Utxo::new(
                tx.input[0].previous_output,
                psbt.inputs[0].witness_utxo.clone().expect("some"),
                None,
            );
            app.add_pegins(&[&utxo]).expect("valid pegin utxo");
        }

        // make sure there are 7 utxos (2 tracked and 5 untracked)
        let request = Request::new(rpc::Empty {});
        let response = app.get_all_utxos(request).await;
        let utxos = response.expect("utxos to exist").into_inner().utxos;
        assert_eq!(utxos.len(), 10);

        // request a psbt which should include two conflicting inputs
        let request = Request::new(rpc::MakeTxRequest {
            signing_session_id: [0u8; 32].to_vec(),
            checkpoint_block_hash: BlockHash::all_zeros().to_byte_array().to_vec(),
        });
        let response = app.get_psbt(request).await;

        // deserialize the psbt from bytes
        let psbt_bytes = response.expect("valid psbt").into_inner().psbt;
        let psbt = Psbt::deserialize(psbt_bytes.as_slice()).expect("valid psbt");

        // assert that the psbt contains the tracked(conflicting) inputs
        let psbt_inputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|input| input.previous_output)
            .collect::<HashSet<_>>();

        // psbt should contain all tracked inputs
        assert!(psbt_inputs.is_superset(&tracked_inputs));
    }

    #[tokio::test]
    // tests db can determine if a conflicting input is present in a psbt
    async fn test_has_conflicting_input() {
        let app = setup();
        let (shares, pk_package) = trusted_dealer_setup(app.min_signers, app.max_signers);
        let key_package = frost::keys::KeyPackage::try_from(shares[&app.identifier].clone())
            .expect("valid key package");

        // Add the key packages
        app.db.set_pubkey_package_by_id(LEGACY_MULTISIG_ID, pk_package.clone()).expect("set public key package");
        app.db.set_key_package_by_id(LEGACY_MULTISIG_ID, key_package.clone()).expect("set key package");

        // now generate some random utxos and save them
        for _ in 0..3 {
            let dummy_tx = create_tx(1, 1, None);
            let utxo =
                Utxo::new(dummy_tx.input[0].previous_output, dummy_tx.output[0].clone(), None);
            app.db.store_utxos(&[&utxo]).expect("Failed to store UTXO");
        }

        // create pegout
        let pegout_id = create_random_pegout_id();
        let spk = random_p2wpkh_script().as_bytes().to_vec();
        let request = Request::new(rpc::NotifyPegoutsRequest {
            pending_pegouts: vec![rpc::PendingPegout {
                pegout_id: pegout_id.as_bytes().to_vec(),
                spk: spk.clone(),
                amount: 1_000, // sats
                height: 1,
            }],
        });
        app.notify_pegouts(request).await.expect("valid pegout request");

        let pending_pegouts = app.db.get_pending_pegouts().expect("valid pending pegouts");
        let tx_out = pending_pegouts[0].txout();

        // create psbt for pending pegout
        // extract the tx and track so psbt has conflicting input
        let mut psbt_with_conflicting_input = create_psbt(1, 1, None);
        psbt_with_conflicting_input.outputs[0].set_pegout_id(pegout_id.as_bytes());

        // set the tx output to the pegout
        let mut tracked_tx = psbt_with_conflicting_input.clone().extract_tx().expect("valid tx");
        tracked_tx.output = vec![tx_out];

        // Add the utxo
        let utxo = Utxo::new(
            tracked_tx.input[0].previous_output,
            psbt_with_conflicting_input.inputs[0].witness_utxo.clone().expect("some"),
            None,
        );
        app.add_pegins(&[&utxo]).expect("valid pegin utxo");

        // track the tx
        app.add_tracked_tx(tracked_tx.clone(), &pending_pegouts, SystemTime::now())
            .await
            .expect("tx to be tracked");

        // create psbt with no conflicting input
        // the pegout is honoring the pegout request but its input is not in the tracked tx
        let mut psbt_no_conflicting_input = create_psbt(1, 1, None);
        psbt_no_conflicting_input.outputs[0].set_pegout_id(pegout_id.as_bytes());

        // validate the psbt with no conflicting input
        let result = has_conflicting_input(&app.db, &psbt_no_conflicting_input);
        assert_eq!(result.unwrap_err().to_string(), "no conflicting input");

        // validate the psbt with conflicting input
        let result = has_conflicting_input(&app.db, &psbt_with_conflicting_input);
        assert!(result.is_ok());
    }
}
