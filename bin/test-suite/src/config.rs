use argh::FromArgs;
use displaydoc::Display as DisplayDoc;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use thiserror::Error;
use tokio::{fs::File, io::AsyncReadExt};
use url::Url;

use crate::suite::RunSuite;

#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Open config file: {0}
    OpenConfig(std::io::Error),
    /// Failed to parse config: {0}
    ParseConfig(toml::de::Error),
    /// Failed to parse config as utf-8: {0}
    ParseUtf8(std::string::FromUtf8Error),
    /// Failed to read config file: {0}
    ReadConfig(std::io::Error),
    /// Failed to read config metadata: {0}
    ReadMeta(std::io::Error),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {}

impl Config {
    pub async fn new(path: impl AsRef<Path> + Send) -> Result<Self, Error> {
        read_to_string(path).await?.parse()
    }

    pub fn from_envs(&mut self) {}
}

impl FromStr for Config {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s).map_err(Error::ParseConfig)
    }
}

async fn read_to_string(path: impl AsRef<Path> + Send) -> Result<String, Error> {
    let mut file = File::open(path).await.map_err(Error::OpenConfig)?;
    let meta = file.metadata().await.map_err(Error::ReadMeta)?;
    let mut contents = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.read_to_end(&mut contents).await.map_err(Error::ReadConfig)?;
    Ok(String::from_utf8(contents).map_err(Error::ParseUtf8)?)
}

/// Test Suite Service
#[derive(FromArgs)]
pub struct CliArgs {
    /// path to the toml config file
    #[argh(option, short = 'c')]
    pub config: String,
    /// suite of tests to run: Consensus|all (default: all)
    #[argh(option, short = 'r', from_str_fn(parse_suite), default = "RunSuite::Consensus")]
    pub run_suite: RunSuite,
    /// individual test timeout in milliseconds (default: 20000)
    #[argh(option, short = 't', default = "20_000")]
    pub timeout: u64,
    /// dry run to perform (default: false)
    #[argh(option, short = 'd', default = "false")]
    pub dry_run: bool,
    /// jwt directory
    #[argh(option, short = 'j')]
    pub jwt_dir: PathBuf,
    /// min frost signers
    #[argh(option, default = "2")]
    pub min_signers: u16,
    /// max frost signers
    #[argh(option, default = "2")]
    pub max_signers: u16,
    /// btc network
    #[argh(option, default = "String::from(\"regtest\")")]
    pub btc_network: String,
    /// btc rpc url
    #[argh(option, from_str_fn(parse_url))]
    pub bitcoind_url: Url,
    /// btc user
    #[argh(option, default = "String::from(\"foo\")")]
    pub bitcoind_user: String,
    #[argh(option, default = "String::from(\"bar\")")]
    /// btc password
    pub bitcoind_pass: String,
}

pub fn parse_suite(value: &str) -> Result<RunSuite, String> {
    value.parse().map_err(|_| format!("Failed to parse RunSuite: {}", value))
}

pub fn parse_url(value: &str) -> Result<Url, String> {
    Url::parse(value).map_err(|_| format!("Failed to parse url: {}", value))
}
