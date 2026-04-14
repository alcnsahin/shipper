use anyhow::Result;
use console::style;
use regex::Regex;
use std::io::{self, Write};
use std::path::PathBuf;

pub async fn run() -> Result<()> {
    println!("{}", style("Initializing shipper").bold());
    println!();

    let toml_path = PathBuf::from("shipper.toml");
    let existing = ExistingConfig::read(&toml_path);

    // If shipper.toml already exists, only add missing platforms instead of overwriting.
    if existing.has_ios || existing.has_android {
        return run_add_platform(&toml_path, existing).await;
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

    // ── Platform selection ────────────────────────────────────────────────────
    println!();
    let platforms = prompt_platforms()?;
    let configure_ios = platforms.ios;
    let configure_android = platforms.android;

    // ── iOS ───────────────────────────────────────────────────────────────────
    let ios_config = if configure_ios {
        Some(prompt_ios_inputs(&detected)?)
    } else {
        None
    };

    // ── Android ───────────────────────────────────────────────────────────────
    let android_config = if configure_android {
        Some(prompt_android_inputs(&detected)?)
    } else {
        None
    };

    let service_account_hint = if configure_android {
        detected.google_service_account.clone()
    } else {
        None
    };

    let content = generate_project_config(&project_name, ios_config.as_ref(), android_config.as_ref());
    std::fs::write(&toml_path, &content)?;
    println!();
    println!("  {} Created shipper.toml", style("✓").bold().green());

    ensure_global_config(
        if configure_ios { &detected.apple_team_id } else { &None },
        if configure_android { service_account_hint.as_deref() } else { None },
        configure_ios,
        configure_android,
    )?;

    print_next_steps(configure_ios, configure_android, service_account_hint.is_none());
    Ok(())
}

/// Called when shipper.toml already exists. Detects which platforms are missing
/// and appends only the new platform's config without touching existing sections.
async fn run_add_platform(toml_path: &PathBuf, existing: ExistingConfig) -> Result<()> {
    let detected = ProjectDefaults::detect();

    let missing_ios = !existing.has_ios;
    let missing_android = !existing.has_android;

    if !missing_ios && !missing_android {
        println!(
            "  {} shipper.toml already has both [ios] and [android] configured.",
            style("i").dim()
        );
        println!("  Nothing to add.");
        return Ok(());
    }

    // Tell the user what's already there and what will be added.
    let already: Vec<&str> = [
        if existing.has_ios { Some("ios") } else { None },
        if existing.has_android { Some("android") } else { None },
    ]
    .iter()
    .flatten()
    .copied()
    .collect();

    println!(
        "  {} shipper.toml found — {} already configured.",
        style("i").dim(),
        already.join(", ")
    );

    if detected.is_expo {
        println!(
            "  {} Expo project detected — reading app.json, eas.json, build.gradle",
            style("✓").bold().green()
        );
    }
    println!();

    // Ask which missing platform(s) to add.
    let add_ios = if missing_ios {
        print!("  Add [ios] configuration? [Y/n] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        !input.trim().eq_ignore_ascii_case("n")
    } else {
        false
    };

    let add_android = if missing_android {
        print!("  Add [android] configuration? [Y/n] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        !input.trim().eq_ignore_ascii_case("n")
    } else {
        false
    };

    if !add_ios && !add_android {
        println!("  Nothing added.");
        return Ok(());
    }

    // Collect inputs and append to existing shipper.toml.
    let mut append = String::new();

    let ios_config = if add_ios {
        println!();
        println!("  {}", style("iOS").bold());
        let cfg = prompt_ios_inputs(&detected)?;
        append.push_str(&generate_ios_section(&cfg));
        Some(cfg)
    } else {
        None
    };

    let android_config = if add_android {
        println!();
        println!("  {}", style("Android").bold());
        let cfg = prompt_android_inputs(&detected)?;
        append.push_str(&generate_android_section(&cfg));
        Some(cfg)
    } else {
        None
    };

    // Append new sections to the existing file.
    let mut existing_content = std::fs::read_to_string(toml_path)?;
    if !existing_content.ends_with('\n') {
        existing_content.push('\n');
    }
    existing_content.push_str(&append);
    std::fs::write(toml_path, &existing_content)?;

    println!();
    println!("  {} Updated shipper.toml", style("✓").bold().green());

    let service_account_hint = if add_android {
        detected.google_service_account.clone()
    } else {
        None
    };

    ensure_global_config(
        if add_ios { &detected.apple_team_id } else { &None },
        if add_android { service_account_hint.as_deref() } else { None },
        add_ios,
        add_android,
    )?;

    print_next_steps(
        ios_config.is_some(),
        android_config.is_some(),
        service_account_hint.is_none(),
    );
    Ok(())
}

// ─── Existing config detection ───────────────────────────────────────────────

struct ExistingConfig {
    has_ios: bool,
    has_android: bool,
}

impl ExistingConfig {
    fn read(path: &PathBuf) -> Self {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        ExistingConfig {
            has_ios: content.contains("[ios]"),
            has_android: content.contains("[android]"),
        }
    }
}

// ─── Shared prompt helpers ────────────────────────────────────────────────────

fn prompt_ios_inputs(detected: &ProjectDefaults) -> Result<IosInputs> {
    let workspace = prompt_optional("  Workspace path", detected.ios_workspace.as_deref())?;
    let scheme = prompt("  Scheme", detected.ios_scheme.as_deref())?;
    let bundle_id = prompt("  Bundle ID", detected.ios_bundle_id.as_deref())?;
    let asc_app_id = prompt_optional(
        "  App Store Connect App ID (leave empty if app doesn't exist yet)",
        detected.asc_app_id.as_deref(),
    )?;
    if asc_app_id.is_none() {
        println!(
            "  {} First upload: create the app at appstoreconnect.apple.com → Apps → +",
            style("i").dim()
        );
        println!(
            "  {} Then add asc_app_id to shipper.toml to enable build status polling.",
            style("i").dim()
        );
    }
    Ok(IosInputs { workspace, scheme, bundle_id, asc_app_id })
}

fn prompt_android_inputs(detected: &ProjectDefaults) -> Result<AndroidInputs> {
    let project_dir = prompt("  Project dir", Some("android"))?;
    let package_name = prompt("  Package name", detected.android_package.as_deref())?;
    let track = prompt(
        "  Release track (internal/alpha/beta/production)",
        detected.android_track.as_deref().or(Some("internal")),
    )?;
    let build_type_input = prompt(
        "  Build type (aab / apk)",
        Some(match detected.android_build_type.as_deref() {
            Some("bundle") | None => "aab",
            Some(other) => other,
        }),
    )?;
    let build_type = if build_type_input == "aab" { "bundle".to_string() } else { build_type_input };

    // Keystore path and alias: use detected values from build.gradle, or fall back to
    // shipper's default location. If the keystore doesn't exist at deploy time,
    // shipper will generate one automatically with keytool.
    let keystore_path = detected.keystore_path.clone()
        .unwrap_or_else(|| format!("~/.shipper/keys/{}.keystore", package_name));
    let keystore_alias = detected.keystore_alias.clone()
        .unwrap_or_else(|| "release".to_string());

    Ok(AndroidInputs { project_dir, package_name, track, build_type, keystore_path, keystore_alias })
}

fn print_next_steps(configure_ios: bool, configure_android: bool, needs_service_account: bool) {
    println!();
    println!("  {} Next steps:", style("→").bold().cyan());
    println!();
    println!("     1. Fill in credentials in ~/.shipper/config.toml");
    if configure_ios {
        println!("     2. Place .p8 key at ~/.shipper/keys/AuthKey_<KEY_ID>.p8");
    }
    if configure_android && needs_service_account {
        println!("     3. Place Google service account at ~/.shipper/keys/play-store-sa.json");
    }
    if configure_ios {
        println!("     → shipper deploy ios");
    }
    if configure_android {
        println!("     → shipper deploy android");
    }
    println!();
}

// ─── Platform selection ───────────────────────────────────────────────────────

struct PlatformChoice {
    ios: bool,
    android: bool,
}

fn prompt_platforms() -> Result<PlatformChoice> {
    println!("  {} Platform (ios / android / all):", style("?").bold().cyan());
    print!("  [all]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();

    let choice = match trimmed.as_str() {
        "ios" => PlatformChoice { ios: true, android: false },
        "android" => PlatformChoice { ios: false, android: true },
        "" | "all" | "both" => PlatformChoice { ios: true, android: true },
        other => anyhow::bail!(
            "Unknown platform '{}'. Use: ios, android, or all",
            other
        ),
    };

    Ok(choice)
}

// ─── Input containers ─────────────────────────────────────────────────────────

struct IosInputs {
    workspace: Option<String>,
    scheme: String,
    bundle_id: String,
    asc_app_id: Option<String>,
}

struct AndroidInputs {
    project_dir: String,
    package_name: String,
    track: String,
    build_type: String,
    keystore_path: String,
    keystore_alias: String,
}

// ─── Detection ────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ProjectDefaults {
    is_expo: bool,
    name: Option<String>,
    ios_bundle_id: Option<String>,
    ios_scheme: Option<String>,
    ios_workspace: Option<String>,
    android_package: Option<String>,
    android_track: Option<String>,
    android_build_type: Option<String>,
    keystore_path: Option<String>,
    keystore_alias: Option<String>,
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
            // Expo uses expo.name for the Xcode scheme name (not expo.scheme,
            // which is the deep-link URL scheme, nor expo.slug).
            d.ios_scheme = app_json["expo"]["name"]
                .as_str()
                .or_else(|| app_json["expo"]["slug"].as_str())
                .map(str::to_string);
            d.android_package = app_json["expo"]["android"]["package"]
                .as_str()
                .map(str::to_string);
        }

        d.ios_workspace = find_xcworkspace();

        // For Expo projects, ios/ doesn't exist until after prebuild.
        // Predict the workspace path from the scheme name so init can write it to shipper.toml.
        if d.is_expo && d.ios_workspace.is_none() {
            if let Some(ref scheme) = d.ios_scheme {
                d.ios_workspace = Some(format!("ios/{}.xcworkspace", scheme));
            }
        }

        if let Some(eas) = read_eas_json() {
            d.asc_app_id = find_eas_ios_field(&eas, "ascAppId");
            d.apple_team_id = find_eas_ios_field(&eas, "appleTeamId");
            d.google_service_account = find_eas_android_field(&eas, "serviceAccountKeyPath");
            d.android_track = find_eas_android_field(&eas, "track");
            d.android_build_type = find_eas_android_build_type(&eas);
        }

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
    let profiles = eas["build"].as_object()?;
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

fn read_gradle_signing() -> Option<(String, Option<String>)> {
    let content = std::fs::read_to_string("android/app/build.gradle").ok()?;
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
    for entry in std::fs::read_dir("ios").ok()?.flatten() {
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

fn generate_ios_section(ios: &IosInputs) -> String {
    let workspace_line = match &ios.workspace {
        Some(ws) => format!("workspace = \"{}\"\n", ws),
        None => "# workspace = \"ios/MyApp.xcworkspace\"\n".to_string(),
    };
    let asc_line = match &ios.asc_app_id {
        Some(id) => format!("asc_app_id = \"{}\"\n", id),
        None => "# asc_app_id = \"\"  # Add after creating app in App Store Connect\n".to_string(),
    };
    format!(
        r#"
[ios]
{workspace_line}scheme = "{scheme}"
bundle_id = "{bundle_id}"
{asc_line}export_method = "app-store"
# provisioning_profile = "MyApp AppStore"
# code_sign_identity = "Apple Distribution: Company Name (TEAMID)"
configuration = "Release"
"#,
        workspace_line = workspace_line,
        scheme = ios.scheme,
        bundle_id = ios.bundle_id,
        asc_line = asc_line,
    )
}

fn generate_android_section(android: &AndroidInputs) -> String {
    format!(
        r#"
[android]
project_dir = "{project_dir}"
package_name = "{package_name}"
track = "{track}"
keystore_path = "{keystore_path}"
keystore_alias = "{keystore_alias}"
keystore_password_path = "~/.shipper/keys/keystore-password"
# key_password_path = "~/.shipper/keys/key-password"
build_type = "{build_type}"
"#,
        project_dir = android.project_dir,
        package_name = android.package_name,
        track = android.track,
        keystore_path = android.keystore_path,
        keystore_alias = android.keystore_alias,
        build_type = android.build_type,
    )
}

fn generate_project_config(
    name: &str,
    ios: Option<&IosInputs>,
    android: Option<&AndroidInputs>,
) -> String {
    let mut out = format!("[project]\nname = \"{name}\"\n");

    if let Some(ios) = ios {
        out.push_str(&generate_ios_section(ios));
    }
    if let Some(android) = android {
        out.push_str(&generate_android_section(android));
    }

    out.push_str(
        r#"
[versioning]
strategy = "semver"
auto_increment = true
"#,
    );

    out
}

fn ensure_global_config(
    apple_team_id: &Option<String>,
    service_account_hint: Option<&str>,
    include_apple: bool,
    include_google: bool,
) -> Result<()> {
    let config_path = crate::config::global_config_path();
    if config_path.exists() {
        return Ok(());
    }

    let config_dir = config_path.parent().unwrap();
    std::fs::create_dir_all(config_dir)?;
    std::fs::create_dir_all(config_dir.join("keys"))?;

    let mut content = "[global]\nnotify = []\nlog_level = \"info\"\n".to_string();

    if include_apple {
        let team_id = apple_team_id.as_deref().unwrap_or("YOUR_TEAM_ID");
        content.push_str(&format!(
            r#"
[credentials.apple]
team_id = "{team_id}"
key_id = "YOUR_KEY_ID"
issuer_id = "your-issuer-id"
key_path = "~/.shipper/keys/AuthKey_YOUR_KEY_ID.p8"
"#
        ));
    }

    if include_google {
        let sa_path = service_account_hint.unwrap_or("~/.shipper/keys/play-store-sa.json");
        content.push_str(&format!(
            r#"
[credentials.google]
service_account = "{sa_path}"
"#
        ));
    }

    content.push_str(
        r#"
# [notifications.telegram]
# bot_token_path = "~/.shipper/keys/telegram-bot-token"
# chat_id = "-100xxxxxxxxxx"
"#,
    );

    std::fs::write(&config_path, content)?;
    println!(
        "  {} Created ~/.shipper/config.toml",
        style("✓").bold().green()
    );

    Ok(())
}
