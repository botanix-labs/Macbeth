use crate::{
    it_info_print,
    suite::consensus::{
        common::{
            poa_node::{CannonStateNofificationPayload, FederationMemberTestConfig, Notifications},
            spawn_child_process, MINTING_CONTRACT_BYTECODE,
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
use reth::{
    args::FedMemberPubKey, commands::poa::PoaNodeComponents,
    consensus_common::utils::unix_timestamp, network::Peers,
};
use reth_primitives::{
    constants::nums_secp256k1_pk,
    extra_data_header::{ExtraDataHeader, CHAIN_VERSION, EXTRA_HEADER_VERSION},
    hex::encode as hex_encode,
    Address,
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
use tokio::process::Child;
use url::Url;

const RPC_PORT_BASE: u16 = 8545;
const DISCOVERY_PORT_BASE: u16 = 30321;

#[derive(Debug)]
pub struct SpawnedRpcServerProcess {
    pub rpc_port: u16,
    pub discovery_port: u16,
    pub child_process: Child,
}

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
    pub botanix_fee_recipient: String,
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
        botanix_fee_recipient: String,
    ) -> anyhow::Result<Self> {
        let rpc_port = RPC_PORT_BASE + index;
        let discovery_port = DISCOVERY_PORT_BASE + index;
        Ok(Self {
            index,
            temp_path: {
                let ret = tempfile::TempDir::new()
                    .context("error creating tempdir")?
                    .into_path()
                    .join(format!("_{}", unix_timestamp().to_string()));
                let _ = std::fs::create_dir_all(&ret).context("error creating tmpdir subdir")?;
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
            botanix_fee_recipient,
        })
    }

    pub fn insert_peers_list(&mut self, peers: Vec<FederationMemberTestConfig>) {
        self.peers_list = peers;
    }

    pub fn spawn_service(
        &mut self,
        edh_authorities_list: Arc<Vec<PublicKey>>,
        fed_member_peers_list: Vec<FederationMemberTestConfig>,
    ) -> anyhow::Result<SpawnedRpcServerProcess> {
        it_info_print!(format!("RPC Engine {} secret key = {}", self.index, &self.secret_key));
        self.insert_peers_list(fed_member_peers_list.clone());

        let datadir = self.temp_path.to_str().context("created temp path is unparsable")?;
        let discovery_secret_path = Path::new(&self.temp_path).join("discovery-secret");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(discovery_secret_path.clone())
            .context("discovery secret file cannot be created/opened")?;
        let _ = file
            .write_all(&self.secret_key.as_bytes())
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

        let federation_config = FederationTomlConfig::new(
            edh_authorities,
            self.botanix_fee_recipient.clone(),
            String::from(MINTING_CONTRACT_BYTECODE),
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
        working_directory.push("bin");
        working_directory.push("reth");

        let federation_config_path = federation_config_path.display().to_string();
        let rpc_port = self.rpc_port.to_string();
        let bitcoind_url = self.bitcoind_url.to_string();
        let bitcoind_username = self.bitcoind_username.clone();
        let bitcoind_password = self.bitcoind_password.clone();
        let discovery_port = self.discovery_port.to_string();

        // prepare run arguments
        let command = "cargo";
        let args = vec![
            "poa",
            "--is-testnet",
            "--ntp-server",
            "time.cloudflare.com",
            "--federation-config-path",
            federation_config_path.as_str(),
            "--datadir",
            datadir,
            "--debug.terminate",
            "--http",
            "--http.corsdomain",
            "*",
            "--http.port",
            rpc_port.as_str(),
            "--http.addr",
            "127.0.0.1",
            "--http.api",
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
        ];

        Ok(SpawnedRpcServerProcess {
            child_process: spawn_child_process(command, args, working_directory)?,
            discovery_port: self.discovery_port,
            rpc_port: self.rpc_port,
        })
    }
}

impl NonFederationMemberTestConfig {
    fn start_node<P>(&self, components: PoaNodeComponents<P>) -> anyhow::Result<()> {
        it_info_print!("Engine started non federation task with index: ", self.index);

        let PoaNodeComponents { task_executor, provider: db, network, .. } = components;

        let mut canon_events = db.subscribe_to_canonical_state();
        let rx_sender = self.sender.clone();
        let engine_index = self.index;

        let peers_list = self.peers_list.clone();
        it_info_print!("RPC Engine peers list", peers_list.len());

        task_executor.spawn(Box::pin(async move {
            // add the peers
            'inner: loop {
                for peer in peers_list.iter() {
                    let peer_socket = SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                        peer.discovery_port,
                    );
                    network.add_trusted_peer(peer.peer_id, peer_socket);
                    it_info_print!("RPC added peer", peer.peer_id);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let all_peers =
                    network.get_all_peers().await.expect("Error getting all peers from network");
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
                    .expect("Error sending a canon state notification payload");
            }
        }));

        Ok(())
    }
}

#[allow(clippy::cast_possible_truncation)]
pub async fn create_rpc_node(
    global_context: Arc<GlobalContext>,
    federation_members: HashMap<u16, FederationMemberTestConfig>,
    botanix_fee_recipeint: String,
) -> anyhow::Result<(NonFederationMemberTestConfig, tokio::sync::broadcast::Sender<Notifications>)>
{
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);

    let secp = secp256k1::Secp256k1::new();
    let secret_key = secp256k1::SecretKey::new(&mut rand::thread_rng());
    let pk = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
    let rpc_secret_key = hex_encode(secret_key.as_ref());
    let rpc_peer_id = pk2id(&pk);

    // set index as federation_members length + 1:
    // this will ensure the correct ports are used
    let index = federation_members.len() as u16 + 1;
    let rpc_node = NonFederationMemberTestConfig::new(
        index,
        rpc_secret_key,
        tx.clone(),
        global_context.bitcoind_url.clone(),
        global_context.bitcoind_user.clone(),
        global_context.bitcoind_pass.clone(),
        rpc_peer_id,
        botanix_fee_recipeint,
    );

    // Note: before we create the chain.toml edh and authorities list need to be set

    Ok((rpc_node?, tx))
}
