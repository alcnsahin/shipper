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

    print_banner();

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

fn print_banner() {
    let version = env!("CARGO_PKG_VERSION");

    // Stars
    println!(
        "  {}   {}   {}   {}",
        style("·").dim(),
        style("*").yellow().dim(),
        style("·").dim(),
        style("*").yellow().dim()
    );
    // Nose cone (7 spaces, centered over 5-wide body)
    println!(
        "       {}{}{}",
        style("╱").cyan(),
        style("▲").cyan().bold(),
        style("╲").cyan()
    );
    // Window — "APP" being shipped
    println!(
        "      {}{}{}  {} {}",
        style("│").cyan(),
        style("APP").cyan().bold(),
        style("│").cyan(),
        style("shipper").bold(),
        style(version).dim()
    );
    // Separator
    println!(
        "      {}───{}  {}",
        style("│").cyan(),
        style("│").cyan(),
        style("ship it.").dim()
    );
    // Base
    println!("      {}─┬─{}", style("╰").cyan(), style("╯").cyan());
    // Exhaust
    println!("        {}", style("│").dim());
    // Flames
    println!(
        "       {}{}{}",
        style("╱").yellow(),
        style("│").yellow(),
        style("╲").yellow()
    );
    // Sparks
    println!(
        "      {} {} {}",
        style("·").yellow().dim(),
        style("·").yellow().dim(),
        style("·").yellow().dim()
    );
    println!();
}
