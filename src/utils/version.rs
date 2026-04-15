use anyhow::{Context, Result};
use regex::Regex;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct AppVersion {
    pub version_name: String, // e.g. "1.0.1"
    pub build_number: u32,    // e.g. 42
}

impl AppVersion {
    pub fn bump_build(&mut self) {
        self.build_number += 1;
    }

    pub fn bump_patch(&mut self) {
        let parts: Vec<u32> = self
            .version_name
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();
        if parts.len() == 3 {
            self.version_name = format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1);
        }
        self.build_number += 1;
    }
}

// ─── Expo (app.json) ──────────────────────────────────────────────────────────

/// Reads the iOS build number from app.json (expo.ios.buildNumber).
pub fn read_expo_version(app_json_path: &Path) -> Result<AppVersion> {
    let content = std::fs::read_to_string(app_json_path)
        .with_context(|| format!("Failed to read {}", app_json_path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).context("Failed to parse app.json")?;

    let version_name = json["expo"]["version"]
        .as_str()
        .unwrap_or("1.0.0")
        .to_string();

    let ios_build = json["expo"]["ios"]["buildNumber"]
        .as_str()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1);

    Ok(AppVersion { version_name, build_number: ios_build })
}

/// Reads the Android version code from app.json (expo.android.versionCode).
pub fn read_expo_version_android(app_json_path: &Path) -> Result<AppVersion> {
    let content = std::fs::read_to_string(app_json_path)
        .with_context(|| format!("Failed to read {}", app_json_path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).context("Failed to parse app.json")?;

    let version_name = json["expo"]["version"]
        .as_str()
        .unwrap_or("1.0.0")
        .to_string();

    // versionCode is stored as an integer in app.json
    let version_code = json["expo"]["android"]["versionCode"]
        .as_u64()
        .map(|v| v as u32)
        .unwrap_or(1);

    Ok(AppVersion { version_name, build_number: version_code })
}

pub fn write_expo_version_ios(app_json_path: &Path, version: &AppVersion) -> Result<()> {
    let content = std::fs::read_to_string(app_json_path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content)?;

    json["expo"]["version"] = serde_json::json!(version.version_name);
    json["expo"]["ios"]["buildNumber"] = serde_json::json!(version.build_number.to_string());

    let updated = serde_json::to_string_pretty(&json)?;
    std::fs::write(app_json_path, updated)?;
    Ok(())
}

pub fn write_expo_version_android(app_json_path: &Path, version: &AppVersion) -> Result<()> {
    let content = std::fs::read_to_string(app_json_path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content)?;

    json["expo"]["version"] = serde_json::json!(version.version_name);
    json["expo"]["android"]["versionCode"] = serde_json::json!(version.build_number);

    let updated = serde_json::to_string_pretty(&json)?;
    std::fs::write(app_json_path, updated)?;
    Ok(())
}

// ─── iOS native (Info.plist) ──────────────────────────────────────────────────

pub fn read_info_plist_version(plist_path: &Path) -> Result<AppVersion> {
    let content = std::fs::read_to_string(plist_path)
        .with_context(|| format!("Failed to read {}", plist_path.display()))?;

    let version_name = extract_plist_string(&content, "CFBundleShortVersionString")
        .unwrap_or_else(|| "1.0.0".to_string());
    let build_str = extract_plist_string(&content, "CFBundleVersion")
        .unwrap_or_else(|| "1".to_string());
    let build_number = build_str.parse::<u32>().unwrap_or(1);

    Ok(AppVersion { version_name, build_number })
}

pub fn write_info_plist_version(plist_path: &Path, version: &AppVersion) -> Result<()> {
    let content = std::fs::read_to_string(plist_path)?;

    let updated = replace_plist_string(
        &content,
        "CFBundleShortVersionString",
        &version.version_name,
    );
    let updated = replace_plist_string(
        &updated,
        "CFBundleVersion",
        &version.build_number.to_string(),
    );

    std::fs::write(plist_path, updated)?;
    Ok(())
}

fn extract_plist_string(content: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r"<key>{}</key>\s*<string>([^<]+)</string>",
        regex::escape(key)
    ))
    .ok()?;
    re.captures(content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn replace_plist_string(content: &str, key: &str, value: &str) -> String {
    let re = Regex::new(&format!(
        r"(<key>{}</key>\s*<string>)[^<]+(</string>)",
        regex::escape(key)
    ))
    .unwrap();
    re.replace(content, format!("${{1}}{}${{2}}", value))
        .to_string()
}

// ─── Android native (build.gradle) ───────────────────────────────────────────

pub fn read_gradle_version(gradle_path: &Path) -> Result<AppVersion> {
    let content = std::fs::read_to_string(gradle_path)
        .with_context(|| format!("Failed to read {}", gradle_path.display()))?;

    let version_code_re = Regex::new(r"versionCode\s+(\d+)").unwrap();
    let version_name_re = Regex::new(r#"versionName\s+"([^"]+)""#).unwrap();

    let build_number = version_code_re
        .captures(&content)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u32>().ok())
        .unwrap_or(1);

    let version_name = version_name_re
        .captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "1.0.0".to_string());

    Ok(AppVersion { version_name, build_number })
}

pub fn write_gradle_version(gradle_path: &Path, version: &AppVersion) -> Result<()> {
    let content = std::fs::read_to_string(gradle_path)?;

    let code_re = Regex::new(r"(versionCode\s+)\d+").unwrap();
    let name_re = Regex::new(r#"(versionName\s+)"[^"]+""#).unwrap();

    let updated = code_re
        .replace(&content, format!("${{1}}{}", version.build_number))
        .to_string();
    let updated = name_re
        .replace(&updated, format!("${{1}}\"{}\"", version.version_name))
        .to_string();

    std::fs::write(gradle_path, updated)?;
    Ok(())
}

// ─── Project type detection ───────────────────────────────────────────────────

pub fn is_expo_project() -> bool {
    Path::new("app.json").exists()
        && std::fs::read_to_string("app.json")
            .map(|s| s.contains("\"expo\""))
            .unwrap_or(false)
}
