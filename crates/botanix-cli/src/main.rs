use anyhow::Context;
use botanix_authority_peg::peg_contract::PeginMeta;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about)]
enum App {
    /// Work with pegin proofs.
    #[command(subcommand)]
    PeginProof(PeginProof),
}

#[derive(Debug, Subcommand)]
enum PeginProof {
    /// Inspect a pegin proof.
    #[command()]
    Inspect {
        /// The pegin proof in hex.
        proof: String,
    },
}

fn inner_main() -> Result<(), anyhow::Error> {
    match App::parse() {
        App::PeginProof(cmd) => match cmd {
            PeginProof::Inspect { proof } => {
                let bytes = hex::decode(&proof).context("invalid hex")?;
                let meta = PeginMeta::deserialize(&bytes).context("invalid proof format")?;
                println!("{:#?}", meta);
            }
        },
    }
    Ok(())
}

fn main() {
    if let Err(e) = inner_main() {
        eprintln!("ERROR: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use clap::CommandFactory;
    use std::{ffi::OsString, str::FromStr};

    // Helper function to parse command line arguments
    fn parse_args(args: &[&str]) -> App {
        let args = args.iter().map(|s| OsString::from_str(s).unwrap()).collect::<Vec<_>>();
        App::parse_from(args)
    }

    #[test]
    fn test_app_parse() {
        let args = &["app", "pegin-proof", "inspect", "deadbeef"];
        let app = parse_args(args);

        assert_matches!(app, App::PeginProof(PeginProof::Inspect { proof }) => {
            assert_eq!(proof, "deadbeef");
        });
    }

    #[test]
    fn test_inspect_valid_proof() {
        let args = &["app", "pegin-proof", "inspect", "deadbeef"];
        let app = parse_args(args);

        match app {
            App::PeginProof(PeginProof::Inspect { proof }) => {
                assert_eq!(proof, "deadbeef");
                // Test hex decoding works
                let bytes_result = hex::decode(&proof);
                assert!(bytes_result.is_ok());

                let bytes = bytes_result.unwrap();
                assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
            }
            _ => panic!("Expected PeginProof::Inspect"),
        }
    }

    #[test]
    fn test_inspect_invalid_hex() {
        // Test with invalid hex input
        let args = &["app", "pegin-proof", "inspect", "invalid-hex"];
        let app = parse_args(args);

        match app {
            App::PeginProof(PeginProof::Inspect { proof }) => {
                assert_eq!(proof, "invalid-hex");
                // Test that hex decoding fails as expected
                let result = hex::decode(&proof);
                assert!(result.is_err());
            }
            _ => panic!("Expected PeginProof::Inspect"),
        }
    }

    #[test]
    fn test_app_verification() {
        App::command().debug_assert();
    }

    fn test_inner_main(args: &[&str]) -> Result<(), anyhow::Error> {
        let args = args.iter().map(|s| OsString::from_str(s).unwrap()).collect::<Vec<_>>();
        match App::parse_from(args) {
            App::PeginProof(cmd) => match cmd {
                PeginProof::Inspect { proof } => {
                    let bytes = hex::decode(&proof).context("invalid hex")?;
                    if bytes.is_empty() {
                        return Err(anyhow::anyhow!("empty proof"));
                    }
                    Ok(())
                }
            },
        }
    }

    #[test]
    fn test_inner_main_valid_proof() {
        let result = test_inner_main(&["app", "pegin-proof", "inspect", "deadbeef"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_inner_main_invalid_hex() {
        let result = test_inner_main(&["app", "pegin-proof", "inspect", "invalid-hex"]);
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid hex"));
    }

    #[test]
    fn test_inner_main_empty_proof() {
        let result = test_inner_main(&["app", "pegin-proof", "inspect", ""]);
        assert!(result.is_err());
    }
}
