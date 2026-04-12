use anyhow::Result;
use console::style;
use regex::Regex;
use std::io::{self, Write};
use std::path::PathBuf;

pub async fn run() -> Result<()> {
    println!("{}", style("Initializing shipper").bold());
    println!();

    // Check if shipper.toml already exists
    if PathBuf::from("shipper.toml").exists() {
        println!(
            "  {} shipper.toml already exists in this directory.",
            style("!").yellow().bold()
        );
        print!("  Overwrite? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  Aborted.");
            return Ok(());
        }
    }

    let detected = ProjectDefaults::detect();
    if detected.is_expo {
        println!(
            "  {} Expo project detected — reading app.json, eas.json, build.gradle",
            style("✓").bold().green()
        );
        println!();
    }

    // ── Project ───────────────────────────────────────────────────────────────
    let dir_name = detect_dir_name();
    let project_name = prompt(
        "Project name",
        detected.name.as_deref().or(dir_name.as_deref()),
    )?;

    // ── iOS ───────────────────────────────────────────────────────────────────
    println!();
    println!("  {}", style("iOS").bold());
    let ios_workspace = prompt_optional(
        "  Workspace path",
        detected.ios_workspace.as_deref(),
    )?;
    let ios_scheme = prompt("  Scheme", detected.ios_scheme.as_deref())?;
    let ios_bundle_id = prompt("  Bundle ID", detected.ios_bundle_id.as_deref())?;
    let asc_app_id = prompt(
        "  App Store Connect App ID",
        detected.asc_app_id.as_deref(),
    )?;

    // ── Android ───────────────────────────────────────────────────────────────
    println!();
    println!("  {}", style("Android").bold());
    let android_dir = prompt("  Project dir", Some("android"))?;
    let android_package = prompt("  Package name", detected.android_package.as_deref())?;
    let android_track = prompt(
        "  Release track (internal/alpha/beta/production)",
        detected.android_track.as_deref().or(Some("internal")),
    )?;
    let build_type = prompt(
        "  Build type (bundle=AAB / apk)",
        detected.android_build_type.as_deref().or(Some("bundle")),
    )?;
    let keystore_path = prompt(
        "  Keystore path",
        detected
            .keystore_path
            .as_deref()
            .or(Some("~/.shipper/keys/release.keystore")),
    )?;
    let keystore_alias = prompt("  Keystore alias", detected.keystore_alias.as_deref())?;

    let service_account_hint = detected.google_service_account.clone();

    let content = generate_project_config(
        &project_name,
        ios_workspace.as_deref(),
        &ios_scheme,
        &ios_bundle_id,
        &asc_app_id,
        &android_dir,
        &android_package,
        &android_track,
        &build_type,
        &keystore_path,
        &keystore_alias,
    );

    std::fs::write("shipper.toml", &content)?;
    println!();
    println!("  {} Created shipper.toml", style("✓").bold().green());

    ensure_global_config(&detected.apple_team_id, service_account_hint.as_deref())?;

    println!();
    println!("  {} Next steps:", style("→").bold().cyan());
    println!();
    println!("     1. Fill in credentials in ~/.shipper/config.toml");
    println!("     2. Place .p8 key at ~/.shipper/keys/AuthKey_<KEY_ID>.p8");
    if service_account_hint.is_none() {
        println!("     3. Place Google service account at ~/.shipper/keys/play-store-sa.json");
    }
    println!("     4. shipper deploy ios");
    println!();

    Ok(())
}

// ─── Detection ────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ProjectDefaults {
    is_expo: bool,
    // General
    name: Option<String>,
    // iOS
    ios_bundle_id: Option<String>,
    ios_scheme: Option<String>,
    ios_workspace: Option<String>,
    // Android
    android_package: Option<String>,
    android_track: Option<String>,
    android_build_type: Option<String>,
    keystore_path: Option<String>,
    keystore_alias: Option<String>,
    // Credentials (→ global config)
    asc_app_id: Option<String>,
    apple_team_id: Option<String>,
    google_service_account: Option<String>,
}

impl ProjectDefaults {
    fn detect() -> Self {
        let mut d = ProjectDefaults::default();

        if let Some(app_json) = read_app_json() {
            d.is_expo = true;
            d.name = app_json["expo"]["name"].as_str().map(str::to_string);
            d.ios_bundle_id = app_json["expo"]["ios"]["bundleIdentifier"]
                .as_str()
                .map(str::to_string);
            d.ios_scheme = app_json["expo"]["scheme"]
                .as_str()
                .or_else(|| app_json["expo"]["slug"].as_str())
                .or_else(|| app_json["expo"]["name"].as_str())
                .map(str::to_string);
            d.android_package = app_json["expo"]["android"]["package"]
                .as_str()
                .map(str::to_string);
        }

        // ios/ dir scan → .xcworkspace
        d.ios_workspace = find_xcworkspace();

        // eas.json
        if let Some(eas) = read_eas_json() {
            d.asc_app_id = find_eas_ios_field(&eas, "ascAppId");
            d.apple_team_id = find_eas_ios_field(&eas, "appleTeamId");
            d.google_service_account =
                find_eas_android_field(&eas, "serviceAccountKeyPath");
            d.android_track = find_eas_android_field(&eas, "track");
            // build type from build profiles
            d.android_build_type = find_eas_android_build_type(&eas);
        }

        // android/app/build.gradle → keystore alias + path
        if let Some((alias, path)) = read_gradle_signing() {
            d.keystore_alias = Some(alias);
            if let Some(p) = path {
                d.keystore_path = Some(p);
            }
        }

        d
    }
}

// ─── Readers ──────────────────────────────────────────────────────────────────

fn read_app_json() -> Option<serde_json::Value> {
    let content = std::fs::read_to_string("app.json").ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("expo")?;
    Some(json)
}

fn read_eas_json() -> Option<serde_json::Value> {
    let content = std::fs::read_to_string("eas.json").ok()?;
    serde_json::from_str(&content).ok()
}

fn find_eas_ios_field(eas: &serde_json::Value, field: &str) -> Option<String> {
    let profiles = eas["submit"].as_object()?;
    for (_, profile) in profiles {
        if let Some(val) = profile["ios"][field].as_str() {
            return Some(val.to_string());
        }
    }
    None
}

fn find_eas_android_field(eas: &serde_json::Value, field: &str) -> Option<String> {
    let profiles = eas["submit"].as_object()?;
    for (_, profile) in profiles {
        if let Some(val) = profile["android"][field].as_str() {
            return Some(val.to_string());
        }
    }
    None
}

fn find_eas_android_build_type(eas: &serde_json::Value) -> Option<String> {
    // Check build profiles for android.buildType
    let profiles = eas["build"].as_object()?;
    // Prefer "production" profile, then any
    let order = ["production", "preview", "release"];
    for name in &order {
        if let Some(val) = profiles.get(*name).and_then(|p| p["android"]["buildType"].as_str()) {
            return Some(if val == "aab" { "bundle" } else { val }.to_string());
        }
    }
    for (_, profile) in profiles {
        if let Some(val) = profile["android"]["buildType"].as_str() {
            return Some(if val == "aab" { "bundle" } else { val }.to_string());
        }
    }
    None
}

/// Parse android/app/build.gradle for signingConfigs.release
fn read_gradle_signing() -> Option<(String, Option<String>)> {
    let gradle_path = PathBuf::from("android/app/build.gradle");
    let content = std::fs::read_to_string(&gradle_path).ok()?;

    let alias_re = Regex::new(r#"keyAlias\s+["']?([^"'\s\n]+)["']?"#).unwrap();
    let store_re = Regex::new(r#"storeFile\s+file\(["']([^"']+)["']\)"#).unwrap();

    let alias = alias_re
        .captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())?;

    let store_path = store_re
        .captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    Some((alias, store_path))
}

fn find_xcworkspace() -> Option<String> {
    let ios_dir = PathBuf::from("ios");
    if !ios_dir.exists() {
        return None;
    }
    for entry in std::fs::read_dir(&ios_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("xcworkspace") {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

fn detect_dir_name() -> Option<String> {
    if let Ok(content) = std::fs::read_to_string("package.json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(name) = json["name"].as_str() {
                return Some(name.to_string());
            }
        }
    }
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
}

// ─── Prompts ──────────────────────────────────────────────────────────────────

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    let display = match default {
        Some(d) => format!("  {} [{}]: ", label, style(d).dim()),
        None => format!("  {}: ", label),
    };
    print!("{}", display);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();

    if trimmed.is_empty() {
        if let Some(d) = default {
            return Ok(d.to_string());
        }
        anyhow::bail!("{} is required", label);
    }
    Ok(trimmed)
}

fn prompt_optional(label: &str, default: Option<&str>) -> Result<Option<String>> {
    let display = match default {
        Some(d) => format!("{} [{}]: ", label, style(d).dim()),
        None => format!("{} (optional): ", label),
    };
    print!("{}", display);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();

    if trimmed.is_empty() {
        Ok(default.map(str::to_string))
    } else {
        Ok(Some(trimmed))
    }
}

// ─── Config generation ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn generate_project_config(
    name: &str,
    workspace: Option<&str>,
    scheme: &str,
    bundle_id: &str,
    asc_app_id: &str,
    android_dir: &str,
    android_package: &str,
    android_track: &str,
    build_type: &str,
    keystore_path: &str,
    keystore_alias: &str,
) -> String {
    let workspace_line = match workspace {
        Some(ws) => format!("workspace = \"{}\"\n", ws),
        None => "# workspace = \"ios/MyApp.xcworkspace\"\n".to_string(),
    };

    format!(
        r#"[project]
name = "{name}"

[ios]
{workspace_line}scheme = "{scheme}"
bundle_id = "{bundle_id}"
asc_app_id = "{asc_app_id}"
export_method = "app-store"
# provisioning_profile = "MyApp AppStore"
# code_sign_identity = "Apple Distribution: Company Name (TEAMID)"
configuration = "Release"

[android]
project_dir = "{android_dir}"
package_name = "{android_package}"
track = "{android_track}"
keystore_path = "{keystore_path}"
keystore_alias = "{keystore_alias}"
keystore_password_path = "~/.shipper/keys/keystore-password"
# key_password_path = "~/.shipper/keys/key-password"
build_type = "{build_type}"

[versioning]
strategy = "semver"
auto_increment = true
"#
    )
}

fn ensure_global_config(
    apple_team_id: &Option<String>,
    service_account_hint: Option<&str>,
) -> Result<()> {
    let config_path = crate::config::global_config_path();
    if config_path.exists() {
        return Ok(());
    }

    let config_dir = config_path.parent().unwrap();
    std::fs::create_dir_all(config_dir)?;
    std::fs::create_dir_all(config_dir.join("keys"))?;

    let team_id = apple_team_id.as_deref().unwrap_or("YOUR_TEAM_ID");
    let sa_path = service_account_hint.unwrap_or("~/.shipper/keys/play-store-sa.json");

    let content = format!(
        r#"[global]
notify = []
log_level = "info"

[credentials.apple]
team_id = "{team_id}"
key_id = "YOUR_KEY_ID"
issuer_id = "your-issuer-id"
key_path = "~/.shipper/keys/AuthKey_YOUR_KEY_ID.p8"

[credentials.google]
service_account = "{sa_path}"

# [notifications.telegram]
# bot_token_path = "~/.shipper/keys/telegram-bot-token"
# chat_id = "-100xxxxxxxxxx"
"#
    );

    std::fs::write(&config_path, content)?;
    println!(
        "  {} Created ~/.shipper/config.toml",
        style("✓").bold().green()
    );

    Ok(())
}
