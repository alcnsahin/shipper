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

        /// Show what would happen without actually building or uploading
        #[arg(long, global = true)]
        dry_run: bool,
    },
    /// Initialize shipper in the current project
    Init,
    /// Validate shipper.toml and ~/.shipper/config.toml
    Validate,
}

#[derive(Subcommand, Clone)]
pub enum DeployTarget {
    /// Build and submit iOS app to TestFlight / App Store
    Ios,
    /// Build and submit Android app to Play Store
    Android,
    /// Deploy iOS and Android in parallel
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load the global config before the logger so `global.log_level` can
    // set the default filter. Project config (shipper.toml) is loaded later
    // inside each subcommand and may legitimately be absent (e.g. `init`).
    let global = config::load_global_or_default();
    utils::logger::init(cli.verbose, &global.global.log_level);

    print_banner();

    match cli.command {
        Commands::Deploy { platform, dry_run } => {
            if dry_run {
                commands::deploy::dry_run(platform, global)?;
            } else {
                commands::deploy::run(platform, global).await?;
            }
        }
        Commands::Init => {
            commands::init::run().await?;
        }
        Commands::Validate => {
            commands::validate::run()?;
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
