use anyhow::Result;
use console::style;

use crate::config::Config;
use crate::platforms::{android, ios};
use crate::utils::notifier::{notify, DeployResult};
use crate::DeployTarget;

pub async fn run(target: DeployTarget) -> Result<()> {
    let config = Config::load()?;

    match target {
        DeployTarget::Ios => deploy_ios(&config).await,
        DeployTarget::Android => deploy_android(&config).await,
        DeployTarget::All => {
            deploy_ios(&config).await?;
            println!();
            deploy_android(&config).await
        }
    }
}

async fn deploy_ios(config: &Config) -> Result<()> {
    let app_name = &config.project.project.name;

    match ios::deploy(config).await {
        Ok(version) => {
            println!();
            println!(
                "  {} {} v{} ({}) → TestFlight",
                style("✓").bold().green(),
                style(app_name).bold(),
                version.version_name,
                version.build_number
            );
            println!();

            let result = DeployResult {
                app_name: app_name.clone(),
                platform: "ios".to_string(),
                version: version.version_name,
                build_number: version.build_number.to_string(),
                destination: "TestFlight".to_string(),
                success: true,
                error: None,
            };
            notify(config, &result).await.ok(); // Non-fatal
            Ok(())
        }
        Err(e) => {
            println!();
            println!(
                "  {} iOS deploy failed: {}",
                style("✗").bold().red(),
                e
            );

            let result = DeployResult {
                app_name: app_name.clone(),
                platform: "ios".to_string(),
                version: String::new(),
                build_number: String::new(),
                destination: "TestFlight".to_string(),
                success: false,
                error: Some(e.to_string()),
            };
            notify(config, &result).await.ok();
            Err(e)
        }
    }
}

async fn deploy_android(config: &Config) -> Result<()> {
    let app_name = &config.project.project.name;
    let track = config
        .project
        .android
        .as_ref()
        .map(|a| a.track.clone())
        .unwrap_or_else(|| "internal".to_string());

    match android::deploy(config).await {
        Ok(version) => {
            println!();
            println!(
                "  {} {} v{} ({}) → {} track",
                style("✓").bold().green(),
                style(app_name).bold(),
                version.version_name,
                version.build_number,
                track
            );
            println!();

            let result = DeployResult {
                app_name: app_name.clone(),
                platform: "android".to_string(),
                version: version.version_name,
                build_number: version.build_number.to_string(),
                destination: format!("Play Store ({})", track),
                success: true,
                error: None,
            };
            notify(config, &result).await.ok();
            Ok(())
        }
        Err(e) => {
            println!();
            println!(
                "  {} Android deploy failed: {}",
                style("✗").bold().red(),
                e
            );

            let result = DeployResult {
                app_name: app_name.clone(),
                platform: "android".to_string(),
                version: String::new(),
                build_number: String::new(),
                destination: format!("Play Store ({})", track),
                success: false,
                error: Some(e.to_string()),
            };
            notify(config, &result).await.ok();
            Err(e)
        }
    }
}
