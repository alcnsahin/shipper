use anyhow::{Context, Result};
use console::style;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

use crate::config::{AppleCredentials, Config, IosConfig};
use crate::error::ShipperError;
use crate::stores::appstore;
use crate::utils::progress;
use crate::utils::version::{self, AppVersion};

/// Timeout for expo prebuild / pod install.
const PREP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Timeout for xcodebuild archive.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Timeout for xcodebuild -exportArchive.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Timeout for xcrun altool upload.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

const TOTAL_STEPS_BASE: usize = 6;

/// Run the full iOS deploy pipeline.
///
/// When `pre_bumped` is `Some`, the version bump step is skipped — used by
/// `deploy all` to avoid a race on `app.json` when both platforms run in
/// parallel.
pub async fn deploy(config: &Config, pre_bumped: Option<AppVersion>) -> Result<AppVersion> {
    let ios = config.ios_config()?;
    let apple = config.apple_credentials()?;

    preflight_checks(ios)?;

    // Step count: 6 base + 1 if polling + 1 if TestFlight groups
    let has_poll = ios.asc_app_id.is_some();
    let has_testflight = has_poll && !ios.testflight_groups.is_empty();
    let total = TOTAL_STEPS_BASE + usize::from(has_poll) + usize::from(has_testflight);

    println!("{}", style("iOS Pipeline").bold().underlined());
    println!();

    // Step 1: Version bump
    let app_version = match pre_bumped {
        Some(v) => {
            progress::step(1, total, "Version (pre-bumped)");
            progress::success(&format!("{} ({})", v.version_name, v.build_number));
            v
        }
        None => {
            progress::step(1, total, "Bumping version");
            let v = bump_version(config, ios)?;
            progress::success(&format!("{} ({})", v.version_name, v.build_number));
            v
        }
    };

    // Read env vars from eas.json for the configured build profile.
    // These are injected into expo prebuild and xcodebuild so EXPO_PUBLIC_* values
    // are available when app.config.js runs and when Metro bundles the JS.
    let eas_env = read_eas_env_vars(&ios.build_profile);
    if !eas_env.is_empty() {
        println!(
            "  {} Using eas.json env vars from profile \"{}\" ({} vars)",
            style("i").dim(),
            ios.build_profile,
            eas_env.len()
        );
    }

    // Step 2: Expo prebuild (if applicable)
    if version::is_expo_project() {
        progress::step(2, total, "Running expo prebuild");
        expo_prebuild(&eas_env).await?;
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

    // Ensure signing credentials are installed (cert in Keychain, profile on disk).
    // Looks in ~/.shipper/<project_name>/ios/keys/ then ./credentials/ios/ — installs automatically.
    let project_name = &config.project.project.name;
    ensure_signing_setup(ios, project_name).await?;

    // Resolve signing config — use shipper.toml values, auto-detect, or prompt
    let (signing_profile, signing_identity) = resolve_signing_config(ios).await?;

    // Step 4: Archive
    progress::step(4, total, "Archiving with xcodebuild");
    let archive_path = archive(
        ios,
        resolved_workspace.as_deref(),
        &resolved_scheme,
        signing_profile.as_deref(),
        signing_identity.as_deref(),
        &apple.team_id,
        &app_version,
        &eas_env,
    )
    .await?;
    progress::success(&format!("Archive created: {}", archive_path.display()));

    // Step 5: Export IPA
    progress::step(5, total, "Exporting IPA");
    let ipa_path = export_ipa(
        ios,
        &archive_path,
        signing_profile.as_deref(),
        signing_identity.as_deref(),
        &apple.team_id,
    )
    .await?;
    progress::success(&format!("IPA: {}", ipa_path.display()));

    // Step 6: Upload to App Store Connect
    progress::step(6, total, "Uploading to App Store Connect");
    upload_to_asc(ios, apple, &ipa_path).await?;
    progress::success("Upload complete");

    // Step 7: Poll processing (only if asc_app_id is set)
    let mut current_step = 7;
    if let Some(asc_app_id) = &ios.asc_app_id {
        progress::step(
            current_step,
            total,
            "Waiting for App Store Connect processing",
        );
        let processed = appstore::poll_build_processing(
            apple,
            asc_app_id,
            &app_version.build_number.to_string(),
        )
        .await?;
        let uploaded = processed.uploaded_date.as_deref().unwrap_or("unknown");
        progress::success(&format!(
            "Build processed — v{} (id: {}, uploaded: {})",
            processed.version, processed.id, uploaded
        ));
        current_step += 1;

        // Step 8: TestFlight distribution (only when groups are configured)
        if !ios.testflight_groups.is_empty() {
            progress::step(current_step, total, "Distributing to TestFlight groups");

            // Submit for beta review first
            appstore::submit_to_testflight(apple, &processed.id).await?;
            tracing::info!("Build submitted for beta review");

            // Add to each configured group
            for group in &ios.testflight_groups {
                appstore::add_build_to_beta_group(apple, asc_app_id, &processed.id, group).await?;
                println!("    {} Added to group \"{}\"", style("✓").green(), group);
            }
            progress::success(&format!(
                "Distributed to {} group(s)",
                ios.testflight_groups.len()
            ));
        }
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

    check_tool("xcodebuild", "install Xcode from the App Store")?;
    check_tool(
        "xcrun",
        "install the Xcode Command Line Tools (xcode-select --install)",
    )?;

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

fn check_tool(tool: &'static str, hint: &'static str) -> Result<()> {
    which::which(tool).map_err(|_| crate::error::ShipperError::ToolNotFound { tool, hint })?;
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

async fn expo_prebuild(env_vars: &std::collections::HashMap<String, String>) -> Result<()> {
    let output = tokio::time::timeout(
        PREP_TIMEOUT,
        Command::new("npx")
            .args(["expo", "prebuild", "--platform", "ios", "--clean"])
            .envs(env_vars)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| {
        ShipperError::BuildFailed("expo prebuild (ios): timed out after 10 minutes".into())
    })?
    .context("Failed to run 'npx expo prebuild'")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ShipperError::BuildFailed(format!(
            "expo prebuild (ios) exited {}:\n{}",
            exit_code_str(&output.status),
            tail_lines(&stderr, 30)
        ))
        .into());
    }
    Ok(())
}

/// Read env vars from eas.json for the given build profile.
/// Returns an empty map if eas.json is absent, unreadable, or has no env section.
fn read_eas_env_vars(build_profile: &str) -> std::collections::HashMap<String, String> {
    let content = match std::fs::read_to_string("eas.json") {
        Ok(c) => c,
        Err(_) => return std::collections::HashMap::new(),
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return std::collections::HashMap::new(),
    };
    let env = &json["build"][build_profile]["env"];
    match env.as_object() {
        Some(obj) => obj
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        None => std::collections::HashMap::new(),
    }
}

// ─── CocoaPods ────────────────────────────────────────────────────────────────

async fn pod_install(ios_dir: &Path) -> Result<()> {
    let spinner = progress::timed_spinner("pod install --repo-update");

    let output = tokio::time::timeout(
        PREP_TIMEOUT,
        tokio::process::Command::new("pod")
            .args(["install", "--repo-update"])
            .current_dir(ios_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| ShipperError::BuildFailed("pod install: timed out after 10 minutes".into()))?
    .context("Failed to run 'pod install' — is CocoaPods installed? (gem install cocoapods)")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = if !stderr.trim().is_empty() {
            &stderr
        } else {
            &stdout
        };
        return Err(ShipperError::BuildFailed(format!(
            "pod install exited {}:\n{}",
            exit_code_str(&output.status),
            tail_lines(details, 40)
        ))
        .into());
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

fn find_scheme_in_workspace(_workspace: &str, configured_scheme: &str) -> Option<String> {
    // Expo puts .xcscheme inside the .xcodeproj, not the .xcworkspace.
    // Scan all .xcworkspace and .xcodeproj dirs in ios/ (skip Pods and xcuserdata).
    let schemes = collect_shared_schemes();

    if schemes.is_empty() {
        return None;
    }
    // Exact match — already correct
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

/// Collect all shared scheme names from ios/*.xcworkspace and ios/*.xcodeproj
/// (xcshareddata/xcschemes only — skips Pods and per-user xcuserdata).
fn collect_shared_schemes() -> Vec<String> {
    let mut schemes = Vec::new();
    let ios_entries = match std::fs::read_dir("ios") {
        Ok(e) => e,
        Err(_) => return schemes,
    };
    for entry in ios_entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "xcworkspace" && ext != "xcodeproj" {
            continue;
        }
        // Skip CocoaPods umbrella project
        if path.file_stem().and_then(|s| s.to_str()) == Some("Pods") {
            continue;
        }
        let schemes_dir = path.join("xcshareddata/xcschemes");
        if let Ok(entries) = std::fs::read_dir(&schemes_dir) {
            for se in entries.flatten() {
                let sp = se.path();
                if sp.extension().and_then(|e| e.to_str()) == Some("xcscheme") {
                    if let Some(name) = sp.file_stem().and_then(|s| s.to_str()) {
                        schemes.push(name.to_string());
                    }
                }
            }
        }
    }
    schemes
}

// ─── Signing setup (auto-install cert + profile) ─────────────────────────────

/// Called before every deploy. Checks whether the distribution certificate
/// and provisioning profile are already installed. If not, looks for them in:
///   1. ~/.shipper/keys/<bundle_id>/
///   2. ./credentials/ios/   (EAS download location)
///
/// If still missing, runs `eas credentials --platform ios` automatically to
/// download them, then copies to ~/.shipper/keys/<bundle_id>/ and installs.
async fn ensure_signing_setup(ios: &IosConfig, project_name: &str) -> Result<()> {
    let has_cert = detect_code_sign_identity().await.is_some();
    let has_profile = detect_provisioning_profile(&ios.bundle_id).is_some();

    if has_cert && has_profile {
        return Ok(());
    }

    // Project-scoped: ~/.shipper/{project_name}/ios/keys/
    let shipper_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".shipper")
        .join(project_name)
        .join("ios")
        .join("keys");

    let search_dirs: &[PathBuf] = &[shipper_dir.clone(), PathBuf::from("credentials/ios")];

    let find_cert = || {
        search_dirs
            .iter()
            .map(|d| d.join("dist-cert.p12"))
            .find(|p| p.exists())
    };
    let find_profile = || {
        search_dirs
            .iter()
            .map(|d| d.join("profile.mobileprovision"))
            .find(|p| p.exists())
    };

    // If anything is missing from both Xcode dirs and local disk, fetch via EAS automatically.
    if (!has_cert && find_cert().is_none()) || (!has_profile && find_profile().is_none()) {
        fetch_eas_credentials(ios, &shipper_dir).await?;
    }

    if !has_cert {
        match find_cert() {
            Some(ref path) => {
                let creds_json = search_dirs
                    .iter()
                    .map(|d| d.join("credentials.json"))
                    .find(|p| p.exists());
                let password = read_p12_password(creds_json.as_deref())?;
                let spinner =
                    progress::spinner("Installing distribution certificate to Keychain...");
                import_certificate(path, &password)?;
                spinner.finish_and_clear();
                println!(
                    "  {} Distribution certificate installed",
                    style("✓").green().bold()
                );
                persist_to_shipper_keys(path, &shipper_dir, "dist-cert.p12")?;
                if let Some(ref cj) = creds_json {
                    persist_to_shipper_keys(cj, &shipper_dir, "credentials.json")?;
                }
            }
            None => {
                anyhow::bail!(
                    "Distribution certificate not found after 'eas credentials'. \
                     Check your EAS project configuration."
                );
            }
        }
    }

    if !has_profile {
        match find_profile() {
            Some(ref path) => {
                let spinner = progress::spinner("Installing provisioning profile...");
                install_profile(path)?;
                spinner.finish_and_clear();
                println!(
                    "  {} Provisioning profile installed",
                    style("✓").green().bold()
                );
                persist_to_shipper_keys(path, &shipper_dir, "profile.mobileprovision")?;
            }
            None => {
                anyhow::bail!(
                    "Provisioning profile not found after 'eas credentials'. \
                     Check your EAS project configuration."
                );
            }
        }
    }

    Ok(())
}

/// Runs `eas credentials --platform ios` interactively so the user can authenticate
/// and download signing credentials. After EAS writes to ./credentials/ios/, copies
/// the files to ~/.shipper/keys/<bundle_id>/ for future runs.
async fn fetch_eas_credentials(_ios: &IosConfig, shipper_dir: &Path) -> Result<()> {
    println!(
        "  {} Signing credentials not found — launching EAS to download them...",
        style("i").dim()
    );
    println!();

    let status = tokio::process::Command::new("eas")
        .args(["credentials", "--platform", "ios"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .context("Failed to run 'eas credentials' — is EAS CLI installed? (npm i -g eas-cli)")?;

    println!();

    if !status.success() {
        anyhow::bail!(
            "'eas credentials --platform ios' failed — cannot continue without signing credentials"
        );
    }

    // Move any files EAS downloaded to ./credentials/ios/ into ~/.shipper/keys/<bundle_id>/
    // and delete them from the project directory — private keys must not stay in source tree.
    let eas_dir = PathBuf::from("credentials/ios");
    for filename in &[
        "dist-cert.p12",
        "profile.mobileprovision",
        "credentials.json",
    ] {
        let src = eas_dir.join(filename);
        if src.exists() {
            persist_to_shipper_keys(&src, shipper_dir, filename).ok();
        }
    }
    // Remove the now-empty EAS download directory
    std::fs::remove_dir(&eas_dir).ok();
    std::fs::remove_dir("credentials").ok();

    Ok(())
}

fn persist_to_shipper_keys(src: &Path, shipper_dir: &Path, filename: &str) -> Result<()> {
    let dest = shipper_dir.join(filename);
    if dest == src {
        return Ok(()); // already there
    }
    std::fs::create_dir_all(shipper_dir)?;
    std::fs::copy(src, &dest)?;
    // Delete the source so private keys don't linger outside ~/.shipper/keys/
    std::fs::remove_file(src).ok();
    Ok(())
}

fn read_p12_password(creds_json: Option<&Path>) -> Result<String> {
    if let Some(path) = creds_json {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                for key in &[
                    "certPassword",
                    "password",
                    "p12Password",
                    "distributionCertificatePassword",
                ] {
                    if let Some(pw) = json[key].as_str() {
                        return Ok(pw.to_string());
                    }
                }
            }
        }
    }
    print!("  P12 certificate password: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn import_certificate(p12_path: &Path, password: &str) -> Result<()> {
    let login_keychain = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join("Library/Keychains/login.keychain-db");

    let status = std::process::Command::new("security")
        .args([
            "import",
            &p12_path.to_string_lossy(),
            "-k",
            &login_keychain.to_string_lossy(),
            "-P",
            password,
            "-T",
            "/usr/bin/codesign",
            "-T",
            "/usr/bin/security",
            "-A",
        ])
        .status()
        .context("Failed to run 'security import'")?;

    if !status.success() {
        anyhow::bail!(
            "Failed to import certificate. Check that the password is correct.\n\
             Password is in ~/.shipper/keys/<bundle_id>/credentials.json → certPassword"
        );
    }
    Ok(())
}

fn install_profile(profile_path: &Path) -> Result<()> {
    let data = std::fs::read(profile_path)?;
    let text = String::from_utf8_lossy(&data);
    let start = text.find("<?xml").context("Invalid mobileprovision file")?;
    let end = text
        .find("</plist>")
        .context("Invalid mobileprovision file")?;
    let plist = &text[start..end + "</plist>".len()];

    let uuid = extract_plist_string(plist, "UUID")
        .context("Could not read UUID from provisioning profile")?;

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let candidates = [
        home.join("Library/Developer/Xcode/UserData/Provisioning Profiles"),
        home.join("Library/MobileDevice/Provisioning Profiles"),
    ];
    let dest_dir = candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone());

    std::fs::create_dir_all(&dest_dir)?;
    std::fs::copy(
        profile_path,
        dest_dir.join(format!("{}.mobileprovision", uuid)),
    )?;
    Ok(())
}

// ─── Signing config resolution ────────────────────────────────────────────────

async fn resolve_signing_config(ios: &IosConfig) -> Result<(Option<String>, Option<String>)> {
    let profile = match &ios.provisioning_profile {
        Some(p) => Some(p.clone()),
        None => {
            let detected = detect_provisioning_profile(&ios.bundle_id);
            match detected {
                Some(ref p) => {
                    println!(
                        "  {} Provisioning profile detected: {}",
                        style("i").dim(),
                        p
                    );
                    detected
                }
                None => prompt_optional_signing("Provisioning profile name")?,
            }
        }
    };

    let identity = match &ios.code_sign_identity {
        Some(i) => Some(i.clone()),
        None => {
            let detected = detect_code_sign_identity().await;
            match detected {
                Some(ref i) => {
                    println!("  {} Code sign identity detected: {}", style("i").dim(), i);
                    detected
                }
                None => prompt_optional_signing(
                    "Code sign identity (e.g. 'Apple Distribution: Company (TEAMID)')",
                )?,
            }
        }
    };

    Ok((profile, identity))
}

/// Scan known provisioning profile directories for a profile matching the bundle ID.
/// Only returns App Store / distribution profiles (get-task-allow = false).
/// Xcode 15+ uses ~/Library/Developer/Xcode/UserData/Provisioning Profiles/
/// Older versions use ~/Library/MobileDevice/Provisioning Profiles/
fn detect_provisioning_profile(bundle_id: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let candidates = [
        home.join("Library/Developer/Xcode/UserData/Provisioning Profiles"),
        home.join("Library/MobileDevice/Provisioning Profiles"),
    ];
    let profiles_dir = candidates.iter().find(|p| p.exists())?.clone();
    for entry in std::fs::read_dir(&profiles_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mobileprovision") {
            continue;
        }
        if let Some(info) = read_mobileprovision(&path) {
            // Skip development profiles
            if info.is_development {
                continue;
            }
            if info.bundle_id == bundle_id || info.bundle_id == format!("*.{}", bundle_id) {
                return Some(info.name);
            }
        }
    }
    None
}

struct ProfileInfo {
    name: String,
    bundle_id: String,
    is_development: bool,
}

fn read_mobileprovision(path: &Path) -> Option<ProfileInfo> {
    let data = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&data);
    // The plist is embedded as plaintext inside the PKCS7 envelope
    let start = text.find("<?xml")?;
    let end = text.find("</plist>")?;
    let plist = &text[start..end + "</plist>".len()];

    let name = extract_plist_string(plist, "Name")?;
    let app_id = extract_plist_string(plist, "application-identifier")?;
    // Strip team prefix: "TEAMID.com.example.app" → "com.example.app"
    let bundle_id = app_id
        .split_once('.')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| app_id.clone());
    let is_development = plist.contains("<key>get-task-allow</key>")
        && plist
            .split("<key>get-task-allow</key>")
            .nth(1)
            .map(|s| s.contains("<true/>"))
            .unwrap_or(false);

    Some(ProfileInfo {
        name,
        bundle_id,
        is_development,
    })
}

fn extract_plist_string(plist: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{}</key>", key);
    let after = plist.split(&marker).nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after.find("</string>")?;
    Some(after[start..end].trim().to_string())
}

/// Get the first Apple/iPhone Distribution identity from the Keychain.
async fn detect_code_sign_identity() -> Option<String> {
    let output = tokio::process::Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("Apple Distribution") || line.contains("iPhone Distribution") {
            if let (Some(s), Some(e)) = (line.find('"'), line.rfind('"')) {
                if s < e {
                    return Some(line[s + 1..e].to_string());
                }
            }
        }
    }
    None
}

fn prompt_optional_signing(label: &str) -> Result<Option<String>> {
    print!("  {} (optional, press Enter to skip): ", label);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    })
}

// Parameter list collapses in Faz 6.2 when ios.rs is split and build options
// move into a dedicated struct.
#[allow(clippy::too_many_arguments)]
async fn archive(
    ios: &IosConfig,
    workspace: Option<&str>,
    scheme: &str,
    provisioning_profile: Option<&str>,
    code_sign_identity: Option<&str>,
    team_id: &str,
    version: &AppVersion,
    eas_env: &std::collections::HashMap<String, String>,
) -> Result<PathBuf> {
    let archive_path = PathBuf::from(&ios.build_dir).join(format!("{}.xcarchive", scheme));

    std::fs::create_dir_all(&ios.build_dir).context("Failed to create build directory")?;

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

    args.push(format!("DEVELOPMENT_TEAM={}", team_id));

    if let Some(profile) = provisioning_profile {
        args.push(format!("PROVISIONING_PROFILE_SPECIFIER={}", profile));
    }

    if let Some(identity) = code_sign_identity {
        args.push(format!("CODE_SIGN_IDENTITY={}", identity));
    }

    // Inject version numbers
    args.push(format!("CURRENT_PROJECT_VERSION={}", version.build_number));
    args.push(format!("MARKETING_VERSION={}", version.version_name));

    let spinner = progress::timed_spinner("xcodebuild archive");

    let output = tokio::time::timeout(
        ARCHIVE_TIMEOUT,
        Command::new("xcodebuild")
            .args(&args)
            .envs(eas_env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| {
        ShipperError::BuildFailed("xcodebuild archive: timed out after 30 minutes".into())
    })?
    .context("Failed to run xcodebuild archive")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        let is_linker_failure = combined.contains("linker command failed")
            || combined.contains("Undefined symbols")
            || combined.contains("library not found");

        let errors: Vec<&str> = combined
            .lines()
            .filter(|l| {
                let t = l.trim();
                if t.is_empty() || t.starts_with("//") {
                    return false;
                }
                // Standard compiler/tool errors
                if t.contains("error:") || t.contains("fatal:") || t.starts_with("**") {
                    return true;
                }
                // Linker-specific output: ld errors, undefined symbols, missing libraries
                if is_linker_failure
                    && (t.starts_with("ld:")
                        || t.starts_with("Undefined symbols")
                        || t.starts_with("  \"_")
                        || t.contains("referenced from:")
                        || t.contains("library not found")
                        || t.contains("framework not found"))
                {
                    return true;
                }
                false
            })
            .take(40)
            .collect();

        let exit = exit_code_str(&output.status);
        if !errors.is_empty() {
            return Err(ShipperError::BuildFailed(format!(
                "xcodebuild archive exited {exit}:\n{}",
                errors.join("\n")
            ))
            .into());
        }

        // Fall back: last 40 lines of combined output (xcodebuild summary is at the end).
        return Err(ShipperError::BuildFailed(format!(
            "xcodebuild archive exited {exit}:\n{}",
            tail_lines(&combined, 40)
        ))
        .into());
    }

    Ok(archive_path)
}

// ─── Export IPA ───────────────────────────────────────────────────────────────

async fn export_ipa(
    ios: &IosConfig,
    archive_path: &Path,
    provisioning_profile: Option<&str>,
    code_sign_identity: Option<&str>,
    team_id: &str,
) -> Result<PathBuf> {
    let export_path = PathBuf::from(&ios.build_dir).join("ipa");

    let export_plist =
        generate_export_plist(ios, provisioning_profile, code_sign_identity, team_id);
    let plist_path = PathBuf::from(&ios.build_dir).join("ExportOptions.plist");
    std::fs::write(&plist_path, &export_plist).context("Failed to write ExportOptions.plist")?;

    let spinner = progress::timed_spinner("Exporting IPA");

    let output = tokio::time::timeout(
        EXPORT_TIMEOUT,
        Command::new("xcodebuild")
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
            .output(),
    )
    .await
    .map_err(|_| {
        ShipperError::BuildFailed("xcodebuild exportArchive: timed out after 10 minutes".into())
    })?
    .context("Failed to run xcodebuild -exportArchive")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ShipperError::BuildFailed(format!(
            "xcodebuild exportArchive exited {}:\n{}",
            exit_code_str(&output.status),
            tail_lines(&stderr, 30)
        ))
        .into());
    }

    // Find the .ipa file
    let ipa = find_ipa(&export_path)?;
    Ok(ipa)
}

fn generate_export_plist(
    ios: &IosConfig,
    provisioning_profile: Option<&str>,
    code_sign_identity: Option<&str>,
    team_id: &str,
) -> String {
    // "app-store" was deprecated in Xcode 16 — use "app-store-connect"
    let method = match ios.export_method.as_str() {
        "app-store" => "app-store-connect",
        other => other,
    };

    let mut plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>{method}</string>
    <key>destination</key>
    <string>export</string>
    <key>teamID</key>
    <string>{team_id}</string>
    <key>signingStyle</key>
    <string>manual</string>
"#
    );

    let profile = provisioning_profile.or(ios.provisioning_profile.as_deref());
    if let Some(p) = profile {
        plist.push_str(&format!(
            "    <key>provisioningProfiles</key>\n    <dict>\n        <key>{}</key>\n        <string>{}</string>\n    </dict>\n",
            ios.bundle_id, p
        ));
    }

    let identity = code_sign_identity.or(ios.code_sign_identity.as_deref());
    if let Some(i) = identity {
        plist.push_str(&format!(
            "    <key>signingCertificate</key>\n    <string>{}</string>\n",
            i
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

/// altool only searches a fixed set of directories for the .p8 key file.
/// Copy it to ~/.appstoreconnect/private_keys/ so altool can find it.
fn ensure_key_for_altool(apple: &AppleCredentials) -> Result<()> {
    let src = crate::config::expand_path(&apple.key_path);
    if !src.exists() {
        anyhow::bail!(
            "App Store Connect API key not found: {}\nCheck key_path in ~/.shipper/config.toml",
            src.display()
        );
    }
    let dest_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".appstoreconnect/private_keys");
    std::fs::create_dir_all(&dest_dir)?;
    let dest = dest_dir.join(format!("AuthKey_{}.p8", apple.key_id));
    if !dest.exists() {
        std::fs::copy(&src, &dest)
            .context("Failed to copy API key to ~/.appstoreconnect/private_keys/")?;
    }
    Ok(())
}

async fn upload_to_asc(_ios: &IosConfig, apple: &AppleCredentials, ipa_path: &Path) -> Result<()> {
    ensure_key_for_altool(apple)?;

    let spinner = progress::timed_spinner("Uploading IPA via xcrun altool");

    let output = tokio::time::timeout(
        UPLOAD_TIMEOUT,
        Command::new("xcrun")
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
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("xcrun altool: timed out after 15 minutes"))?
    .context("Failed to run xcrun altool")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!("Upload failed:\n{}\n{}", stderr.trim(), stdout.trim());
    }

    Ok(())
}

/// Return the last `n` non-empty lines of `text`.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().rev().take(n).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// Format an exit status as a human-readable string.
fn exit_code_str(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "signal".to_string(),
    }
}
