use displaydoc::Display as DisplayDoc;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
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
    /// Failed to read public key: {0}
    #[allow(dead_code)]
    InvalidPublicKeyFormat(#[from] secp256k1::Error),
    /// Failed to read config socket address: {0}
    #[allow(dead_code)]
    InvalidSocketAddress(#[from] std::net::AddrParseError),
    /// Failed to resolve socket address: {0}
    #[allow(dead_code)]
    SocketAddressResolution(std::io::Error),
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
pub struct FederationTomlConfig {
    /// federation members public keys
    pub federation_member_public_key: Vec<FedMemberPubKey>,
    /// botanix fee recipient
    pub botanix_fee_recipient: String,
    /// The precompiled Minting contract bytecode
    pub minting_contract_bytecode: String,
    /// LST fee receiver
    pub lst_fee_receiver: String,
}

impl FederationTomlConfig {
    #[allow(dead_code)]
    pub(crate) async fn new_from_path(path: impl AsRef<Path> + Send) -> Result<Self, Error> {
        read_to_string(path)?.parse()
    }

    /// Create a new genesis config
    pub const fn new(
        federation_member_public_key: Vec<FedMemberPubKey>,
        botanix_fee_recipient: String,
        minting_contract_bytecode: String,
        lst_fee_receiver: String,
    ) -> Self {
        Self {
            federation_member_public_key,
            botanix_fee_recipient,
            minting_contract_bytecode,
            lst_fee_receiver,
        }
    }
    /// Write the config to a file
    pub fn write_to_path(&self, path: impl AsRef<Path> + Send) -> Result<(), Error> {
        let toml = toml::to_string(self).map_err(Error::ParseSerializeConfig)?;
        let mut file = File::create(path).map_err(Error::OpenConfig)?;
        file.write_all(toml.as_bytes()).map_err(Error::ReadConfig)
    }

    /// Convert the config to a string
    pub fn to_string(&self) -> Result<String, Error> {
        toml::to_string(self).map_err(Error::ParseSerializeConfig)
    }

    /// Extracts federation public keys and socket addresses from the config
    pub fn get_federation_pks_from_path(
        &self,
    ) -> Result<Vec<(secp256k1::PublicKey, SocketAddr)>, Error> {
        let federation_members = self
            .federation_member_public_key
            .iter()
            .map(|key| {
                let public_key = secp256k1::PublicKey::from_str(&key.key).map_err(Error::from)?;

                let soc_addr = match key.socket_addr.parse::<SocketAddr>() {
                    Ok(addr) => addr,
                    Err(_) => key
                        .socket_addr
                        .to_socket_addrs()
                        .map_err(Error::SocketAddressResolution)?
                        .next()
                        .ok_or_else(|| {
                            Error::SocketAddressResolution(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("No addresses resolved for {}", key.socket_addr),
                            ))
                        })?,
                };

                Ok((public_key, soc_addr))
            })
            .collect::<Result<Vec<(secp256k1::PublicKey, SocketAddr)>, Error>>()?;

        Ok(federation_members)
    }
}
impl FromStr for FederationTomlConfig {
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

/// Writes random bytes to a filepath
#[allow(dead_code)]
pub(crate) fn write_data_to_file(path: impl AsRef<Path> + Send, data: &[u8]) -> Result<(), Error> {
    let mut file = File::create(path).map_err(Error::OpenConfig)?;
    file.write_all(data).map_err(Error::ReadConfig)
}
