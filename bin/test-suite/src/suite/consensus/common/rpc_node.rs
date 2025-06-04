use crate::{
    it_error_print, it_info_print,
    suite::consensus::{
        common::{
            poa_node::FederationMemberTestConfig, spawn_child_process, Scope,
            MINTING_CONTRACT_BYTECODE,
        },
        GlobalContext,
    },
};
use anyhow::Context;
use reth_chainspec::BotanixTestnetGenesisConfig;
use reth_network_peers::pk2id;
use reth_node_core::args::FederationTomlConfig;

use askama::Template;
use bitcoin::{hashes::Hash, BlockHash};
use reth::args::FedMemberPubKey;
use reth_primitives::{
    constants::nums_secp256k1_pk,
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    Address,
};
use reth_rpc_types::PeerId;
use secp256k1::{PublicKey, SecretKey, SECP256K1};
use std::{
    collections::BTreeMap,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::process::Child;
use url::Url;

use super::{
    botanix_client::BotanixEthClient,
    create_temp_working_directory, kill_process_at_port,
    poa_node::{ABCI_PORT_BASE, DISCOVERY_PORT_BASE, RPC_PORT_BASE, WS_PORT_BASE},
};

#[derive(Clone, Debug)]
pub enum Notifications {}

#[derive(Debug)]
pub struct SpawnedRpcServerProcess {
    pub rpc_port: u16,
    pub ws_port: u16,
    pub discovery_port: u16,
    pub child_process: Child,
}

impl SpawnedRpcServerProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        // additionally make sure all ports used are freed
        kill_process_at_port(self.discovery_port);
        kill_process_at_port(self.rpc_port);
        kill_process_at_port(self.ws_port);
    }

    pub async fn destroy_all_sync(&self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        // additionally make sure all ports used are freed
        kill_process_at_port(self.discovery_port);
        kill_process_at_port(self.rpc_port);
        kill_process_at_port(self.ws_port);
    }
}
#[derive(Clone, Debug)]
pub struct NonFederationMemberTestConfig {
    pub index: u16,
    pub temp_path: PathBuf,
    pub secret_key: SecretKey,
    pub rpc_port: u16,
    pub ws_port: u16,
    pub discovery_port: u16,
    pub abci_port: u16,
    pub bitcoind_url: Url,
    pub bitcoind_username: String,
    pub bitcoind_password: String,
    pub peers_list: Vec<FederationMemberTestConfig>,
    pub sender: tokio::sync::broadcast::Sender<Notifications>,
    pub peer_id: PeerId,
    pub botanix_fee_recipient: String,
    pub botanix_eth_client: Option<BotanixEthClient>,
    pub lst_fee_receiver: String,
}

impl NonFederationMemberTestConfig {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        index: u16,
        secret_key: SecretKey,
        sender: tokio::sync::broadcast::Sender<Notifications>,
        bitcoind_url: Url,
        bitcoind_username: String,
        bitcoind_password: String,
        peer_id: PeerId,
        botanix_fee_recipient: String,
        rpc_port: u16,
        ws_port: u16,
        discovery_port: u16,
        abci_port: u16,
        lst_fee_receiver: String,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            index,
            temp_path: create_temp_working_directory()?,
            secret_key,
            rpc_port,
            ws_port,
            discovery_port,
            abci_port,
            bitcoind_url,
            bitcoind_username,
            bitcoind_password,
            peers_list: vec![],
            sender,
            peer_id,
            botanix_fee_recipient,
            botanix_eth_client: None,
            lst_fee_receiver,
        })
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    #[allow(clippy::too_many_lines)]
    pub fn spawn_service(
        &mut self,
        edh_authorities_list: Arc<Vec<PublicKey>>,
        poa_nodes: Vec<FederationMemberTestConfig>,
    ) -> anyhow::Result<SpawnedRpcServerProcess> {
        it_info_print!(format!("RPC Engine {} secret key = {:?}", self.index, &self.secret_key));
        self.insert_peers_list(poa_nodes.clone());

        let datadir = self.temp_path.to_str().context("created temp path is unparsable")?;
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .context("discovery secret file cannot be created/opened")?;
        let _ = file
            .write_all(&self.secret_key.display_secret().to_string().as_bytes())
            .context("error writing secret key to file")?;

        // now create the edh
        let edh = ExtraDataHeader::new(
            EXTRA_HEADER_VERSION,
            CHAIN_VERSION,
            BlockHash::hash(&[1]),
            nums_secp256k1_pk(),
            Address::ZERO,
        );

        // update genesis config with edh and render file
        let _botanix_testnet_config_genesis = {
            let edh = hex::encode(edh.serialize());
            let botanix_testnet_config_genesis = BotanixTestnetGenesisConfig { edh: &edh };
            let rendered_json = botanix_testnet_config_genesis
                .render()
                .context("error rendering botanix testnet genesis config")?;
            rendered_json
        };

        // Need to create a chain.toml in the data dir
        // Need to zip together the soc address and pk
        let mut fed_member_pks = vec![];
        for peer in poa_nodes.iter() {
            let pk = FedMemberPubKey {
                key: peer.secret_key.public_key(SECP256K1).to_string(),
                socket_addr: format!("127.0.0.1:{}", peer.discovery_port),
            };
            fed_member_pks.push(pk);
        }

        let mut edh_authorities = vec![];
        for authority in edh_authorities_list.to_vec().iter() {
            for pk in fed_member_pks.iter() {
                if pk.key == authority.to_string() {
                    edh_authorities.push(pk.clone());
                    break;
                }
            }
        }

        // Need to create a federation.toml in the data dir
        let federation_config = FederationTomlConfig::new(
            edh_authorities,
            self.botanix_fee_recipient.clone(),
            String::from(MINTING_CONTRACT_BYTECODE),
            self.lst_fee_receiver.clone(),
        );
        it_info_print!("Federation config", federation_config);
        let federation_config_path = Path::new(datadir).join("federation.toml");
        federation_config
            .write_to_path(&federation_config_path)
            .context("Error writing federation config to path")?;

        // point to the relevant working directory
        let mut working_directory =
            std::env::current_dir().context("Error obtaining current directory")?;
        for _ in 0..2 {
            working_directory.pop();
        }

        let federation_config_path = federation_config_path.display().to_string();
        let rpc_port = self.rpc_port.to_string();
        let ws_port = self.ws_port.to_string();
        let bitcoind_url = self.bitcoind_url.to_string();
        let bitcoind_username = self.bitcoind_username.clone();
        let bitcoind_password = self.bitcoind_password.clone();
        let discovery_port = self.discovery_port.to_string();
        let abci_port = self.abci_port.to_string();

        // prepare run arguments
        let command = "./target/debug/reth";
        let binary_abs_path = working_directory.join(Path::new(command));
        if !std::fs::exists(&binary_abs_path)? {
            return Err(anyhow::anyhow!(
                "reth binary not found at {}. Please compile it first before running the test-suite",
                binary_abs_path.display().to_string()
            ));
        }
        let args = vec![
            "poa",
            "-vvv",
            "--disable-discovery",
            "--is-testnet",
            "--ntp-server",
            "time.cloudflare.com",
            "--federation-config-path",
            federation_config_path.as_str(),
            "--ipcdisable",
            "--datadir",
            datadir,
            "--debug.terminate",
            "--http",
            "--http.corsdomain",
            "*",
            "--http.port",
            rpc_port.as_str(),
            "--http.addr",
            "0.0.0.0",
            "--http.api",
            "eth,net,trace,txpool,web3,rpc,admin",
            "--ws",
            "--ws.addr",
            "0.0.0.0",
            "--ws.origins",
            "*",
            "--ws.port",
            ws_port.as_str(),
            "--ws.api",
            "eth,net,trace,txpool,web3,rpc,admin",
            "--btc-network",
            "regtest",
            "--bitcoind.url",
            bitcoind_url.as_str(),
            "--bitcoind.username",
            bitcoind_username.as_str(),
            "--bitcoind.password",
            bitcoind_password.as_str(),
            "--port",
            discovery_port.as_str(),
            "--p2p-secret-key",
            discovery_secret_path.to_str().context("discovery secret path to exist")?,
            "--abci-port",
            abci_port.as_str(),
            "--sync.enable_state_sync",
            "--sync.enable_historical_sync",
        ];

        Ok(SpawnedRpcServerProcess {
            child_process: spawn_child_process(
                Scope::RpcNode(self.index),
                command,
                args,
                working_directory,
            )?,
            discovery_port: self.discovery_port,
            rpc_port: self.rpc_port,
            ws_port: self.ws_port,
        })
    }

    pub fn await_initialization(&self) -> anyhow::Result<()> {
        it_info_print!("Engine started non federation task with index: ", self.index);
        let engine_index = self.index;
        let peers_list = self.peers_list.clone();
        it_info_print!("RPC Engine peers list", peers_list.len());
        let botanix_eth_client = self
            .botanix_eth_client
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Uninitialized botanix eth client"))?;

        tokio::spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    let enode_url =
                        format!("enode://{}@{}", peer.peer_id.to_string(), peer_socket.to_string());
                    if let Err(_) = botanix_eth_client.add_trusted_peer(&enode_url).await {
                        it_error_print!("RPC failed to add a peer", peer.peer_id);
                    } else {
                        it_info_print!("RPC added peer", peer.peer_id);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers = botanix_eth_client.get_peers_counts().await.unwrap_or_default();
                it_info_print!(
                    "RPC Engine connected with peers",
                    format!("index={}: peers_count={}", engine_index, all_peers.len())
                );
                if all_peers.len() == peers_list.len() {
                    break 'inner;
                }
            }
        }));

        Ok(())
    }
}

#[allow(clippy::cast_possible_truncation)]
pub async fn create_rpc_nodes(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<(
    BTreeMap<u16, NonFederationMemberTestConfig>,
    tokio::sync::broadcast::Sender<Notifications>,
)> {
    let secp = secp256k1::Secp256k1::new();
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);
    let mut rpc_members: BTreeMap<u16, NonFederationMemberTestConfig> = BTreeMap::new();

    // create all rpc instances
    for member_index in 0..global_context.rpc_instances {
        let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
        let rpc_peer_id = pk2id(&pk);

        let rpc_node = NonFederationMemberTestConfig::new(
            global_context.fed_instances + member_index, // indexing follows up from poa nodes onwards
            secret_key,
            tx.clone(),
            global_context.bitcoind_url.clone(),
            global_context.bitcoind_user.clone(),
            global_context.bitcoind_pass.clone(),
            rpc_peer_id,
            global_context.botanix_fee_recipient.clone(),
            RPC_PORT_BASE + global_context.fed_instances + member_index, /* Note: make sure we
                                                                          * start port assigning
                                                                          * after poa servers */
    WS_PORT_BASE + global_context.fed_instances + member_index, /* Note: make sure we
                                                                          * start port assigning
                                                                          * after poa servers */
            DISCOVERY_PORT_BASE + global_context.fed_instances + member_index, /* Note: make sure we start port assigning after poa servers */
            ABCI_PORT_BASE + 1000 * (global_context.fed_instances + member_index),
            global_context.lst_fee_receiver.clone(),
        ).await?;
        rpc_members.insert(global_context.fed_instances + member_index, rpc_node);
    }

    // Note: before we create the chain.toml edh and authorities list need to be set
    Ok((rpc_members, tx))
}
