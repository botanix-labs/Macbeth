#[cfg(test)]
pub mod test_utils {
    use crate::serde::ser::Error;
    use std::{
        collections::{BTreeMap, HashMap},
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use bitcoin::{
        absolute::LockTime, block::Header, blockdata::transaction::TxOut, hashes::Hash, psbt::Psbt,
        secp256k1::SECP256K1, Address, Amount, Block, BlockHash, FeeRate, OutPoint, ScriptBuf,
        Sequence, Transaction, TxIn, Txid,
    };
    use bitcoincore_rpc::json::{EstimateMode, EstimateSmartFeeResult};
    use frost_secp256k1_tr as frost;
    use rand::{rngs::OsRng, thread_rng, RngCore};
    use reth_btc_wallet::{address::generate_taproot_change_scriptpubkey, util::VerifyingKeyExt};
    use tempfile::TempDir;
    use tokio::sync::Mutex;
    use url::Url;

    use crate::{
        config::Config, database, pegout_id::PegoutId, pegout_scheduler::PegoutRequest, App,
    };

    #[macro_export]
    macro_rules! frost_id {
        ($index:expr) => {
            frost::Identifier::try_from($index).expect("valid id")
        };
    }

    const NETWORK: bitcoin::Network = bitcoin::Network::Regtest;
    const FEERATE: FeeRate = FeeRate::from_sat_per_kwu(5 * 250);

    #[derive(Clone, Debug)]
    pub struct MockBitcoind;
    impl bitcoincore_rpc::RpcApi for MockBitcoind {
        fn estimate_smart_fee(
            &self,
            _conf_target: u16,
            _estimate_mode: Option<EstimateMode>,
        ) -> Result<EstimateSmartFeeResult, bitcoincore_rpc::Error> {
            let fee_rate = FeeRate::from_sat_per_vb(3).expect("valid fee rate");
            Ok(EstimateSmartFeeResult {
                fee_rate: Some(Amount::from_sat(fee_rate.to_sat_per_kwu() * 4)),
                errors: None,
                blocks: 1,
            })
        }

        fn get_blockchain_info(
            &self,
        ) -> bitcoincore_rpc::Result<bitcoincore_rpc::json::GetBlockchainInfoResult> {
            Ok(bitcoincore_rpc::json::GetBlockchainInfoResult {
                initial_block_download: false,
                // Rest of the fields are unused in application code
                chain: bitcoin::Network::Regtest,
                blocks: 1,
                headers: 1,
                difficulty: 1.0,
                pruned: false,
                warnings: "".to_string(),
                best_block_hash: bitcoin::BlockHash::all_zeros(),
                median_time: 0,
                verification_progress: 1.0,
                chain_work: vec![],
                size_on_disk: 0,
                prune_height: None,
                automatic_pruning: None,
                prune_target_size: None,
                softforks: HashMap::new(),
            })
        }

        fn call<T: for<'a> serde::de::Deserialize<'a>>(
            &self,
            method: &str,
            params: &[serde_json::Value],
        ) -> Result<T, bitcoincore_rpc::Error> {
            println!("call: {:?}, {:?}", method, params);

            let mut raw_args = Vec::new();
            if params.len() > 0 {
                raw_args = params
                    .iter()
                    .map(|a| {
                        let json_string = serde_json::to_string(a)?;
                        serde_json::value::RawValue::from_string(json_string)
                    })
                    .map(|a| a.map_err(|e| bitcoincore_rpc::Error::Json(e)))
                    .collect::<Result<Vec<_>, _>>()?;
            }

            if method == "getblockchaininfo" {
                return Ok(serde_json::from_str("{\"initialblockdownload\": false}").unwrap());
            }
            if method == "getbestblockhash" {
                let block_hash = bitcoin::BlockHash::all_zeros();
                return Ok(serde_json::from_str(&format!("\"{block_hash}\"",)).unwrap());
            }
            if method == "getblockheaderinfo" {
                let block_hash = bitcoin::BlockHash::all_zeros();
                let current_time =
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                return Ok(serde_json::from_str(
                    &format!("{{\"hash\": \"{block_hash}\", \"confirmations\": 1, \"height\": 1, \"version\": 1, \"version_hex\": \"01000000\", \"merkleroot\": \"{block_hash}\", \"time\": {current_time}, \"mediantime\": {current_time}, \"nonce\": 1, \"bits\": \"1d00ffff\", \"difficulty\": 1, \"chainwork\": \"0000000000000000000000000000000000000000000000000000000000000001\", \"n_tx\": 1, \"previousblockhash\": \"{block_hash}\", \"nextblockhash\": \"{block_hash}\"}}",),
                ).unwrap());
            }
            if method == "getblockheader" {
                let block_hash = bitcoin::BlockHash::all_zeros();
                let current_time =
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                return Ok(serde_json::from_str(
                    &format!("{{\"hash\": \"{block_hash}\", \"confirmations\": 1, \"height\": 1, \"version\": 1, \"version_hex\": \"01000000\", \"merkleroot\": \"{block_hash}\", \"time\": {current_time}, \"mediantime\": {current_time}, \"nonce\": 1, \"bits\": \"1d00ffff\", \"difficulty\": 1, \"chainwork\": \"0000000000000000000000000000000000000000000000000000000000000001\", \"nTx\": 1, \"previousblockhash\": \"{block_hash}\", \"nextblockhash\": \"{block_hash}\"}}")
                ).unwrap());
            }
            if method == "getmempoolentry" {
                // error case is triggered by a specific txid
                let error_txid = String::from(
                    "c5473905cc714c8b63f229246478e4e85faf32b96babffe4bba2ba8ddc05be3e",
                );
                if raw_args.len() > 0 &&
                    raw_args[0].get().to_string().trim_matches('\"') == error_txid
                {
                    return Err(bitcoincore_rpc::Error::Json(serde_json::error::Error::custom(
                        "tx not in mempool",
                    )));
                }

                let txid = Txid::from_byte_array([0u8; 32]);
                return Ok(serde_json::from_str(&format!("{{\"size\": 250, \"weight\": 1000, \"time\": 1680000000, \"height\": 680000, \"descendantcount\": 2, \"descendantsize\": 500, \"ancestorcount\": 1, \"ancestorsize\": 250, \"wtxid\": \"{txid}\", \"fees\": {{\"base\": 1000, \"modified\": 1100, \"ancestor\": 1200, \"descendant\": 1300}}, \"depends\": [\"{txid}\"], \"spentby\": [\"{txid}\"], \"bip125-replaceable\": true, \"unbroadcast\": false}}",),
                ).unwrap());
            }

            unimplemented!()
        }
    }

    impl MockBitcoind {
        pub fn new() -> Self {
            Self {}
        }
    }

    /* Some Test utils. Should probably be in a separate file */

    pub fn create_random_pegout_id() -> PegoutId {
        let mut rng = thread_rng();
        let mut pegout_id = [0u8; 36];
        rng.fill_bytes(&mut pegout_id);
        PegoutId::from_bytes(&pegout_id).unwrap()
    }

    pub fn pegout_requests_from_tx(tx: &Transaction, pegout_idxs: &[usize]) -> Vec<PegoutRequest> {
        let mut pegout_requests = Vec::new();
        for idx in pegout_idxs {
            pegout_requests.push(PegoutRequest {
                spk: tx.output[*idx].script_pubkey.clone(),
                value: tx.output[*idx].value,
                id: create_random_pegout_id(),
                botanix_height: 0,
            });
        }
        pegout_requests
    }

    pub fn setup_db() -> (database::Db, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db = database::Db::open(temp_dir.path()).unwrap();
        (db, temp_dir)
    }

    pub fn random_txid() -> Txid {
        let mut rng = thread_rng();
        let mut txid = [0u8; 32];
        rng.fill_bytes(&mut txid);
        Txid::from_slice(&txid).unwrap()
    }

    pub fn eth_vector_to_fixed_bytes(eth: Vec<u8>) -> [u8; 20] {
        let mut eth_addr = [0u8; 20];
        eth_addr.copy_from_slice(&eth);
        eth_addr
    }

    pub fn random_p2tr_keyspend_script() -> ScriptBuf {
        let key_pair = bitcoin::secp256k1::generate_keypair(&mut OsRng);
        let change_script = generate_taproot_change_scriptpubkey(&key_pair.1);
        change_script
    }

    pub fn random_p2wpkh_script() -> ScriptBuf {
        let sk = bitcoin::PrivateKey::generate(NETWORK);
        let pk = sk.public_key(SECP256K1);
        let spk = Address::p2wpkh(&pk, NETWORK).unwrap().script_pubkey();

        spk
    }

    pub fn deterministic_p2wpkh_script() -> ScriptBuf {
        let sk =
            bitcoin::PrivateKey::from_wif("cV4G8b983VToX9qL5u82qKVNkVEMs3F3gSdf3s1ormG5S5vi2Gi6")
                .unwrap();
        let pk = sk.public_key(SECP256K1);
        let spk = Address::p2wpkh(&pk, NETWORK).unwrap().script_pubkey();

        spk
    }
    pub fn setup() -> App<MockBitcoind> {
        let mock_bitcoind = MockBitcoind::new();

        let (db, temp_dir) = setup_db();
        let txindex = Mutex::new(
            App::<MockBitcoind>::load_pegout_scheduler(&db, BlockHash::all_zeros(), 6).unwrap(),
        );
        let app = App {
            db,
            pegout_scheduler: txindex,
            btc_network: NETWORK,
            tx_lock: Arc::new(Mutex::new(())),
            identifier: frost_id!(1u16),
            max_signers: 3,
            min_signers: 2,
            frost_round1_dkg: Arc::new(Mutex::new(None)),
            frost_round2_dkg: Arc::new(Mutex::new(None)),
            frost_round1_nonces: Arc::new(Mutex::new(None)),
            btc_signing_server_jwt_secret: None,
            bitcoind_client: mock_bitcoind,
            fall_back_fee_rate: bitcoin::FeeRate::from_sat_per_vb(30).expect("valid fee rate"),
            telemetry: None,
            // This config doesn't matter since we are setting app up manually
            // Normally this would be read from a config file
            config: Config {
                db: temp_dir.path().join("db.db"),
                btc_network: bitcoin::Network::Regtest,
                identifier: 1,
                address: "localhost".to_string(),
                max_signers: 3,
                min_signers: 3,
                toml: None,
                btc_signing_server_jwt_secret: None,
                bitcoind_url: Url::parse("http://localhost:18443")
                    .expect("Bitcoind url to be valid"),
                bitcoind_user: "foo".to_string(),
                bitcoind_pass: "bar".to_string(),
                fee_rate_diff_percentage: 100,
                fall_back_fee_rate_sat_per_vbyte: 30,
                metrics_port: 7000,
            },
        };

        app
    }

    pub fn trusted_dealer_setup(
        min_signers: u16,
        max_signers: u16,
    ) -> (BTreeMap<frost::Identifier, frost::keys::SecretShare>, frost::keys::PublicKeyPackage)
    {
        let rng: rand::prelude::ThreadRng = thread_rng();
        frost::keys::generate_with_dealer(
            max_signers,
            min_signers,
            frost::keys::IdentifierList::Default,
            rng,
        )
        .expect("valid key package")
    }

    // Util function to create a btc tx with random inputs and outputs as defined by fn params
    pub fn create_tx(
        num_inputs: usize,
        num_outputs: usize,
        change: Option<TxOut>,
        deterministic: bool, /* sets specific txid and p2wpkh_script so tx.txid is
                              * deterministic which is useful in tests */
    ) -> Transaction {
        let txid = match deterministic {
            true => Txid::from_byte_array([13u8; 32]),
            false => random_txid(),
        };

        let mut inputs = vec![];
        for i in 0..num_inputs {
            let op = OutPoint::new(txid, i as u32);
            inputs.push(TxIn {
                previous_output: op,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            });
        }

        let mut outputs = vec![];
        for _ in 0..num_outputs {
            let script_pubkey = match deterministic {
                true => deterministic_p2wpkh_script(),
                false => random_p2wpkh_script(),
            };
            outputs.push(TxOut {
                value: Amount::from_sat(1000),
                script_pubkey: script_pubkey.clone(),
            });
        }

        if let Some(change) = change {
            outputs.push(change);
        }

        Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: LockTime::ZERO,
            input: inputs,
            output: outputs,
        }
    }

    pub fn create_block(txs: Vec<Transaction>, prev_hash: bitcoin::BlockHash) -> Block {
        let coin_base_input = TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0u8; 32]), 0xFFFFFFFF),
            script_sig: bitcoin::Script::builder()
                .push_opcode(bitcoin::opcodes::all::OP_PUSHBYTES_3)
                // This hardcodes the height of the block. Could change in the future
                .push_slice(&[10u8; 3])
                .into_script(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
        };
        let coinbase_tx = Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: LockTime::ZERO,
            input: vec![coin_base_input],
            output: vec![],
        };
        let mut txdata = vec![coinbase_tx];
        txdata.extend(txs);
        let block = Block {
            header: Header {
                version: bitcoin::blockdata::block::Version::TWO,
                prev_blockhash: prev_hash,
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 100,
                bits: bitcoin::CompactTarget::from_consensus(0),
                nonce: 0,
            },
            txdata,
        };

        block
    }

    pub fn create_psbt(num_inputs: usize, num_outputs: usize, change: Option<TxOut>) -> Psbt {
        let tx = create_tx(num_inputs, num_outputs, change, false);

        let weight = tx.weight();
        let fee = FEERATE * weight;
        let input_needed = fee.to_sat() + tx.output.iter().map(|o| o.value.to_sat()).sum::<u64>();
        let value_per_input = input_needed / num_inputs as u64 + 1;

        let mut psbt = Psbt::from_unsigned_tx(tx).expect("valid psbt");
        for i in 0..num_inputs {
            psbt.inputs[i].witness_utxo = Some(TxOut {
                value: Amount::from_sat(value_per_input),
                script_pubkey: ScriptBuf::new(),
            });
        }
        psbt
    }

    pub fn get_change(db: &database::Db) -> TxOut {
        let secp_pk = db
            .get_public_key_package()
            .expect("valid key package")
            .expect("key package exists")
            .verifying_key()
            .to_secp_pk()
            .expect("valid secp pk");
        let change_script =
            reth_btc_wallet::address::generate_taproot_change_scriptpubkey(&secp_pk);
        return TxOut { value: Amount::from_sat(500), script_pubkey: change_script };
    }

    pub fn store_pending_pegout(db: &database::Db) -> PegoutId {
        let pegout_id = create_random_pegout_id();
        let pegout_request = PegoutRequest {
            id: pegout_id,
            value: Amount::from_sat(1000),
            spk: random_p2wpkh_script(),
            botanix_height: 0,
        };
        let _ = db.store_pending_pegout(&pegout_request);

        pegout_id
    }
}
