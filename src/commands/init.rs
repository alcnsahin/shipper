use anyhow::Result;
use console::style;
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

    // Detect existing Expo config and pre-fill defaults
    let expo = ExpoDefaults::detect();
    if expo.is_some() {
        println!(
            "  {} Expo project detected — pre-filling from app.json + eas.json",
            style("✓").bold().green()
        );
        println!();
    }
    let expo = expo.unwrap_or_default();

    let dir_name = detect_dir_name();
    let project_name_default = expo.name.as_deref().or(dir_name.as_deref());
    let project_name = prompt("Project name", project_name_default)?;

    println!();
    println!("  {}", style("iOS").bold());
    let ios_workspace = prompt_optional("  Workspace path (e.g. ios/MyApp.xcworkspace)", expo.ios_workspace.as_deref())?;
    let ios_scheme = prompt("  Scheme", expo.ios_scheme.as_deref())?;
    let ios_bundle_id = prompt("  Bundle ID", expo.ios_bundle_id.as_deref())?;
    let asc_app_id = prompt("  App Store Connect App ID (numeric)", expo.asc_app_id.as_deref())?;

    println!();
    println!("  {}", style("Android").bold());
    let android_dir = prompt("  Android project dir", Some("android"))?;
    let android_package = prompt("  Package name", expo.android_package.as_deref())?;
    let keystore_path = prompt("  Keystore path", Some("~/.shipper/keys/release.keystore"))?;
    let keystore_alias = prompt("  Keystore alias", None)?;

    // Global config: if eas.json had a service account, suggest copying it
    let service_account_hint = expo.google_service_account;

    let content = generate_project_config(
        &project_name,
        ios_workspace.as_deref(),
        &ios_scheme,
        &ios_bundle_id,
        &asc_app_id,
        &android_dir,
        &android_package,
        &keystore_path,
        &keystore_alias,
    );

    std::fs::write("shipper.toml", &content)?;
    println!();
    println!("  {} Created shipper.toml", style("✓").bold().green());

    ensure_global_config(&expo.apple_team_id, service_account_hint.as_deref())?;

    println!();
    println!("  {} Next steps:", style("→").bold().cyan());
    println!();
    println!("     1. Fill in your credentials in ~/.shipper/config.toml");
    println!("     2. Place your .p8 key at ~/.shipper/keys/AuthKey_<KEY_ID>.p8");
    if service_account_hint.is_none() {
        println!("     3. Place your Google service account at ~/.shipper/keys/play-store-sa.json");
    }
    println!("     4. Run: shipper deploy ios");
    println!();

    Ok(())
}

// ─── Expo config detection ────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ExpoDefaults {
    name: Option<String>,
    ios_bundle_id: Option<String>,
    ios_scheme: Option<String>,
    ios_workspace: Option<String>,
    android_package: Option<String>,
    // From eas.json
    asc_app_id: Option<String>,
    apple_team_id: Option<String>,
    google_service_account: Option<String>,
}

impl ExpoDefaults {
    fn detect() -> Option<Self> {
        let app_json_path = PathBuf::from("app.json");
        if !app_json_path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&app_json_path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let expo = json.get("expo")?;

        let mut defaults = ExpoDefaults::default();

        defaults.name = expo["name"].as_str().map(|s| s.to_string());

        // iOS
        defaults.ios_bundle_id = expo["ios"]["bundleIdentifier"]
            .as_str()
            .map(|s| s.to_string());

        // Infer scheme from name (Expo default: same as slug or name)
        defaults.ios_scheme = expo["scheme"]
            .as_str()
            .or_else(|| expo["slug"].as_str())
            .or_else(|| expo["name"].as_str())
            .map(|s| s.to_string());

        // Infer workspace path if ios/ dir exists
        if let Some(bundle_id) = &defaults.ios_bundle_id {
            let app_name = bundle_id.split('.').last().unwrap_or("App");
            let ws = format!("ios/{}.xcworkspace", capitalize(app_name));
            if PathBuf::from(&ws).exists() {
                defaults.ios_workspace = Some(ws);
            }
        }

        // Android
        defaults.android_package = expo["android"]["package"]
            .as_str()
            .map(|s| s.to_string());

        // EAS config (eas.json)
        if let Some(eas) = read_eas_json() {
            // ASC App ID
            defaults.asc_app_id = eas["submit"]["production"]["ios"]["ascAppId"]
                .as_str()
                .map(|s| s.to_string());

            // Apple Team ID
            defaults.apple_team_id = eas["submit"]["production"]["ios"]["appleTeamId"]
                .as_str()
                .map(|s| s.to_string());

            // Google service account
            defaults.google_service_account = eas["submit"]["production"]["android"]
                ["serviceAccountKeyPath"]
                .as_str()
                .map(|s| s.to_string());
        }

        Some(defaults)
    }
}

fn read_eas_json() -> Option<serde_json::Value> {
    let content = std::fs::read_to_string("eas.json").ok()?;
    serde_json::from_str(&content).ok()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn detect_dir_name() -> Option<String> {
    // Try package.json first
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
    let display = if let Some(d) = default {
        format!("  {} [{}]: ", label, style(d).dim())
    } else {
        format!("  {}: ", label)
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
    let display = if let Some(d) = default {
        format!("{} [{}]: ", label, style(d).dim())
    } else {
        format!("{} (optional): ", label)
    };

    print!("{}", display);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();

    if trimmed.is_empty() {
        Ok(default.map(|s| s.to_string()))
    } else {
        Ok(Some(trimmed))
    }
}

// ─── Config generation ────────────────────────────────────────────────────────

fn generate_project_config(
    name: &str,
    workspace: Option<&str>,
    scheme: &str,
    bundle_id: &str,
    asc_app_id: &str,
    android_dir: &str,
    android_package: &str,
    keystore_path: &str,
    keystore_alias: &str,
) -> String {
    let workspace_line = if let Some(ws) = workspace {
        format!("workspace = \"{}\"\n", ws)
    } else {
        "# workspace = \"ios/MyApp.xcworkspace\"\n# project = \"ios/MyApp.xcodeproj\"\n"
            .to_string()
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
track = "internal"
keystore_path = "{keystore_path}"
keystore_alias = "{keystore_alias}"
keystore_password_path = "~/.shipper/keys/keystore-password"
# key_password_path = "~/.shipper/keys/key-password"
build_type = "bundle"

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
