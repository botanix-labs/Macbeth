use crate::constants::{BOTANIX_MAINNET, BOTANIX_TESTNET};
use alloy_genesis::Genesis;
use clap::{builder::TypedValueParser, error::Result, Arg, Command};
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use std::{ffi::OsStr, fs, path::PathBuf, sync::Arc};

/// Botanix chain specification parser.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BotanixChainSpecParser;

impl ChainSpecParser for BotanixChainSpecParser {
    const SUPPORTED_CHAINS: &'static [&'static str] = &["mainnet", "testnet"];

    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>> {
        chain_value_parser(s)
    }
}

/// Clap value parser for [`BotanixChainSpec`]s.
///
/// The value parser matches either a known chain, the path
/// to a json file, or a json formatted string in-memory. The json needs to be a Genesis struct.
fn chain_value_parser(s: &str) -> eyre::Result<Arc<ChainSpec>, eyre::Error> {
    match s {
        "mainnet" => Ok(BOTANIX_MAINNET.inner_arc()),
        "testnet" => Ok(BOTANIX_TESTNET.inner_arc()),
        _ => {
            // try to read json from path first
            let raw = match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
                Ok(raw) => raw,
                Err(io_err) => {
                    // valid json may start with "\n", but must contain "{"
                    if s.contains('{') {
                        s.to_string()
                    } else {
                        return Err(io_err.into()) // assume invalid path
                    }
                }
            };

            // both serialized Genesis and ChainSpec structs supported
            let genesis: Genesis = serde_json::from_str(&raw)?;

            Ok(Arc::new(genesis.into()))
        }
    }
}

impl TypedValueParser for BotanixChainSpecParser {
    type Value = Arc<ChainSpec>;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        <Self as ChainSpecParser>::parse(val).map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = Self::SUPPORTED_CHAINS.join(",");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(
        &self,
    ) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        let values = Self::SUPPORTED_CHAINS.iter().map(clap::builder::PossibleValue::new);
        Some(Box::new(values))
    }
}
