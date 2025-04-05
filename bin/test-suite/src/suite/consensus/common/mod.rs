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

pub const MINTING_CONTRACT_BYTECODE: &str = "60806040526004361061003f5760003560e01c80635fe03f45146100445780636f194dc91461006d578063a5d0bb93146100aa578063a8de6d8c146100da575b600080fd5b34801561005057600080fd5b5061006b60048036038101906100669190610724565b610105565b005b34801561007957600080fd5b50610094600480360381019061008f91906107be565b610501565b6040516100a191906107fa565b60405180910390f35b6100c460048036038101906100bf9190610815565b610524565b6040516100d191906108b1565b60405180910390f35b3480156100e657600080fd5b506100ef6105dc565b6040516100fc91906108db565b60405180910390f35b60005a905060008060008973ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900463ffffffff1690506000808973ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900463ffffffff1663ffffffff168663ffffffff16116101f9576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004016101f090610979565b60405180910390fd5b856000808a73ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060006101000a81548163ffffffff021916908363ffffffff16021790555060003a600160048888905061026b91906109f7565b61048560036107d36108fc805a8b6102839190610a28565b61028d9190610a5c565b6102979190610a5c565b6102a19190610a5c565b6102ab9190610a5c565b6102b59190610a5c565b6102bf9190610a5c565b6102c99190610a28565b6102d39190610ab2565b905087811115610318576040517f08c379a000000000000000000000000000000000000000000000000000000000815260040161030f90610b58565b60405180910390fd5b80886103249190610a28565b975060008973ffffffffffffffffffffffffffffffffffffffff168960405161034c90610ba9565b60006040518083038185875af1925050503d8060008114610389576040519150601f19603f3d011682016040523d82523d6000602084013e61038e565b606091505b50509050806103d2576040517f08c379a00000000000000000000000000000000000000000000000000000000081526004016103c990610c0a565b60405180910390fd5b60008573ffffffffffffffffffffffffffffffffffffffff16836040516103f890610ba9565b60006040518083038185875af1925050503d8060008114610435576040519150601f19603f3d011682016040523d82523d6000602084013e61043a565b606091505b505090508061047e576040517f08c379a000000000000000000000000000000000000000000000000000000000815260040161047590610c76565b60405180910390fd5b60008463ffffffff1660208b63ffffffff1667ffffffffffffffff16901b1790508b73ffffffffffffffffffffffffffffffffffffffff167f9de7365c663dc09a824437fcfe283fde0349736c62570a07a36e47f9a5dcaf0f8c838c8c6040516104eb9493929190610d17565b60405180910390a2505050505050505050505050565b60006020528060005260406000206000915054906101000a900463ffffffff1681565b60006402540be40061014a6105399190610ab2565b341161057a576040517f08c379a000000000000000000000000000000000000000000000000000000000815260040161057190610dc9565b60405180910390fd5b3373ffffffffffffffffffffffffffffffffffffffff167f17f87987da8ca71c697791dcfd190d07630cf17bf09c65c5a59b8277d9fe171534878787876040516105c8959493929190610de9565b60405180910390a260019050949350505050565b6402540be40081565b600080fd5b600080fd5b600073ffffffffffffffffffffffffffffffffffffffff82169050919050565b600061061a826105ef565b9050919050565b61062a8161060f565b811461063557600080fd5b50565b60008135905061064781610621565b92915050565b6000819050919050565b6106608161064d565b811461066b57600080fd5b50565b60008135905061067d81610657565b92915050565b600063ffffffff82169050919050565b61069c81610683565b81146106a757600080fd5b50565b6000813590506106b981610693565b92915050565b600080fd5b600080fd5b600080fd5b60008083601f8401126106e4576106e36106bf565b5b8235905067ffffffffffffffff811115610701576107006106c4565b5b60208301915083600182028301111561071d5761071c6106c9565b5b9250929050565b60008060008060008060a08789031215610741576107406105e5565b5b600061074f89828a01610638565b965050602061076089828a0161066e565b955050604061077189828a016106aa565b945050606087013567ffffffffffffffff811115610792576107916105ea565b5b61079e89828a016106ce565b935093505060806107b189828a01610638565b9150509295509295509295565b6000602082840312156107d4576107d36105e5565b5b60006107e284828501610638565b91505092915050565b6107f481610683565b82525050565b600060208201905061080f60008301846107eb565b92915050565b6000806000806040858703121561082f5761082e6105e5565b5b600085013567ffffffffffffffff81111561084d5761084c6105ea565b5b610859878288016106ce565b9450945050602085013567ffffffffffffffff81111561087c5761087b6105ea565b5b610888878288016106ce565b925092505092959194509250565b60008115159050919050565b6108ab81610896565b82525050565b60006020820190506108c660008301846108a2565b92915050565b6108d58161064d565b82525050565b60006020820190506108f060008301846108cc565b92915050565b600082825260208201905092915050565b7f7573657220626974636f696e426c6f636b486569676874206e6565647320746f60008201527f20696e6372656173650000000000000000000000000000000000000000000000602082015250565b60006109636029836108f6565b915061096e82610907565b604082019050919050565b6000602082019050818103600083015261099281610956565b9050919050565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601260045260246000fd5b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601160045260246000fd5b6000610a028261064d565b9150610a0d8361064d565b925082610a1d57610a1c610999565b5b828204905092915050565b6000610a338261064d565b9150610a3e8361064d565b925082821015610a5157610a506109c8565b5b828203905092915050565b6000610a678261064d565b9150610a728361064d565b9250827fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff03821115610aa757610aa66109c8565b5b828201905092915050565b6000610abd8261064d565b9150610ac88361064d565b9250817fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0483118215151615610b0157610b006109c8565b5b828202905092915050565b7f547820636f7374206578636565647320706567696e20616d6f756e7400000000600082015250565b6000610b42601c836108f6565b9150610b4d82610b0c565b602082019050919050565b60006020820190508181036000830152610b7181610b35565b9050919050565b600081905092915050565b50565b6000610b93600083610b78565b9150610b9e82610b83565b600082019050919050565b6000610bb482610b86565b9150819050919050565b7f4d696e7420746f2064657374696e6174696f6e206661696c6564000000000000600082015250565b6000610bf4601a836108f6565b9150610bff82610bbe565b602082019050919050565b60006020820190508181036000830152610c2381610be7565b9050919050565b7f526566756e6420746f20726566756e6441646472657373206661696c65640000600082015250565b6000610c60601e836108f6565b9150610c6b82610c2a565b602082019050919050565b60006020820190508181036000830152610c8f81610c53565b9050919050565b600067ffffffffffffffff82169050919050565b610cb381610c96565b82525050565b600082825260208201905092915050565b82818337600083830152505050565b6000601f19601f8301169050919050565b6000610cf68385610cb9565b9350610d03838584610cca565b610d0c83610cd9565b840190509392505050565b6000606082019050610d2c60008301876108cc565b610d396020830186610caa565b8181036040830152610d4c818486610cea565b905095945050505050565b7f56616c7565206d7573742062652067726561746572207468616e20647573742060008201527f616d6f756e74206f662033333020736174732f76427974650000000000000000602082015250565b6000610db36038836108f6565b9150610dbe82610d57565b604082019050919050565b60006020820190508181036000830152610de281610da6565b9050919050565b6000606082019050610dfe60008301886108cc565b8181036020830152610e11818688610cea565b90508181036040830152610e26818486610cea565b9050969550505050505056fea26469706673582212201986e74c0d677f1f1a8217659d49064a397bce160c954a05a375c7552226cdbb64736f6c634300080d0033";
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
