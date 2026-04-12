use anyhow::{Context, Result};
use console::style;
use std::io::{self, Write};
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
    ).await?;
    progress::success(&format!("Archive created: {}", archive_path.display()));

    // Step 5: Export IPA
    progress::step(5, total, "Exporting IPA");
    let ipa_path = export_ipa(ios, &archive_path, signing_profile.as_deref(), signing_identity.as_deref(), &apple.team_id).await?;
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

// ─── Signing config resolution ────────────────────────────────────────────────

async fn resolve_signing_config(ios: &IosConfig) -> Result<(Option<String>, Option<String>)> {
    let profile = match &ios.provisioning_profile {
        Some(p) => Some(p.clone()),
        None => {
            let detected = detect_provisioning_profile(&ios.bundle_id);
            match detected {
                Some(ref p) => {
                    println!("  {} Provisioning profile detected: {}", style("i").dim(), p);
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
                None => prompt_optional_signing("Code sign identity (e.g. 'Apple Distribution: Company (TEAMID)')")?,
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
    let bundle_id = app_id.splitn(2, '.').nth(1).unwrap_or(&app_id).to_string();
    let is_development = plist.contains("<key>get-task-allow</key>")
        && plist
            .split("<key>get-task-allow</key>")
            .nth(1)
            .map(|s| s.contains("<true/>"))
            .unwrap_or(false);

    Some(ProfileInfo { name, bundle_id, is_development })
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
    Ok(if trimmed.is_empty() { None } else { Some(trimmed) })
}

async fn archive(
    ios: &IosConfig,
    workspace: Option<&str>,
    scheme: &str,
    provisioning_profile: Option<&str>,
    code_sign_identity: Option<&str>,
    team_id: &str,
    version: &AppVersion,
) -> Result<PathBuf> {
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
        // xcodebuild writes build logs to stdout and errors to stderr.
        // Combine both and extract lines containing "error:" for a clean message.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        let errors: Vec<&str> = combined
            .lines()
            .filter(|l| {
                let l = l.trim();
                l.contains("error:") && !l.starts_with("//")
            })
            .take(15)
            .collect();

        if errors.is_empty() {
            // Fall back to last 20 lines of stdout which usually has the summary
            let last_lines: Vec<&str> = stdout.lines().rev().take(20).collect::<Vec<_>>()
                .into_iter().rev().collect();
            anyhow::bail!("xcodebuild archive failed:\n{}", last_lines.join("\n"));
        } else {
            anyhow::bail!("xcodebuild archive failed:\n{}", errors.join("\n"));
        }
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

    let export_plist = generate_export_plist(ios, provisioning_profile, code_sign_identity, team_id);
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
