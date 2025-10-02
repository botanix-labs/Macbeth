use btcserverlib::{
    database,
    util::parse_eth_address,
    wallet::address::{generate_taproot_address, generate_tweaked_public_key},
};
use clap::Parser;
use std::path::PathBuf;
use zeroize::Zeroizing;

/// The aggregated Frost public key of the genesis federation.
///
/// > GENESIS_AGGR_KEY =
/// > hex::decode("03ae26f6152efa6e65619f436aae5076356cacab97bed10c294a38b777efa66e72")
const GENESIS_AGGR_KEY: &[u8] = &[
    3, 174, 38, 246, 21, 46, 250, 110, 101, 97, 159, 67, 106, 174, 80, 118, 53, 108, 172, 171, 151,
    190, 209, 12, 41, 74, 56, 183, 119, 239, 166, 110, 114,
];

#[derive(Clone, Debug, Parser)]
#[command(name = "btc-utils")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Commands,
}

#[derive(Clone, Debug, Parser)]
pub enum Commands {
    /// Export encrypted key packages to a file.
    #[command(name = "export-key-package")]
    ExportKeyPackage(ExportConfig),
    /// Import encrypted key packages from a file.
    #[command(name = "import-key-package")]
    ImportKeyPackage(ImportConfig),
    #[command(name = "compute-gateway-address")]
    /// Compute the gateway pegin address for a given Botanix address.
    ComputeGatewayAddress(ComputeGatewayAddress),
}

#[derive(Clone, Debug, Parser)]
pub struct ExportConfig {
    /// The path to the database containing key packages.
    #[arg(long)]
    pub db: PathBuf,
    /// Output file path for the encrypted export.
    #[arg(long)]
    pub output: PathBuf,
    /// Passphrase as parameter instead of prompt.
    #[arg(long)]
    pub passphrase: Option<Zeroizing<String>>,
    /// Allow using an empty passphrase (NOT recommended).
    #[arg(long, default_value_t = false)]
    pub allow_empty_passphrase: bool,
}

#[derive(Clone, Debug, Parser)]
pub struct ImportConfig {
    /// The path to the database to store key packages.
    #[arg(long)]
    pub db: PathBuf,
    /// Input file path containing the encrypted export.
    #[arg(long)]
    pub input: PathBuf,
    /// Passphrase as parameter instead of prompt.
    #[arg(long)]
    pub passphrase: Option<Zeroizing<String>>,
    /// Overwrite existing key packages in the database.
    #[arg(long, default_value_t = false)]
    pub force_overwrite: bool,
}

#[derive(Clone, Debug, Parser)]
pub struct ComputeGatewayAddress {
    /// The Botanix (ETH) address to whom the BTC should be minted to.
    #[arg(long)]
    pub botanix_address: String,
    /// Custom aggregated public key of a multisig federation. Uses the genesis
    /// federation key on mainnet by default.
    #[arg(long)]
    pub aggregated_key: Option<String>,
}

impl ComputeGatewayAddress {
    fn compute_gateway_address(self) -> anyhow::Result<String, anyhow::Error> {
        let aggr_key = self
            .aggregated_key
            .map(|key| hex::decode(key))
            .transpose()?
            .unwrap_or(GENESIS_AGGR_KEY.to_vec());

        let aggr_key = frost_secp256k1_tr::VerifyingKey::deserialize(&aggr_key)?;

        let eth_address = parse_eth_address(self.botanix_address)?;
        let tweaked_key = generate_tweaked_public_key(&aggr_key, &eth_address).unwrap();
        let gateway_address = generate_taproot_address(&tweaked_key, bitcoin::Network::Bitcoin);

        Ok(gateway_address.to_string())
    }
}

fn get_passphrase(
    provided: Option<Zeroizing<String>>,
    do_confirm: bool,
) -> anyhow::Result<Zeroizing<String>> {
    match provided {
        Some(p) => Ok(p.trim().to_string().into()),
        None => {
            // Prompt user if CLI was not provided.
            let p1: Zeroizing<String> = rpassword::prompt_password("Passphrase: ")?.into();

            if do_confirm {
                let p2: Zeroizing<String> =
                    rpassword::prompt_password("Confirm passphrase: ")?.into();

                if p1 != p2 {
                    anyhow::bail!("Passphrases do not match!");
                }
            }

            // Trim leading and tailing whitespaces.
            Ok(p1.trim().to_string().into())
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<(), anyhow::Error> {
    let cli = Cli::parse();

    match cli.cmd {
        Commands::ExportKeyPackage(c) => {
            // NOTE: This creates a new database if it does not already exist...
            let db = database::Db::open(&c.db).expect("failed to open db");

            // Retrieve the passphrase.
            let do_confirm = true;
            let passphrase = get_passphrase(c.passphrase, do_confirm)?;

            // Passphrase must not be empty, unless explicitly allowed.
            if passphrase.is_empty() && !c.allow_empty_passphrase {
                anyhow::bail!("Passphrase may not be empty unless explicitly allowed");
            }

            // Create the encrypted export.
            let Some(export) = db.export_key_package(passphrase)? else {
                anyhow::bail!("No key package found - correct database path?");
            };

            // Serialize to bytes and write to file.
            let mut bytes = Vec::new();
            ciborium::into_writer(&export, &mut bytes)?;
            std::fs::write(&c.output, &bytes)?;

            println!("Successfully exported encrypted key package to '{}'", c.output.display());
        }
        Commands::ImportKeyPackage(c) => {
            // NOTE: This creates a new database if it does not already exist...
            let db = database::Db::open(&c.db).expect("failed to open db");

            if db.get_key_package()?.is_some() && !c.force_overwrite {
                anyhow::bail!(
                    "Existing key package may not be overwritten unless explicitly allowed"
                )
            }

            // Retrieve the passphrase.
            let do_confirm = false;
            let passphrase = get_passphrase(c.passphrase, do_confirm)?;

            // We don't bother checking whether the passphrase is empty here or
            // not; decryption (failure) handles that for us implicitly.

            // Read the import, attempt to decrypt and save the key package to
            // the database.
            let bytes = std::fs::read(&c.input)?;
            let import: database::ExportedKeyPackage = ciborium::from_reader(bytes.as_slice())?;
            db.import_key_package(passphrase, import)?;

            println!("Successfully imported decrypted key package");
        }
        Commands::ComputeGatewayAddress(c) => {
            let gateway_address = c.compute_gateway_address()?;
            println!("{}", gateway_address.to_string());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btc_utils_test_genesis_aggr_key() {
        let raw =
            hex::decode(b"03ae26f6152efa6e65619f436aae5076356cacab97bed10c294a38b777efa66e72")
                .unwrap();

        assert_eq!(raw, GENESIS_AGGR_KEY);
    }

    #[test]
    fn btc_utils_compute_gateway_address() {
        const MATCH: &str = "bc1pdxrrvdqunlrwnt4qrqs52djk0g4r39s0f5qsa0j9dcctq6jvaf5qu9na42";

        let c = ComputeGatewayAddress {
            botanix_address: "0xE99F129Bb9d60a91f6d0Ae2d6bBC746C52A87220".to_string(),
            aggregated_key: Some(hex::encode(GENESIS_AGGR_KEY)),
        };

        let addr = c.compute_gateway_address().unwrap();
        assert_eq!(&addr, &MATCH);

        // Uses the `GENESIS_AGGR_KEY` by default.
        let c = ComputeGatewayAddress {
            botanix_address: "0xE99F129Bb9d60a91f6d0Ae2d6bBC746C52A87220".to_string(),
            aggregated_key: None,
        };

        let addr = c.compute_gateway_address().unwrap();
        assert_eq!(&addr, MATCH);
    }

    #[test]
    fn btc_utils_compute_gateway_address_bad_aggr_key() {
        let c = ComputeGatewayAddress {
            botanix_address: "0xE99F129Bb9d60a91f6d0Ae2d6bBC746C52A87220".to_string(),
            aggregated_key: Some("zzz".to_string()),
        };

        // Bad aggregated key.
        let _err = c.compute_gateway_address().unwrap_err();
    }
}
