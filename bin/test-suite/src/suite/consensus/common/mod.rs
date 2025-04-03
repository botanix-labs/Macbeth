pub mod bitcoind_node;
pub mod botanix_client;
pub mod btc_server;
pub mod comet_node;
pub mod events;
pub mod poa_node;
pub mod rpc_node;

use anyhow::Context;
use botanix_client::BotanixEthClient;
use core::fmt;
use ethers::core::types::Address as EtherAddress;
use port_killer::kill;
use regex::Regex;
use reth::consensus_common::utils::unix_timestamp;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::mpsc::UnboundedReceiver,
};

#[derive(Debug, Clone, Copy)]
pub enum Scope {
    BtcServer(u16),
    Bitcoind,
    RpcNode(u16),
    PoaNode(u16),
    CometBFT(u16),
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Bitcoind => write!(f, "Bitcoind"),
            Scope::BtcServer(id) => write!(f, "BtcServer-{}", id),
            Scope::RpcNode(id) => write!(f, "RpcNode-{}", id),
            Scope::PoaNode(id) => write!(f, "PoaNode-{}", id),
            Scope::CometBFT(id) => write!(f, "CometBFT-{}", id),
        }
    }
}

pub const MINTING_CONTRACT_BYTECODE: &str = "60806040526004361061003f5760003560e01c80635fe03f45146100445780636f194dc914610066578063a5d0bb93146100b3578063a8de6d8c146100d6575b600080fd5b34801561005057600080fd5b5061006461005f366004610562565b6100fd565b005b34801561007257600080fd5b506100996100813660046105eb565b60006020819052908152604090205463ffffffff1681565b60405163ffffffff90911681526020015b60405180910390f35b6100c66100c136600461060d565b610422565b60405190151581526020016100aa565b3480156100e257600080fd5b506100ef6402540be40081565b6040519081526020016100aa565b60005a6001600160a01b03881660009081526020819052604090205490915063ffffffff9081169086161161018b5760405162461bcd60e51b815260206004820152602960248201527f7573657220626974636f696e426c6f636b486569676874206e6565647320746f60448201526820696e63726561736560b81b60648201526084015b60405180910390fd5b6001600160a01b0387166000908152602081905260408120805463ffffffff191663ffffffff88161790553a60016101c460048761068f565b61048560036107d36108fc805a6101db908b6106b1565b6101e591906106c8565b6101ef91906106c8565b6101f991906106c8565b61020391906106c8565b61020d91906106c8565b61021791906106c8565b61022191906106b1565b61022b91906106e0565b90508681111561027d5760405162461bcd60e51b815260206004820152601c60248201527f547820636f7374206578636565647320706567696e20616d6f756e74000000006044820152606401610182565b61028781886106b1565b96506000886001600160a01b03168860405160006040518083038185875af1925050503d80600081146102d6576040519150601f19603f3d011682016040523d82523d6000602084013e6102db565b606091505b505090508061032c5760405162461bcd60e51b815260206004820152601a60248201527f4d696e7420746f2064657374696e6174696f6e206661696c65640000000000006044820152606401610182565b6000846001600160a01b03168360405160006040518083038185875af1925050503d8060008114610379576040519150601f19603f3d011682016040523d82523d6000602084013e61037e565b606091505b50509050806103cf5760405162461bcd60e51b815260206004820152601e60248201527f526566756e6420746f20726566756e6441646472657373206661696c656400006044820152606401610182565b896001600160a01b03167f922344dc04648c0ce028ecdf9b2c9eed9a6794dbb47b777b54b0cfe069f128aa8a8a8a8a60405161040e9493929190610728565b60405180910390a250505050505050505050565b60006104356402540be40061014a6106e0565b34116104a95760405162461bcd60e51b815260206004820152603860248201527f56616c7565206d7573742062652067726561746572207468616e20647573742060448201527f616d6f756e74206f662033333020736174732f764279746500000000000000006064820152608401610182565b336001600160a01b03167f17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe171534878787876040516104ea959493929190610758565b60405180910390a2506001949350505050565b80356001600160a01b038116811461051457600080fd5b919050565b60008083601f84011261052b57600080fd5b50813567ffffffffffffffff81111561054357600080fd5b60208301915083602082850101111561055b57600080fd5b9250929050565b60008060008060008060a0878903121561057b57600080fd5b610584876104fd565b955060208701359450604087013563ffffffff811681146105a457600080fd5b9350606087013567ffffffffffffffff8111156105c057600080fd5b6105cc89828a01610519565b90945092506105df9050608088016104fd565b90509295509295509295565b6000602082840312156105fd57600080fd5b610606826104fd565b9392505050565b6000806000806040858703121561062357600080fd5b843567ffffffffffffffff8082111561063b57600080fd5b61064788838901610519565b9096509450602087013591508082111561066057600080fd5b5061066d87828801610519565b95989497509550505050565b634e487b7160e01b600052601160045260246000fd5b6000826106ac57634e487b7160e01b600052601260045260246000fd5b500490565b6000828210156106c3576106c3610679565b500390565b600082198211156106db576106db610679565b500190565b60008160001904831182151516156106fa576106fa610679565b500290565b81835281816020850137506000828201602090810191909152601f909101601f19169091010190565b84815263ffffffff8416602082015260606040820152600061074e6060830184866106ff565b9695505050505050565b8581526060602082015260006107726060830186886106ff565b82810360408401526107858185876106ff565b9897505050505050505056fea264697066735822122058bba5f85cc573a5323f630452faca186769309f0808e1ca3fdf25351f8d078264736f6c634300080d0033";
pub const MINT_CONTRACT_ADDRESS: &str = "0x0Ea320990B44236A0cEd0ecC0Fd2b2df33071e78";
pub const PREFUNDED_ACCOUNT_SECRET_KEY: &str =
    "52947524bbc14bd90cc86c32b9b7564da2f7f8de343825fed68cd04da4925d29";
pub fn kill_process_at_port(port: u16) {
    match kill(port) {
        Ok(pid) => {
            if pid {
                tracing::info!(
                    "Successfully killed server process on port process on port {:?}",
                    port
                );
            }
        }
        Err(err) => {
            tracing::error!(
                "Error attempting to kill server process on port {:?} -> {:?}",
                port,
                err
            );
        }
    }
}

pub fn spawn_child_process(
    scope: Scope,
    command: &str,
    args: Vec<&str>,
    process_pwd: impl AsRef<Path>,
) -> anyhow::Result<Child> {
    let (child, _, _) = spawn_child_process_internal(scope, command, args, process_pwd, false)?;
    Ok(child)
}

pub async fn spawn_await_child_process(
    scope: Scope,
    command: &str,
    args: Vec<&str>,
    process_pwd: impl AsRef<Path>,
) -> anyhow::Result<(Child, String, String)> {
    let (child, mut stdout_rx, mut stderr_rx) =
        spawn_child_process_internal(scope, command, args, process_pwd, true)?;

    let stdout_jh = tokio::task::spawn(async move {
        let mut stdout_buffer = String::new();
        while let Some(line) = stdout_rx.recv().await {
            stdout_buffer.push_str(&line);
        }
        stdout_buffer
    });

    let stderr_jh = tokio::task::spawn(async move {
        let mut stderr_buffer = String::new();
        while let Some(line) = stderr_rx.recv().await {
            stderr_buffer.push_str(&line);
        }
        stderr_buffer
    });

    let stdout = stdout_jh.await?;
    let stderr = stderr_jh.await?;

    Ok((child, stdout, stderr))
}

pub fn spawn_child_process_internal(
    scope: Scope,
    command: &str,
    args: Vec<&str>,
    process_pwd: impl AsRef<Path>,
    forward_messages: bool,
) -> anyhow::Result<(Child, UnboundedReceiver<String>, UnboundedReceiver<String>)> {
    let mut cmd = Command::new(command);
    cmd.args(&args).current_dir(process_pwd).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    // Open the log file for both stdout and stderr
    let log_file_path = PathBuf::from(format!("{}.txt", scope.to_string()));
    let log_file = OpenOptions::new().create(true).write(true).append(true).open(log_file_path)?;

    // Clone the log file
    let log_file_stdout = log_file.try_clone()?;
    let log_file_stderr = log_file.try_clone()?;

    // Set up channels for stdout and stderr
    let (stdout_tx, stdout_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stderr_tx, stderr_rx) = tokio::sync::mpsc::unbounded_channel();

    // Handle stdout logging and capturing
    if let Some(stdout) = child.stdout.take() {
        let stdout_tx = stdout_tx.clone();
        let mut file = tokio::fs::File::from_std(log_file_stdout);

        tokio::task::spawn(async move {
            let ansi_escape = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let clean_line = ansi_escape.replace_all(&line, "").to_string();
                tracing::info!("[{}] >>>>>> {}", scope, clean_line);

                // Write the line to the log file
                if let Err(e) = tokio::io::AsyncWriteExt::write_all(
                    &mut file,
                    format!("{}\n", clean_line).as_bytes(),
                )
                .await
                {
                    tracing::error!("Failed to write to log file: {}", e);
                }
                if let Err(e) = file.flush().await {
                    tracing::error!("Failed to flush log file: {}", e);
                }

                if forward_messages {
                    // Send the line over the channel
                    if let Err(e) = stdout_tx.send(format!("{}\n", clean_line)) {
                        tracing::error!("Failed to send stdout over channel: {}", e);
                    }
                }
            }
        });
    }

    // Handle stderr logging and capturing
    if let Some(stderr) = child.stderr.take() {
        let stderr_tx = stderr_tx.clone();
        let mut file = tokio::fs::File::from_std(log_file_stderr);

        tokio::task::spawn(async move {
            let ansi_escape = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let clean_line = ansi_escape.replace_all(&line, "").to_string();
                tracing::info!("[{}] >>>>>> {}", scope, clean_line);

                // Write the line to the log file
                if let Err(e) = tokio::io::AsyncWriteExt::write_all(
                    &mut file,
                    format!("{}\n", clean_line).as_bytes(),
                )
                .await
                {
                    tracing::error!("Failed to write to log file: {}", e);
                }
                if let Err(e) = file.flush().await {
                    tracing::error!("Failed to flush log file: {}", e);
                }

                if forward_messages {
                    // Send the line over the channel
                    if let Err(e) = stderr_tx.send(format!("{}\n", clean_line)) {
                        tracing::error!("Failed to send stderr over channel: {}", e);
                    }
                }
            }
        });
    }

    Ok((child, stdout_rx, stderr_rx))
}

pub async fn create_botanix_eth_client(
    rpc_port: u16,
    ws_port: u16,
) -> anyhow::Result<BotanixEthClient> {
    let mint_contract_address: EtherAddress =
        MINT_CONTRACT_ADDRESS.parse().context("Must be a valid ethereum address")?;
    Ok(BotanixEthClient::new(
        rpc_port,
        ws_port,
        PREFUNDED_ACCOUNT_SECRET_KEY,
        mint_contract_address,
    )
    .await?)
}

pub trait TemplateWriter {
    fn write_to_file(&self, path: impl AsRef<Path> + Send, filename: &str) -> anyhow::Result<()>
    where
        Self: askama::Template + serde::Serialize,
    {
        let rendered_template = self.render().context("Failed to render dynamic template")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.as_ref().to_path_buf().join(filename))
            .context("Failed to create/open a file")?;
        let res = file
            .write_all(rendered_template.as_bytes())
            .context("Failed to write contents to a file");
        res
    }
}

pub fn create_temp_working_directory() -> anyhow::Result<PathBuf> {
    let ret = tempfile::TempDir::new()
        .context("could not create temp. directory")?
        .into_path()
        .join(format!("_{}", unix_timestamp().to_string()));
    std::fs::create_dir_all(&ret).expect("failed to create tempdir subdir");
    Ok(ret)
}
