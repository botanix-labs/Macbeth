use super::{
    botanix_client::BotanixEthClient, kill_process_at_port, poa_node::ABCI_PORT_BASE, Scope,
};
use crate::{
    context::GlobalContext,
    suite::consensus::common::{create_temp_working_directory, spawn_child_process},
};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
};
use tokio::{
    process::Child,
    sync::broadcast::{channel, Sender},
};

#[derive(Clone, Debug)]
pub enum Notifications {}

#[derive(Clone, Debug)]
pub enum TestSignal {
    DisconnectAll(),
    ReconnectAll(),
}

// =============================== COMETBFT CONFIG FILES =========================== //

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisValidator {
    address: String,
    pub_key: ValidatorData,
    power: String,
    name: String,
}

impl From<&PrivValidator> for GenesisValidator {
    fn from(priv_validator: &PrivValidator) -> Self {
        Self {
            address: priv_validator.address.clone(),
            pub_key: priv_validator.pub_key.clone(),
            power: "10".to_string(),
            name: "".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivValidator {
    address: String,
    pub_key: ValidatorData,
    priv_key: ValidatorData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorData {
    #[serde(rename = "type")]
    type_: String,
    value: String,
}

#[derive(Debug)]
pub struct SpawnedCometBftProcess {
    pub cometbft_proxy_app_port: u16,
    pub cometbft_rpc_app_port: u16,
    pub cometbft_p2p_app_port: u16,
    pub child_process: Child,
}

impl SpawnedCometBftProcess {
    pub async fn destroy_all_async(&mut self) {
        // kill the process
        let _ = self.child_process.kill().await;
        // additionally make sure all ports used are freed
        kill_process_at_port(self.cometbft_proxy_app_port);
        kill_process_at_port(self.cometbft_rpc_app_port);
        kill_process_at_port(self.cometbft_p2p_app_port);
    }

    pub async fn destroy_all_sync(&mut self) {
        // kill the process
        let pid = self.child_process.id().expect("Expected a process id");
        let _ = std::process::Command::new("kill")
            .arg("-9") // Use SIGKILL for immediate termination
            .arg(format!("{pid}"))
            .output();
        // additionally make sure all ports used are freed
        kill_process_at_port(self.cometbft_proxy_app_port);
        kill_process_at_port(self.cometbft_rpc_app_port);
        kill_process_at_port(self.cometbft_p2p_app_port);
    }
}

#[derive(Clone, Debug)]
pub struct CometBftNodeConfig {
    pub index: u16,
    pub working_directory: PathBuf,
    pub validator: PrivValidator,
    pub cometbft_proxy_app_port: u16,
    pub cometbft_rpc_app_port: u16,
    pub cometbft_p2p_app_port: u16,
    pub peers_list: Vec<CometBftNodeConfig>,
    pub peer_id: String,
    pub botanix_eth_client: Option<BotanixEthClient>,
    pub test_signal_tx: Sender<TestSignal>,
}

impl CometBftNodeConfig {
    pub async fn new(
        index: u16,
        validator: PrivValidator,
        peer_id: String,
        cometbft_proxy_app_port: u16,
        cometbft_rpc_app_port: u16,
        cometbft_p2p_app_port: u16,
        test_signal_tx: Sender<TestSignal>,
        working_directory: PathBuf,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            index,
            working_directory,
            validator,
            peers_list: vec![],
            peer_id,
            botanix_eth_client: None,
            cometbft_proxy_app_port,
            cometbft_rpc_app_port,
            cometbft_p2p_app_port,
            test_signal_tx,
        })
    }

    pub fn insert_peers_list(&mut self, peers: Vec<CometBftNodeConfig>) {
        self.peers_list = peers;
    }

    pub fn peers_list(&self) -> Vec<CometBftNodeConfig> {
        self.peers_list.clone()
    }

    pub fn spawn_service(&self) -> anyhow::Result<SpawnedCometBftProcess> {
        // prepare run arguments
        let home_path = self.working_directory.to_path_buf();
        let home_path_str = home_path.display().to_string();
        let command = "cometbft";
        let args = vec!["start", "--home", &home_path_str];

        Ok(SpawnedCometBftProcess {
            child_process: spawn_child_process(
                Scope::CometBFT(self.index),
                command,
                args,
                self.working_directory.clone(),
            )?,
            cometbft_proxy_app_port: self.cometbft_proxy_app_port,
            cometbft_rpc_app_port: self.cometbft_rpc_app_port,
            cometbft_p2p_app_port: self.cometbft_p2p_app_port,
        })
    }
}

impl CometBftNodeConfig {
    pub fn await_initialization(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn init_cometbft_node(
    index: u16,
    working_directory: &PathBuf,
) -> anyhow::Result<(ExitStatus, String, String)> {
    let working_dir_str = working_directory.display().to_string();
    let command = "cometbft";
    let args = vec!["init", "--home", &working_dir_str];
    let child = spawn_child_process(Scope::CometBFT(index), command, args, working_directory)?;
    let output = child.wait_with_output().await?;
    let exit_status = output.status;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    Ok((exit_status, stdout, stderr))
}

async fn get_enode(
    index: u16,
    working_directory: &PathBuf,
) -> anyhow::Result<(ExitStatus, String, String)> {
    let working_dir_str = working_directory.display().to_string();
    let command = "cometbft";
    let args = vec!["show-node-id", "--home", &working_dir_str];
    let child = spawn_child_process(Scope::CometBFT(index), command, args, working_directory)?;
    let output = child.wait_with_output().await?;
    let exit_status = output.status;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    Ok((exit_status, stdout, stderr))
}

fn updated_genesis_file(
    working_directory: &PathBuf,
    all_validators: Vec<GenesisValidator>,
) -> anyhow::Result<()> {
    // read genesis.json file and update some keys
    let genesis_file = Path::new(&working_directory).join("config").join("genesis.json");
    let mut genesis_object =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&genesis_file)?)
            .context("Error reading genesis.json file")?;

    if let Some(chain_id) = genesis_object.get_mut("chain_id") {
        *chain_id = serde_json::Value::String("3636".to_string());
    }

    if let Some(max_gas) = genesis_object.pointer_mut("/consensus_params/block/max_gas") {
        *max_gas = json!("-1");
    }

    if let Some(vote_extensions_enable_height) =
        genesis_object.pointer_mut("/consensus_params/feature/vote_extensions_enable_height")
    {
        *vote_extensions_enable_height = json!("0");
    }

    if let Some(vote_extensions_enable_height) =
        genesis_object.pointer_mut("/consensus_params/feature/pbts_enable_height")
    {
        *vote_extensions_enable_height = json!("1");
    }

    if let Some(validators) = genesis_object.pointer_mut("/validators") {
        *validators = json!(all_validators);
    }

    // Serialize the modified object and write it back to the file
    let updated_content = serde_json::to_string_pretty(&genesis_object)
        .context("Failed to serialize updated genesis.json content")?;

    fs::write(&genesis_file, updated_content)
        .context("Failed to write updated genesis.json file")?;

    Ok(())
}

fn update_config_toml(cometbft_node: &CometBftNodeConfig) -> anyhow::Result<()> {
    let config_file =
        Path::new(&cometbft_node.working_directory).join("config").join("config.toml");
    let mut toml: toml::Value = toml::from_str(&fs::read_to_string(&config_file)?)
        .context("Unable to parse toml config file")?;
    if let Some(proxy_app_port) = toml.get_mut("proxy_app") {
        *proxy_app_port = toml::value::Value::String(format!(
            "tcp://127.0.0.1:{}",
            cometbft_node.cometbft_proxy_app_port.to_string()
        ));
    }
    if let Some(rpc) = toml.get_mut("rpc") {
        if let Some(laddr) = rpc.get_mut("laddr") {
            *laddr = toml::value::Value::String(cometbft_node.cometbft_rpc_app_port.to_string());
        }
    }
    if let Some(rpc) = toml.get_mut("p2p") {
        if let Some(allow_duplicate_ip) = rpc.get_mut("allow_duplicate_ip") {
            *allow_duplicate_ip = toml::value::Value::Boolean(true);
        }
        if let Some(addr_book_strict) = rpc.get_mut("addr_book_strict") {
            *addr_book_strict = toml::value::Value::Boolean(false);
        }
        if let Some(cometbft_p2p_app_port) = rpc.get_mut("laddr") {
            *cometbft_p2p_app_port = toml::value::Value::String(format!(
                "tcp://0.0.0.0:{}",
                cometbft_node.cometbft_p2p_app_port.to_string()
            ));
        }
        if let Some(persistent_peers) = rpc.get_mut("persistent_peers") {
            let peer_ids = cometbft_node
                .peers_list
                .iter()
                .map(|peer| format!("{}@127.0.0.1:{}", peer.peer_id, peer.cometbft_p2p_app_port))
                .collect::<Vec<String>>()
                .join(",");
            *persistent_peers = toml::value::Value::String(peer_ids);
        }
    }

    // Serialize the modified object and write it back to the file
    let updated_content =
        toml::to_string_pretty(&toml).context("Failed to serialize updated config.toml content")?;
    fs::write(&config_file, updated_content).context("Failed to write updated config.toml file")?;

    Ok(())
}

pub async fn create_cometbft_nodes(
    global_context: Arc<GlobalContext>,
) -> anyhow::Result<(HashMap<u16, CometBftNodeConfig>, tokio::sync::broadcast::Sender<Notifications>)>
{
    let (tx, _rx) = tokio::sync::broadcast::channel::<Notifications>(100);
    let mut cometbft_nodes: HashMap<u16, CometBftNodeConfig> = HashMap::new();

    // loop and crete all cometbft nodes
    for member_index in 0..global_context.fed_instances {
        // allocate ports
        let cometbft_proxy_app_port = ABCI_PORT_BASE + 10000 * member_index;
        let cometbft_rpc_app_port = cometbft_proxy_app_port - 1;
        let cometbft_p2p_app_port = cometbft_rpc_app_port - 1;

        // init working directory
        let working_directory = create_temp_working_directory();

        // init cometbft node
        let (exit_status, stdout, stderr) = init_cometbft_node(member_index, &working_directory)
            .await
            .context("Error initializing cometbft node")?;
        if !exit_status.success() {
            tracing::error!(
                "CometBFT node failed to initialize: {:?} {:?} {:?}",
                exit_status,
                stdout,
                stderr
            );
            return Err(anyhow::anyhow!(
                "CometBFT node failed to initialize: {:?} {:?}",
                exit_status,
                stderr
            ));
        }
        tracing::info!("CometBFT node initialized: {:?}", exit_status.success());

        // read priv_validator_key.json file
        let priv_validator_key_file =
            Path::new(&working_directory).join("config").join("priv_validator_key.json");
        let validator =
            serde_json::from_str::<PrivValidator>(&fs::read_to_string(priv_validator_key_file)?)
                .context("Error reading priv_validator_key.json file")?;

        // get enode
        let (exit_status, stdout, stderr) =
            get_enode(member_index, &working_directory).await.context("Error getting enode")?;
        if !exit_status.success() {
            tracing::error!(
                "CometBFT enode failed to be obtained: {:?} {:?} {:?}",
                exit_status,
                stdout,
                stderr
            );
            return Err(anyhow::anyhow!(
                "CometBFT enode failed to be obtained: {:?} {:?}",
                exit_status,
                stderr
            ));
        }
        let enode = stdout;
        tracing::info!("CometBFT enode: {:?}", enode);

        // prepare test signal
        let (test_signal_tx, _test_signal_rx) = channel::<TestSignal>(10);

        // create the cometbft node
        let cometbft_node = CometBftNodeConfig::new(
            member_index,
            validator,
            enode,
            cometbft_proxy_app_port,
            cometbft_rpc_app_port,
            cometbft_p2p_app_port,
            test_signal_tx,
            working_directory,
        )
        .await?;

        // persist node config
        cometbft_nodes.insert(member_index, cometbft_node);
    }

    // extract validators set
    let all_genesis_validators = cometbft_nodes
        .iter()
        .map(|(_, config)| GenesisValidator::from(&config.validator))
        .collect::<Vec<GenesisValidator>>();

    // now insert peers into each cometbft member
    for member_index in 0..global_context.fed_instances {
        // get the cometbft node
        let cometbft_node =
            cometbft_nodes.get(&member_index).cloned().expect("To have cometbft node");

        // read genesis.json file and update some keys
        updated_genesis_file(&cometbft_node.working_directory, all_genesis_validators.clone())
            .context("Error updating genesis file")?;

        // get all node counterpeers
        let validator_peer_members = cometbft_nodes
            .iter()
            .filter_map(
                |(index, fed_mem)| {
                    if *index != member_index {
                        Some(fed_mem.clone())
                    } else {
                        None
                    }
                },
            )
            .collect::<Vec<_>>();

        if let Some(cometbft_node) = cometbft_nodes.get_mut(&member_index) {
            cometbft_node.insert_peers_list(validator_peer_members);
            // update config.toml file
            update_config_toml(&cometbft_node).context("Error updating config toml file")?;
        };
    }

    Ok((cometbft_nodes, tx))
}
