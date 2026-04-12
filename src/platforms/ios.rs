use anyhow::{Context, Result};
use console::style;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::config::{AppleCredentials, Config, IosConfig};
use crate::stores::appstore;
use crate::utils::progress;
use crate::utils::version::{self, AppVersion};

const TOTAL_STEPS_WITH_POLL: usize = 7;
const TOTAL_STEPS_NO_POLL: usize = 6;

pub async fn deploy(config: &Config) -> Result<AppVersion> {
    let ios = config.ios_config()?;
    let apple = config.apple_credentials()?;

    preflight_checks(ios)?;

    let total = if ios.asc_app_id.is_some() {
        TOTAL_STEPS_WITH_POLL
    } else {
        TOTAL_STEPS_NO_POLL
    };

    println!("{}", style("iOS Pipeline").bold().underlined());
    println!();

    // Step 1: Version bump
    progress::step(1, total, "Bumping version");
    let app_version = bump_version(config, ios)?;
    progress::success(&format!(
        "{} ({})",
        app_version.version_name, app_version.build_number
    ));

    // Step 2: Expo prebuild (if applicable)
    if version::is_expo_project() {
        progress::step(2, total, "Running expo prebuild");
        expo_prebuild().await?;
        progress::success("Expo prebuild complete");
    } else {
        progress::step(2, total, "Expo prebuild — skipped (not an Expo project)");
    }

    // Step 3: Pod install
    let ios_dir = resolve_ios_dir(ios);
    if ios_dir.join("Podfile").exists() {
        progress::step(3, total, "Installing CocoaPods");
        pod_install(&ios_dir).await?;
        progress::success("Pods installed");
    } else {
        progress::step(3, total, "CocoaPods — skipped (no Podfile)");
    }

    // After prebuild, resolve the real workspace path and scheme name from the filesystem.
    // shipper.toml may have wrong casing (e.g. "cyberchan" instead of "CyberChan").
    let (resolved_workspace, resolved_scheme) = resolve_build_config(ios);

    if resolved_workspace.as_deref() != ios.workspace.as_deref() {
        println!(
            "  {} Workspace auto-corrected: {} → {}",
            style("i").dim(),
            ios.workspace.as_deref().unwrap_or("(none)"),
            resolved_workspace.as_deref().unwrap_or("(none)")
        );
    }
    if resolved_scheme != ios.scheme {
        println!(
            "  {} Scheme auto-corrected: {} → {}",
            style("i").dim(),
            ios.scheme,
            resolved_scheme
        );
    }

    // Step 4: Archive
    progress::step(4, total, "Archiving with xcodebuild");
    let archive_path = archive(ios, resolved_workspace.as_deref(), &resolved_scheme, &app_version).await?;
    progress::success(&format!("Archive created: {}", archive_path.display()));

    // Step 5: Export IPA
    progress::step(5, total, "Exporting IPA");
    let ipa_path = export_ipa(ios, &archive_path).await?;
    progress::success(&format!("IPA: {}", ipa_path.display()));

    // Step 6: Upload to App Store Connect
    progress::step(6, total, "Uploading to App Store Connect");
    upload_to_asc(ios, apple, &ipa_path).await?;
    progress::success("Upload complete");

    // Step 7: Poll processing (only if asc_app_id is set)
    if let Some(asc_app_id) = &ios.asc_app_id {
        progress::step(7, total, "Waiting for App Store Connect processing");
        let build_id = appstore::poll_build_processing(
            apple,
            asc_app_id,
            &app_version.version_name,
            &app_version.build_number.to_string(),
        )
        .await?;
        progress::success(&format!("Build processed (id: {})", build_id));
    } else {
        println!(
            "  {} Build polling skipped — add asc_app_id to shipper.toml after creating the app in App Store Connect",
            style("i").dim()
        );
    }

    Ok(app_version)
}

// ─── Preflight ────────────────────────────────────────────────────────────────

fn preflight_checks(ios: &IosConfig) -> Result<()> {
    let spinner = progress::spinner("Running preflight checks...");

    check_tool("xcodebuild")?;
    check_tool("xcrun")?;

    // Verify workspace/project is configured
    if ios.workspace.is_none() && ios.project.is_none() {
        anyhow::bail!("Either [ios].workspace or [ios].project must be set in shipper.toml");
    }

    // For Expo projects, ios/ is created by expo prebuild — skip existence check here.
    // For native projects, the workspace/project must already exist.
    if !version::is_expo_project() {
        if let Some(ws) = &ios.workspace {
            if !Path::new(ws).exists() {
                anyhow::bail!("Workspace not found: {}", ws);
            }
        } else if let Some(proj) = &ios.project {
            if !Path::new(proj).exists() {
                anyhow::bail!("Project not found: {}", proj);
            }
        }
    }

    spinner.finish_and_clear();
    Ok(())
}

fn check_tool(name: &str) -> Result<()> {
    which::which(name)
        .with_context(|| format!("'{}' not found. Install Xcode and try again.", name))?;
    Ok(())
}

// ─── Version ─────────────────────────────────────────────────────────────────

fn bump_version(config: &Config, ios: &IosConfig) -> Result<AppVersion> {
    let auto_increment = config
        .project
        .versioning
        .as_ref()
        .map(|v| v.auto_increment)
        .unwrap_or(true);

    if version::is_expo_project() {
        let app_json = Path::new("app.json");
        let mut v = version::read_expo_version(app_json)?;
        if auto_increment {
            v.bump_build();
        }
        version::write_expo_version_ios(app_json, &v)?;
        return Ok(v);
    }

    // Find Info.plist in the ios/ directory
    let ios_dir = resolve_ios_dir(ios);
    let plist_path = find_info_plist(&ios_dir)?;
    let mut v = version::read_info_plist_version(&plist_path)?;
    if auto_increment {
        v.bump_build();
    }
    version::write_info_plist_version(&plist_path, &v)?;
    Ok(v)
}

fn find_info_plist(ios_dir: &Path) -> Result<PathBuf> {
    // Common locations
    let candidates = [
        ios_dir.join("Info.plist"),
        ios_dir.join("Resources/Info.plist"),
    ];

    for p in &candidates {
        if p.exists() {
            return Ok(p.clone());
        }
    }

    // Walk one level deep
    if let Ok(entries) = std::fs::read_dir(ios_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join("Info.plist");
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    anyhow::bail!(
        "Could not find Info.plist in {}. Set it explicitly in shipper.toml.",
        ios_dir.display()
    )
}

fn resolve_ios_dir(ios: &IosConfig) -> PathBuf {
    if let Some(ws) = &ios.workspace {
        Path::new(ws)
            .parent()
            .unwrap_or(Path::new("ios"))
            .to_path_buf()
    } else if let Some(proj) = &ios.project {
        Path::new(proj)
            .parent()
            .unwrap_or(Path::new("ios"))
            .to_path_buf()
    } else {
        PathBuf::from("ios")
    }
}

// ─── Expo prebuild ────────────────────────────────────────────────────────────

async fn expo_prebuild() -> Result<()> {
    run_command(
        "npx",
        &["expo", "prebuild", "--platform", "ios", "--clean"],
        "Expo prebuild failed",
    )
    .await
}

// ─── CocoaPods ────────────────────────────────────────────────────────────────

async fn pod_install(ios_dir: &Path) -> Result<()> {
    let spinner = progress::spinner("pod install --repo-update ...");

    let status = tokio::process::Command::new("pod")
        .args(["install", "--repo-update"])
        .current_dir(ios_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .context("Failed to run 'pod install'")?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("pod install failed with exit code {:?}", status.code());
    }

    Ok(())
}

// ─── xcodebuild archive ───────────────────────────────────────────────────────

/// Scan the actual ios/ directory for the workspace and scheme after prebuild.
/// Returns (resolved_workspace, resolved_scheme) — corrects casing issues in shipper.toml.
fn resolve_build_config(ios: &IosConfig) -> (Option<String>, String) {
    // Find real workspace by scanning ios/ (handles case mismatches on macOS)
    let actual_workspace = scan_for_xcworkspace().or_else(|| ios.workspace.clone());

    let scheme = match &actual_workspace {
        Some(ws) => find_scheme_in_workspace(ws, &ios.scheme).unwrap_or_else(|| ios.scheme.clone()),
        None => ios.scheme.clone(),
    };

    (actual_workspace, scheme)
}

fn scan_for_xcworkspace() -> Option<String> {
    for entry in std::fs::read_dir("ios").ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("xcworkspace") {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

fn find_scheme_in_workspace(workspace: &str, configured_scheme: &str) -> Option<String> {
    let schemes_dir = PathBuf::from(workspace).join("xcshareddata/xcschemes");
    let schemes: Vec<String> = std::fs::read_dir(&schemes_dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("xcscheme") {
                p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
            } else {
                None
            }
        })
        .collect();

    // Exact match — already correct, no change needed
    if schemes.iter().any(|s| s == configured_scheme) {
        return None;
    }
    // Case-insensitive match
    let lower = configured_scheme.to_lowercase();
    if let Some(m) = schemes.iter().find(|s| s.to_lowercase() == lower) {
        return Some(m.clone());
    }
    // Single scheme fallback
    if schemes.len() == 1 {
        return Some(schemes[0].clone());
    }
    None
}

async fn archive(ios: &IosConfig, workspace: Option<&str>, scheme: &str, version: &AppVersion) -> Result<PathBuf> {
    let archive_path = PathBuf::from(&ios.build_dir)
        .join(format!("{}.xcarchive", scheme));

    std::fs::create_dir_all(&ios.build_dir)
        .context("Failed to create build directory")?;

    let mut args = vec![
        "archive".to_string(),
        "-configuration".to_string(),
        ios.configuration.clone(),
        "-scheme".to_string(),
        scheme.to_string(),
        "-archivePath".to_string(),
        archive_path.to_string_lossy().to_string(),
        "-destination".to_string(),
        "generic/platform=iOS".to_string(),
        "CODE_SIGN_STYLE=Manual".to_string(),
    ];

    if let Some(ws) = workspace {
        args.insert(1, ws.to_string());
        args.insert(1, "-workspace".to_string());
    } else if let Some(proj) = &ios.project {
        args.insert(1, proj.clone());
        args.insert(1, "-project".to_string());
    }

    if let Some(profile) = &ios.provisioning_profile {
        args.push(format!("PROVISIONING_PROFILE_SPECIFIER={}", profile));
    }

    if let Some(identity) = &ios.code_sign_identity {
        args.push(format!("CODE_SIGN_IDENTITY={}", identity));
    }

    // Inject version numbers
    args.push(format!("CURRENT_PROJECT_VERSION={}", version.build_number));
    args.push(format!("MARKETING_VERSION={}", version.version_name));

    let spinner = progress::spinner("xcodebuild archive (this can take a few minutes)...");

    let output = Command::new("xcodebuild")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run xcodebuild archive")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Extract the error lines for a cleaner message
        let errors: Vec<&str> = stderr
            .lines()
            .filter(|l| l.contains("error:"))
            .take(10)
            .collect();
        if errors.is_empty() {
            anyhow::bail!("xcodebuild archive failed:\n{}", stderr.trim());
        } else {
            anyhow::bail!("xcodebuild archive failed:\n{}", errors.join("\n"));
        }
    }

    Ok(archive_path)
}

// ─── Export IPA ───────────────────────────────────────────────────────────────

async fn export_ipa(ios: &IosConfig, archive_path: &Path) -> Result<PathBuf> {
    let export_path = PathBuf::from(&ios.build_dir).join("ipa");

    let export_plist = generate_export_plist(ios);
    let plist_path = PathBuf::from(&ios.build_dir).join("ExportOptions.plist");
    std::fs::write(&plist_path, &export_plist)
        .context("Failed to write ExportOptions.plist")?;

    let spinner = progress::spinner("Exporting IPA...");

    let output = Command::new("xcodebuild")
        .args([
            "-exportArchive",
            "-archivePath",
            &archive_path.to_string_lossy(),
            "-exportPath",
            &export_path.to_string_lossy(),
            "-exportOptionsPlist",
            &plist_path.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run xcodebuild -exportArchive")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("IPA export failed:\n{}", stderr.trim());
    }

    // Find the .ipa file
    let ipa = find_ipa(&export_path)?;
    Ok(ipa)
}

fn generate_export_plist(ios: &IosConfig) -> String {
    let method = &ios.export_method;

    let mut plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>{method}</string>
    <key>destination</key>
    <string>upload</string>
"#
    );

    if let Some(profile) = &ios.provisioning_profile {
        plist.push_str(&format!(
            "    <key>provisioningProfiles</key>\n    <dict>\n        <key>{}</key>\n        <string>{}</string>\n    </dict>\n",
            ios.bundle_id, profile
        ));
    }

    if let Some(identity) = &ios.code_sign_identity {
        plist.push_str(&format!(
            "    <key>signingCertificate</key>\n    <string>{}</string>\n",
            identity
        ));
    }

    plist.push_str("</dict>\n</plist>\n");
    plist
}

fn find_ipa(export_path: &Path) -> Result<PathBuf> {
    let entries = std::fs::read_dir(export_path)
        .with_context(|| format!("Cannot read export directory: {}", export_path.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("ipa") {
            return Ok(path);
        }
    }

    anyhow::bail!("No .ipa file found in {}", export_path.display())
}

// ─── Upload ───────────────────────────────────────────────────────────────────

async fn upload_to_asc(
    _ios: &IosConfig,
    apple: &AppleCredentials,
    ipa_path: &Path,
) -> Result<()> {
    let spinner = progress::spinner("Uploading IPA via xcrun altool...");

    let output = Command::new("xcrun")
        .args([
            "altool",
            "--upload-app",
            "--type",
            "ios",
            "--file",
            &ipa_path.to_string_lossy(),
            "--apiKey",
            &apple.key_id,
            "--apiIssuer",
            &apple.issuer_id,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run xcrun altool")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "Upload failed:\n{}\n{}",
            stderr.trim(),
            stdout.trim()
        );
    }

    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

async fn run_command(program: &str, args: &[&str], error_msg: &str) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed to run '{}'", program))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{}: {}", error_msg, stderr.trim());
    }

    Ok(())
}
