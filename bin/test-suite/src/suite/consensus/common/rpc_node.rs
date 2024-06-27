use crate::{
    it_info_print,
    suite::consensus::{
        common::poa_node::{
            CannonStateNofificationPayload, FederationMemberTestConfig, Notifications,
        },
        GlobalContext,
    },
};
use askama::Template;
use bitcoin::{
    hashes::{sha256, Hash},
    BlockHash,
};
use clap::Parser;
use reth::{
    args::{FedMemberPubKey, GenesisTomlConfig},
    cli::ext::{NoArgs, PoaNodeCommandConfig, RethNodeComponents},
    commands::poa::PoaNodeCommand,
    consensus_common::utils::unix_timestamp,
    network::Peers,
};
use reth_network_types::pk2id;
use reth_primitives::{
    chain::spec::BotanixTestnetGenesisConfig,
    create_botanix_config_with_genesis,
    extra_data_header::{ExtraDataHeader, EXTRA_HEADER_VERSION},
    hex::encode as hex_encode,
};
use reth_provider::CanonStateSubscriptions;
use reth_rpc_types::PeerId;
use secp256k1::{PublicKey, SECP256K1};
use std::{
    collections::HashMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};
use url::Url;

const RPC_PORT_BASE: u16 = 8545;
const DISCOVERY_PORT_BASE: u16 = 30321;

#[derive(Clone, Debug)]
pub struct NonFederationMemberTestConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: String,
    pub rpc_port: u16,
    pub discovery_port: u16,
    pub bitcoind_url: Url,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::broadcast::Sender<Notifications>,
    pub peer_id: PeerId,
}

impl NonFederationMemberTestConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        index: u16,
        secret_key: String,
        sender: tokio::sync::broadcast::Sender<Notifications>,
        bitcoind_url: Url,
        bitcoind_username: String,
        bitcoind_password: String,
        peer_id: PeerId,
    ) -> Self {
        let rpc_port = RPC_PORT_BASE + index;
        let discovery_port = DISCOVERY_PORT_BASE + index;
        Self {
            index,
            temp_path: {
                let ret = tempfile::TempDir::new()
                    .expect("tempdir is okay")
                    .into_path()
                    .join(format!("_{}", unix_timestamp().to_string()));
                std::fs::create_dir_all(&ret).expect("can't create tmpdir subdir");
                ret
            },
            secret_key,
            rpc_port,
            discovery_port,
            bitcoind_url,
            bitcoind_username,
            bitcoind_password,
            peers_list: vec![],
            sender,
            peer_id,
        }
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn build_command(
        &mut self,
        edh_authorities_list: Arc<Vec<PublicKey>>,
        fed_member_peers_list: Vec<FederationMemberTestConfig>,
    ) -> PoaNodeCommand<NoArgs<NonFederationMemberTestConfig>> {
        it_info_print!(format!("RPC Engine {} secret key = {}", self.index, &self.secret_key));
        self.insert_peers_list(fed_member_peers_list.clone());

        let datadir = self.temp_path.to_str().expect("temp path is okay");
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .expect("file can be opened");
        file.write_all(&self.secret_key.as_bytes()).expect("secret key written to file");

        // now create the edh
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            None,
            Some(edh_authorities_list.to_vec()),
            None,
            None,
            // to make sure they're not identical, hash random data
            BlockHash::hash(&[1]),
            sha256::Hash::hash(&[2]),
        );

        // update genesis config with edh and render file
        let botanix_testnet_config_genesis = {
            let edh = hex::encode(edh.serialize());
            let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = botanix_testnet_config_genesis.render().unwrap();
            rendered_json
        };

        // Need to create a chain.toml in the data dir
        // Need to zip together the soc address and pk
        let mut fed_member_pks = vec![];
        for peer in fed_member_peers_list.iter() {
            let pk = FedMemberPubKey {
                key: peer.secret_key.public_key(SECP256K1).to_string(),
                socket_addr: format!("127.0.0.1:{}", peer.discovery_port),
            };
            fed_member_pks.push(pk);
        }

        // NOTE: fed members have already created their EDH with the correct authorities
        // but the order may not be the same as fed_member_pks since we added ourselves last
        // so compare the EDH authorities list and build a new list in the correct order
        let mut edh_authorities = vec![];
        for authority in edh_authorities_list.to_vec().iter() {
            for pk in fed_member_pks.iter() {
                if pk.key == authority.to_string() {
                    edh_authorities.push(pk.clone());
                    break;
                }
            }
        }

        let chain_config =
            GenesisTomlConfig::new("integration test toml".to_string(), edh_authorities, None);
        it_info_print!("Chain config", chain_config);
        chain_config.write_to_path(Path::new(datadir).join("chain.toml")).unwrap();

        let no_args = NoArgs::with(self.clone());
        let mut command = PoaNodeCommand::<NoArgs<FederationMemberTestConfig>>::parse_from([
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
            "--btc-network",
            "regtest",
            "--bitcoind.url",
            self.bitcoind_url.as_str(),
            "--bitcoind.username",
            self.bitcoind_username.as_str(),
            "--bitcoind.password",
            self.bitcoind_password.as_str(),
            "--port",
            format!("{}", self.discovery_port).as_str(),
            "--p2p-secret-key",
            discovery_secret_path.to_str().expect("discovery secret path to exist"),
        ])
        .with_ext::<NoArgs<NonFederationMemberTestConfig>>(no_args);
        // use botanix chain spec
        let genesis = serde_json::from_str(&botanix_testnet_config_genesis)
            .expect("Can't deserialize Botanix Testnet genesis json");
        // use 6 as the pegin confirmation depth
        let botanix_testnet = create_botanix_config_with_genesis(genesis, 6);
        command.chain = Arc::new(botanix_testnet.clone());

        command
    }
}

impl PoaNodeCommandConfig for NonFederationMemberTestConfig {
    #[allow(clippy::unwrap_used)]
    fn on_node_started(&self, components: RethNodeComponents) -> eyre::Result<()> {
        it_info_print!("Engine started non federation task with index: ", self.index);

        let RethNodeComponents { executor, db, network } = components;

        let mut canon_events = db.subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        let peers_list = self.peers_list.clone();
        it_info_print!("RPC Engine peers list", peers_list.len());

        executor.spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    network.add_peer(peer.peer_id, peer_socket);
                    it_info_print!("RPC added peer", peer.peer_id);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers = network.get_all_peers().await.unwrap();
                it_info_print!(
                    "RPC Engine connected with peers",
                    format!("index={}: peers_count={}", engine_index, all_peers.len())
                );
                if all_peers.len() == peers_list.len() {
                    break 'inner;
                }
            }

            // start waiting for canon event notifications
            while let Ok(canon_state_notification) = canon_events.recv().await {
                let _ = rx_sender
                    .send(Notifications::CanonState(CannonStateNofificationPayload {
                        engine_index,
                        ts: tokio::time::Instant::now(),
                        notification: canon_state_notification,
                    }))
                    .unwrap();
            }
        }));

        Ok(())
    }
}

#[allow(clippy::cast_possible_truncation)]
pub async fn create_rpc_node(
    global_context: Arc<GlobalContext>,
    federation_members: HashMap<u16, FederationMemberTestConfig>,
) -> (NonFederationMemberTestConfig, tokio::sync::broadcast::Sender<Notifications>) {
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);

    let secp = secp256k1::Secp256k1::new();
    let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let pk = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
    let rpc_secret_key = hex_encode(secret_key.as_ref());
    let rpc_peer_id = pk2id(&pk);

    // set index as federation_members length + 1:
    // this will ensure the correct ports are used
    let index = federation_members.len() as u16 + 1;
    let mut rpc_node = NonFederationMemberTestConfig::new(
        index,
        rpc_secret_key,
        tx.clone(),
        global_context.bitcoind_url.clone(),
        global_context.bitcoind_user.clone(),
        global_context.bitcoind_pass.clone(),
        rpc_peer_id,
    );

    // Note: before we create the chain.toml edh and authorities list need to be set

    (rpc_node, tx)
}
