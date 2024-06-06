use displaydoc::Display as DisplayDoc;
use serde::Deserialize;
use std::{fs::File, io::Read, path::Path, str::FromStr};
use thiserror::Error;

#[derive(Debug, DisplayDoc, Error)]
pub(crate) enum Error {
    /// Open config file: {0}
    #[allow(dead_code)]
    OpenConfig(std::io::Error),
    /// Failed to parse config: {0}
    ParseConfig(toml::de::Error),
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct GenesisAddressBalance {
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct FedMemberPubKey {
    /// The pub key of the member
    pub(crate) key: String,
    /// The socket address of the member
    pub(crate) socket_addr: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct GenesisTomlConfig {
    /// Network name
    #[allow(dead_code)]
    pub(crate) name: String,
    /// federation members public keys
    pub(crate) federation_member_public_key: Vec<FedMemberPubKey>,
    /// genesis addresses initial account state
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub(crate) initial_account_state: Option<Vec<GenesisAddressBalance>>,
}

impl GenesisTomlConfig {
    #[allow(dead_code)]
    pub(crate) async fn new(path: impl AsRef<Path> + Send) -> Result<Self, Error> {
        read_to_string(path)?.parse()
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
