//! poa-testnet consensus integration test
use clap::Parser;
use ethers::{
    core::k256::ecdsa::SigningKey,
    prelude::*,
    providers::{Http, Provider},
    signers::LocalWallet,
    utils::{self},
};
use reth::{
    cli::{
        components::RethNodeComponents,
        ext::{NoArgs, NoArgsCliExt, RethNodeCommandConfig},
    },
    network::Peers,
    poa::PoaNodeCommand,
    runner::CliRunner,
    tasks::TaskSpawner,
};
use reth_primitives::{public_key_to_address, Address, ChainSpec, BOTANIX_TESTNET};
use reth_provider::{CanonStateNotification, CanonStateSubscriptions};
use reth_rpc_types::PeerId;
use secp256k1::SecretKey;
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{self, AsyncBufReadExt},
    process::Command,
};

const RPC_PORT_BASE: u16 = 8545;
const AUTHRPC_PORT_BASE: u16 = 8551;
const DISCOVERY_PORT_BASE: u16 = 30303;
const FED_MEMBER1_SECRET_KEY: &'static str =
    "0a35afe1386497890e1dce7286a5b378b978ede20db900e6ce5b4eb1a0449ad6";
const FED_MEMBER2_SECRET_KEY: &'static str =
    "0cc8f5cc52b62b570dc69001f1ab49cd1a7056bf6312fe057f094135f2c9b019";
const SENDER_SECRET_KEY: &'static str =
    "52947524bbc14bd90cc86c32b9b7564da2f7f8de343825fed68cd04da4925d29";
const RECEIVER_ADDRESS: &'static str = "0x613580C865985dA78613Ea7EBCF7a3b8C5445F93";
const SEND_AMOUNT: u64 = 1; // = 1 Botanix
const INTEGRATION_TEST_ROUNDS: u8 = 3;
const SELECTED_FED_MEMBER_INDEX: usize = 0;

#[derive(Debug)]
pub struct TestPayloadSender {
    pub client: SignerMiddleware<Provider<Http>, Wallet<SigningKey>>,
    pub sender_address: Address,
}

impl TestPayloadSender {
    pub async fn new(rpc_port: u16, sender_secret_key: &str) -> Self {
        // Connect to the network
        let provider =
            Provider::<Http>::try_from(&format!("http://127.0.0.1:{}", rpc_port)).unwrap();
        println!("Node URL: {}", &format!("http://127.0.0.1:{}", rpc_port));

        // get chain id
        let chain_id = provider.get_chainid().await.unwrap();
        assert!(U256::from(BOTANIX_TESTNET.chain().id()) == chain_id, "expected same chain id");

        // get the sender address
        let secp_sender_secret_key = SecretKey::from_str(sender_secret_key).unwrap();
        let secp_sender_pub_key = secp256k1::PublicKey::from_secret_key(
            &secp256k1::Secp256k1::new(),
            &secp_sender_secret_key,
        );
        let sender_address = public_key_to_address(secp_sender_pub_key);

        // create a local wallet
        let wallet: LocalWallet =
            sender_secret_key.parse::<LocalWallet>().unwrap().with_chain_id(chain_id.as_u64());

        // connect the wallet to the provider
        let client = SignerMiddleware::new(provider, wallet);

        Self { client, sender_address }
    }

    pub async fn send(
        &self,
        receiver_address: &str,
        amount_botanix: u64,
    ) -> Result<TxHash, &'static str> {
        // get current receiver balance
        let receiver_account = NameOrAddress::from_str(receiver_address).unwrap();
        let receiver_cur_balance = self.client.get_balance(receiver_account, None).await.unwrap();
        println!("Receiver current balance: {:?}", receiver_cur_balance.to_string());

        // get current sender balance
        let sender_account = NameOrAddress::from_str(&self.sender_address.to_string()).unwrap();
        let sender_cur_balance = self.client.get_balance(sender_account, None).await.unwrap();
        println!("Sender current balance: {:?}", sender_cur_balance.to_string());

        // this also knows to estimate the `max_priority_fee_per_gas` but added it manually too
        let tx = Eip1559TransactionRequest::new()
            .to(receiver_address)
            .value(U256::from(utils::parse_ether(amount_botanix).unwrap()))
            .max_priority_fee_per_gas(U256::from(2000000000_u128)); // 2 Gwei

        // send the tx with the initialized signer client
        let pending_tx = self.client.send_transaction(tx, None).await.unwrap();
        Ok(pending_tx.tx_hash())
    }
}

pub fn is_inturn(authorities_len: u64, signer_index: u64) -> bool {
    // use minutes as time unit to determine in turn
    let timestamp = unix_timestamp() / 60;

    (timestamp / authorities_len) % authorities_len == signer_index
}

pub fn unix_timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Clone, Debug)]
struct ChannelPayload {
    pub engine_index: u16,
    pub ts: tokio::time::Instant,
    pub notification: CanonStateNotification,
}

#[derive(Clone, Debug)]
struct FederationMemberTestConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: SecretKey,
    pub rpc_port: u16,
    pub authrpc_port: u16,
    pub discovery_port: u16,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::mpsc::Sender<ChannelPayload>,
}

impl FederationMemberTestConfig {
    pub fn new(
        index: u16,
        secret_key: SecretKey,
        sender: tokio::sync::mpsc::Sender<ChannelPayload>,
    ) -> Self {
        Self {
            index,
            temp_path: tempfile::TempDir::new().expect("tempdir is okay").into_path(),
            secret_key,
            rpc_port: RPC_PORT_BASE + index,
            authrpc_port: AUTHRPC_PORT_BASE + index,
            discovery_port: DISCOVERY_PORT_BASE + index,
            peers_list: vec![],
            sender,
        }
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn build_command(&self) -> PoaNodeCommand<NoArgsCliExt<FederationMemberTestConfig>> {
        println!("Engine {} data directory", self.index);
        println!(
            "Engine {} secret key = {}",
            self.index,
            &self.secret_key.display_secret().to_string()
        );

        let datadir = self.temp_path.to_str().expect("temp path is okay");
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .unwrap();
        file.write_all(&self.secret_key.display_secret().to_string().as_bytes()).unwrap();

        let no_args = NoArgs::with(self.clone());
        let mut command = PoaNodeCommand::<NoArgsCliExt<FederationMemberTestConfig>>::parse_from([
            "poa",
            "--chain",
            "botanix_testnet",
            "--datadir",
            datadir,
            "--debug.terminate",
            "--http",
            "--http.corsdomain",
            "*",
            "--http.port",
            format!("{}", self.rpc_port).as_str(),
            "--http.addr",
            "127.0.0.1",
            "--http.api",
            "eth,net,trace,txpool,web3,rpc,admin",
            "--authrpc.addr",
            "127.0.0.1",
            "--authrpc.port",
            format!("{}", self.authrpc_port).as_str(),
            "--btc-server",
            "localhost:8080",
            "--btc-block-source",
            "https://mempool.space/signet/api",
            "--port",
            format!("{}", self.discovery_port).as_str(),
            "--p2p-secret-key",
            discovery_secret_path.to_str().unwrap(),
        ])
        .with_ext::<NoArgsCliExt<FederationMemberTestConfig>>(no_args);

        // use custom chain spec
        command.chain = testnet_custom_chain();

        command
    }
}

impl RethNodeCommandConfig for FederationMemberTestConfig {
    fn on_node_started<Reth: RethNodeComponents>(&mut self, components: &Reth) -> eyre::Result<()> {
        println!("Engine {} started task", self.index);

        // add the peers
        for peer in self.peers_list.iter() {
            let peer_socket =
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), peer.discovery_port);
            components.network().add_peer(PeerId::random(), peer_socket);
        }
        println!("Engine {} added peers", self.index);

        let _pool = components.pool();
        let mut canon_events = components.events().subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        components.task_executor().spawn(Box::pin(async move {
            // TODO: test the block creation per index
            while let Some(canon_event) = canon_events.recv().await.ok() {
                println!("Engine {} canonical tip", engine_index);
                let _ = rx_sender
                    .send(ChannelPayload {
                        engine_index,
                        ts: tokio::time::Instant::now(),
                        notification: canon_event,
                    })
                    .await;
            }
        }));

        Ok(())
    }
}

/// This test is disabled for the `optimism` feature flag due to an incompatible feature set.
/// L1 info transactions are not included automatically, which are required for `op-reth` to
/// process transactions.
#[tokio::test]
#[cfg_attr(feature = "optimism", ignore)]
pub async fn test_poa_testnet() {
    // generate test fed members poa nodes
    let (test_fed_members, mut rx) = create_poa_federation_members();

    // assign targeted fed memeber
    let targeted_fed_member = test_fed_members.get(&SELECTED_FED_MEMBER_INDEX).cloned().unwrap();

    // get total authorities number
    let total_authorities = test_fed_members.len();

    // start btc server
    tokio::spawn(async move {
        run_btc_server().await;
    });
    // wait for the btc server to boot up
    tokio::time::sleep(Duration::from_secs(10)).await;

    // run all poa nodes in the background
    for (_index, fed_member_config) in test_fed_members.into_iter() {
        let _ = std::thread::spawn(move || {
            let fed_member_command = fed_member_config.build_command();
            let runner = CliRunner::default();
            runner.run_command_until_exit(|ctx| fed_member_command.execute(ctx)).unwrap();
        });
        // wait for one second inbetween members start
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // create payload client
    let payload_client =
        TestPayloadSender::new(targeted_fed_member.rpc_port, SENDER_SECRET_KEY).await;

    // create a hashmap to store tx hashes
    let mut tx_hashes_set = HashSet::new();

    // send eoa messages to the node at selected index
    println!("======>  Sending eoa transaction...");
    let mut last_tx_hash = payload_client.send(RECEIVER_ADDRESS, SEND_AMOUNT).await.unwrap();
    tx_hashes_set.insert(last_tx_hash.to_fixed_bytes());

    // wait for canonical chain updates reported by the node, then send new tx
    let mut test_rounds = 0;
    while let Some(x) = rx.recv().await {
        println!("======> Received payload from engine index {:?}", x.engine_index);
        assert_eq!(x.engine_index, SELECTED_FED_MEMBER_INDEX as u16);
        if test_rounds == INTEGRATION_TEST_ROUNDS {
            break
        }

        // block verfication
        let block_receipts = x.notification.block_receipts();
        println!("Block receipts? {:?}", block_receipts);
        assert_eq!(block_receipts.len(), 1);
        let block_payload = block_receipts.first().cloned().unwrap();
        assert_eq!(block_payload.1, false);
        assert_eq!(block_payload.0.tx_receipts.len(), 1);
        assert!(block_payload.0.block.number > 0);

        // wait until current turn changes
        let current_turn = is_inturn(total_authorities as u64, targeted_fed_member.index.into());
        'inner: loop {
            let is_test_fed_member_inturn =
                is_inturn(total_authorities as u64, targeted_fed_member.index.into());
            println!("Is in turn? {}", is_test_fed_member_inturn);
            if is_test_fed_member_inturn != current_turn {
                break 'inner
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue
        }
        println!("======>  Sending eoa transaction...");
        last_tx_hash = payload_client.send(RECEIVER_ADDRESS, SEND_AMOUNT).await.unwrap();
        tx_hashes_set.insert(last_tx_hash.to_fixed_bytes());
        test_rounds += 1;
    }
}

fn testnet_custom_chain() -> Arc<ChainSpec> {
    BOTANIX_TESTNET.clone()
}

fn create_poa_federation_members(
) -> (HashMap<usize, FederationMemberTestConfig>, tokio::sync::mpsc::Receiver<ChannelPayload>) {
    // create two secret keys one for each member
    let sc1 = FED_MEMBER1_SECRET_KEY.parse::<SecretKey>().unwrap();
    let sc2 = FED_MEMBER2_SECRET_KEY.parse::<SecretKey>().unwrap();

    // create the member configs
    let (tx, rx) = tokio::sync::mpsc::channel::<ChannelPayload>(10);
    let mut fed_member_config1 = FederationMemberTestConfig::new(0, sc1, tx.clone());
    let mut fed_member_config2 = FederationMemberTestConfig::new(1, sc2, tx);

    // insert peers
    fed_member_config1.insert_peers_list(vec![fed_member_config2.clone()]);
    fed_member_config2.insert_peers_list(vec![fed_member_config1.clone()]);

    // persist all in a hashmap
    let mut fed_members: HashMap<usize, FederationMemberTestConfig> = HashMap::new();
    fed_members.insert(0, fed_member_config1);
    fed_members.insert(1, fed_member_config2);
    (fed_members, rx)
}

async fn run_btc_server() {
    // go to the btc sercer directory
    let mut working_directory = std::env::current_dir().unwrap();
    for _ in 0..3 {
        working_directory.pop();
    }
    working_directory.push("bin");
    working_directory.push("btc-server");

    let command = "cargo";
    let args = vec![
        "run",
        "--bin",
        "btc-server",
        "--",
        "--network",
        "testnet",
        "--pkey",
        "./key.hex",
        "--db",
        "./db",
    ];

    // Create a Command instance and set the working directory
    let mut cmd = Command::new(command);
    cmd.args(&args).current_dir(working_directory).stdout(Stdio::piped());

    // Spawn the command and handle its output
    let mut child = cmd.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();

    let mut lines = io::BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        println!("** BTC SERVER ** >>> {:?}", line);
    }
}
