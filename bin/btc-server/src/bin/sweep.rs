use clap::Parser;

#[derive(Clone, Debug, Parser)]
#[command(name = "sweep")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Commands,
}

#[derive(Clone, Debug, Parser)]
pub enum Commands {
    #[command(name = "make-sweep-psbt")]
    MakeSweepPsbt(MakeSweepPsbtConfig),
    #[command(name = "frost-round-1")]
    FrostRound1(FrostRound1Config),
    #[command(name = "frost-round-2")]
    FrostRound2(FrostRound2Config),
}

#[derive(Clone, Debug, Parser)]
pub struct MakeSweepPsbtConfig {}

#[derive(Clone, Debug, Parser)]
pub struct FrostRound1Config {}

#[derive(Clone, Debug, Parser)]
pub struct FrostRound2Config {}

#[tokio::main]
async fn main() -> anyhow::Result<(), anyhow::Error> {
    let cli = Cli::parse();

    match cli.cmd {
        Commands::MakeSweepPsbt(c) => {
            println!("make sweep psbt");
        }
        Commands::FrostRound1(c) => {
            println!("frost round 1");
        }
        Commands::FrostRound2(c) => {
            println!("frost round 2");
        }
    }

    Ok(())
}
