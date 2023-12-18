
use anyhow::Context;
use clap::{Parser, Subcommand};

use botanix_lib::peg_contract::PeginMeta;


#[derive(Parser)]
#[command(version, about)]
enum App {
    /// Work with pegin proofs.
    #[command(subcommand)]
    PeginProof(PeginProof),
}


#[derive(Subcommand)]
enum PeginProof {
    /// Inspect a pegin proof.
    #[command()]
    Inspect {
        /// The pegin proof in hex.
        proof: String,
    }
}

fn inner_main() -> Result<(), anyhow::Error> {
    match App::parse() {
        App::PeginProof(cmd) => match cmd {
            PeginProof::Inspect { proof } => {
                let bytes = hex::decode(&proof).context("invalid hex")?;
                let meta = PeginMeta::deserialize(&bytes).context("invalid proof format")?;
                println!("{:#?}", meta);
            }
        }
    }
    Ok(())
}

fn main() {
	if let Err(e) = inner_main() {
		eprintln!("ERROR: {}", e);
	}
}
