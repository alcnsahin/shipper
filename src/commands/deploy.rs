use anyhow::Result;
use console::style;
use std::path::Path;
use std::time::Instant;
use tracing::{error, info};

use crate::config::{Config, GlobalConfig};
use crate::platforms::{android, ios};
use crate::utils::credentials::migrate_stray_credentials;
use crate::utils::lock::DeployLock;
use crate::utils::notifier::{notify, DeployResult};
use crate::utils::version::{self, AppVersion};
use crate::DeployTarget;

/// Show what a deploy would do without executing anything.
pub fn dry_run(target: DeployTarget, global: GlobalConfig) -> Result<()> {
    let config = Config::with_global(global)?;
    let app_name = &config.project.project.name;

    println!(
        "  {} Dry run for {}",
        style("▸").cyan().bold(),
        style(app_name).bold()
    );
    println!();

    let platforms: Vec<&str> = match target {
        DeployTarget::Ios => vec!["ios"],
        DeployTarget::Android => vec!["android"],
        DeployTarget::All => vec!["ios", "android"],
    };

    for platform in &platforms {
        match *platform {
            "ios" => {
                let ios = config.ios_config()?;
                let _apple = config.apple_credentials()?;
                let version = if version::is_expo_project() {
                    version::read_expo_version(std::path::Path::new("app.json"))?
                } else {
                    let ios_dir = ios
                        .workspace
                        .as_deref()
                        .and_then(|w| std::path::Path::new(w).parent())
                        .unwrap_or(std::path::Path::new("ios"));
                    find_and_read_ios_version(ios_dir)?
                };
                println!("  iOS:");
                println!("    App:       {}", ios.bundle_id);
                println!(
                    "    Version:   {} ({}) → next build: {}",
                    version.version_name,
                    version.build_number,
                    version.build_number + 1
                );
                println!("    Scheme:    {}", ios.scheme);
                println!("    Export:    {}", ios.export_method);
                println!("    Upload:    xcrun altool → App Store Connect");
                if ios.asc_app_id.is_some() {
                    println!("    Poll:      yes (asc_app_id configured)");
                }
                if !ios.testflight_groups.is_empty() {
                    println!("    TestFlight: {}", ios.testflight_groups.join(", "));
                }
                println!();
            }
            "android" => {
                let android = config.android_config()?;
                let _google = config.google_credentials()?;
                let version = if version::is_expo_project() {
                    version::read_expo_version_android(std::path::Path::new("app.json"))?
                } else {
                    let gradle =
                        std::path::Path::new(&android.project_dir).join("app/build.gradle");
                    version::read_gradle_version(&gradle)?
                };
                println!("  Android:");
                println!("    Package:   {}", android.package_name);
                println!(
                    "    Version:   {} ({}) → next build: {}",
                    version.version_name,
                    version.build_number,
                    version.build_number + 1
                );
                println!("    Build:     {}", android.build_type);
                println!("    Track:     {}", android.track);
                if let Some(fraction) = android.rollout_fraction {
                    if android.track == "production" && fraction < 1.0 {
                        println!("    Rollout:   {:.0}% staged", fraction * 100.0);
                    }
                }
                println!("    Upload:    Play Store Developer API v3");
                println!();
            }
            _ => {}
        }
    }

    println!("  {} No changes made (dry run)", style("i").dim());

    Ok(())
}

/// Find and read iOS version from Info.plist in the given directory.
fn find_and_read_ios_version(ios_dir: &std::path::Path) -> Result<AppVersion> {
    // Check common locations
    for candidate in &[
        ios_dir.join("Info.plist"),
        ios_dir.join("Resources/Info.plist"),
    ] {
        if candidate.exists() {
            return version::read_info_plist_version(candidate);
        }
    }
    // Walk one level deep
    if let Ok(entries) = std::fs::read_dir(ios_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join("Info.plist");
                if candidate.exists() {
                    return version::read_info_plist_version(&candidate);
                }
            }
        }
    }
    anyhow::bail!("Could not find Info.plist for version info")
}

pub async fn run(target: DeployTarget, global: GlobalConfig) -> Result<()> {
    let config = Config::with_global(global)?;

    // Prevent concurrent deploys for the same project.
    let _lock = DeployLock::acquire(&config.project.project.name)?;

    // Move any stray *.keystore / credentials.json files from the project root
    // into ~/.shipper/keys/{project_name}/ before the build starts.
    // Pass the configured keystore_path so migration can update shipper.toml if needed.
    let android_ks_path = config
        .project
        .android
        .as_ref()
        .map(|a| a.keystore_path.as_str());
    migrate_stray_credentials(&config.project.project.name, android_ks_path)?;

    match target {
        DeployTarget::Ios => deploy_ios(&config, None).await,
        DeployTarget::Android => deploy_android(&config, None).await,
        DeployTarget::All => deploy_all(&config).await,
    }
}

/// Bump versions for both platforms, then run iOS and Android in parallel.
async fn deploy_all(config: &Config) -> Result<()> {
    // Bump versions sequentially to avoid race on shared app.json (Expo)
    // or just to have clean version state before parallel builds.
    let auto_increment = config
        .project
        .versioning
        .as_ref()
        .map(|v| v.auto_increment)
        .unwrap_or(true);

    let (ios_version, android_version) = if version::is_expo_project() {
        let app_json = Path::new("app.json");
        let mut iv = version::read_expo_version(app_json)?;
        let mut av = version::read_expo_version_android(app_json)?;
        if auto_increment {
            iv.bump_build();
            av.bump_build();
        }
        // Write both to app.json sequentially — no race.
        version::write_expo_version_ios(app_json, &iv)?;
        version::write_expo_version_android(app_json, &av)?;
        (Some(iv), Some(av))
    } else {
        // Native projects: each platform bumps its own file (Info.plist / build.gradle).
        // No shared state, so let each deploy handle its own bump.
        (None, None)
    };

    println!(
        "  {} Running iOS and Android pipelines in parallel",
        style("⇉").cyan().bold()
    );
    println!();

    let (ios_result, android_result) = tokio::join!(
        deploy_ios(config, ios_version),
        deploy_android(config, android_version),
    );

    // Report both results. If either failed, return an error.
    match (&ios_result, &android_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(_), Ok(())) => ios_result,
        (Ok(()), Err(_)) => android_result,
        (Err(ie), Err(ae)) => {
            anyhow::bail!("Both platforms failed:\n  iOS: {ie}\n  Android: {ae}")
        }
    }
}

#[tracing::instrument(
    name = "deploy",
    skip_all,
    fields(platform = "ios", app = %config.project.project.name),
)]
async fn deploy_ios(config: &Config, pre_bumped: Option<AppVersion>) -> Result<()> {
    let app_name = &config.project.project.name;
    let started = Instant::now();

    match ios::deploy(config, pre_bumped).await {
        Ok(version) => {
            let elapsed = started.elapsed();
            info!(
                elapsed_ms = elapsed.as_millis() as u64,
                version = %version.version_name,
                build = version.build_number,
                "deploy succeeded"
            );

            println!();
            print_summary(
                app_name,
                "iOS",
                &version.version_name,
                version.build_number,
                "TestFlight",
                elapsed,
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
            error!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %e,
                "deploy failed"
            );

            println!();
            println!("  {} iOS deploy failed: {}", style("✗").bold().red(), e);

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

#[tracing::instrument(
    name = "deploy",
    skip_all,
    fields(platform = "android", app = %config.project.project.name),
)]
async fn deploy_android(config: &Config, pre_bumped: Option<AppVersion>) -> Result<()> {
    let app_name = &config.project.project.name;
    let track = config
        .project
        .android
        .as_ref()
        .map(|a| a.track.clone())
        .unwrap_or_else(|| "internal".to_string());
    let started = Instant::now();

    match android::deploy(config, pre_bumped).await {
        Ok(version) => {
            let elapsed = started.elapsed();
            info!(
                elapsed_ms = elapsed.as_millis() as u64,
                version = %version.version_name,
                build = version.build_number,
                track = %track,
                "deploy succeeded"
            );

            println!();
            let rollout_fraction = config
                .project
                .android
                .as_ref()
                .and_then(|a| a.rollout_fraction);
            let destination = match rollout_fraction {
                Some(f) if f < 1.0 && track == "production" => {
                    format!("Play Store ({}, {:.0}% staged rollout)", track, f * 100.0)
                }
                _ => format!("Play Store ({})", track),
            };
            print_summary(
                app_name,
                "Android",
                &version.version_name,
                version.build_number,
                &destination,
                elapsed,
            );
            println!();

            let result = DeployResult {
                app_name: app_name.clone(),
                platform: "android".to_string(),
                version: version.version_name,
                build_number: version.build_number.to_string(),
                destination,
                success: true,
                error: None,
            };
            notify(config, &result).await.ok();
            Ok(())
        }
        Err(e) => {
            error!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                track = %track,
                error = %e,
                "deploy failed"
            );

            println!();
            println!("  {} Android deploy failed: {}", style("✗").bold().red(), e);

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

// ─── Summary ─────────────────────────────────────────────────────────────────

fn print_summary(
    app: &str,
    platform: &str,
    version: &str,
    build: u32,
    destination: &str,
    elapsed: std::time::Duration,
) {
    println!(
        "  {} {} deployed successfully",
        style("✓").bold().green(),
        style(app).bold()
    );
    println!("  ┌──────────────┬─────────────────────────────────");
    println!("  │ {} │ {}", style("Platform     ").dim(), platform);
    println!(
        "  │ {} │ {} ({})",
        style("Version      ").dim(),
        version,
        build
    );
    println!("  │ {} │ {}", style("Destination  ").dim(), destination);
    println!(
        "  │ {} │ {}",
        style("Elapsed      ").dim(),
        format_duration(elapsed)
    );
    println!("  └──────────────┴─────────────────────────────────");
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}
