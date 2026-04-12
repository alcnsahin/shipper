mod commands;
mod config;
mod error;
mod platforms;
mod stores;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use console::style;

#[derive(Parser)]
#[command(
    name = "shipper",
    about = "Ship your mobile apps to stores from your Mac",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy your app to stores
    Deploy {
        #[command(subcommand)]
        platform: DeployTarget,
    },
    /// Initialize shipper in the current project
    Init,
}

#[derive(Subcommand, Clone)]
pub enum DeployTarget {
    /// Build and submit iOS app to TestFlight / App Store
    Ios,
    /// Build and submit Android app to Play Store
    Android,
    /// Deploy iOS and Android sequentially
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    utils::logger::init(cli.verbose);

    println!(
        "{} {}",
        style("shipper").bold().cyan(),
        style(env!("CARGO_PKG_VERSION")).dim()
    );
    println!();

    match cli.command {
        Commands::Deploy { platform } => {
            commands::deploy::run(platform).await?;
        }
        Commands::Init => {
            commands::init::run().await?;
        }
    }

    Ok(())
}
