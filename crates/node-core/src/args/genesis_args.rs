use displaydoc::Display as DisplayDoc;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    str::FromStr,
};
use thiserror::Error;

/// Error type for genesis config
#[derive(Debug, DisplayDoc, Error)]
pub enum Error {
    /// Open config file: {0}
    #[allow(dead_code)]
    OpenConfig(std::io::Error),
    /// Failed to parse config: {0}
    ParseConfig(toml::de::Error),
    /// Failed to serialize parse config: {0}
    ParseSerializeConfig(toml::ser::Error),
    /// Failed to parse config as utf-8: {0}
    #[allow(dead_code)]
    ParseUtf8(std::string::FromUtf8Error),
    /// Failed to read config file: {0}
    #[allow(dead_code)]
    ReadConfig(std::io::Error),
    /// Failed to read config metadata: {0}
    #[allow(dead_code)]
    ReadMeta(std::io::Error),
}

/// Genesis balance and optional code for a given address
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GenesisAddressBalance {
    /// The address that is to be preallocated a balance
    #[allow(dead_code)]
    pub(crate) address: String,
    /// The assigned address balance
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub(crate) balance: Option<String>,
    /// The account code (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub(crate) code: Option<String>,
}

/// Federation member public key and socket address
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FedMemberPubKey {
    /// The pub key of the member
    pub key: String,
    /// The socket address of the member
    pub socket_addr: String,
}

/// Configuration for the genesis block (toml)
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GenesisTomlConfig {
    /// Network name
    #[allow(dead_code)]
    pub name: String,
    /// federation members public keys
    pub federation_member_public_key: Vec<FedMemberPubKey>,
    /// genesis addresses initial account state
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub initial_account_state: Option<Vec<GenesisAddressBalance>>,
}

impl GenesisTomlConfig {
    #[allow(dead_code)]
    pub(crate) async fn new_from_path(path: impl AsRef<Path> + Send) -> Result<Self, Error> {
        read_to_string(path)?.parse()
    }

    /// Create a new genesis config
    pub fn new(
        name: String,
        federation_member_public_key: Vec<FedMemberPubKey>,
        initial_account_state: Option<Vec<GenesisAddressBalance>>,
    ) -> Self {
        Self { name, federation_member_public_key, initial_account_state }
    }
    /// Write the config to a file
    pub fn write_to_path(&self, path: impl AsRef<Path> + Send) -> Result<(), Error> {
        let toml = toml::to_string(self).map_err(Error::ParseSerializeConfig)?;
        let mut file = File::create(path).map_err(Error::OpenConfig)?;
        file.write_all(toml.as_bytes()).map_err(Error::ReadConfig)
    }
}

impl FromStr for GenesisTomlConfig {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s).map_err(Error::ParseConfig)
    }
}

#[allow(dead_code)]
fn read_to_string(path: impl AsRef<Path> + Send) -> Result<String, Error> {
    let mut file = File::open(path).map_err(Error::OpenConfig)?;
    let meta = file.metadata().map_err(Error::ReadMeta)?;
    let mut contents = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.read_to_end(&mut contents).map_err(Error::ReadConfig)?;
    String::from_utf8(contents).map_err(Error::ParseUtf8)
}
