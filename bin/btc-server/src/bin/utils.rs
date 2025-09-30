use btcserverlib::database;
use clap::Parser;
use std::path::PathBuf;
use zeroize::Zeroizing;

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
    }

    Ok(())
}
