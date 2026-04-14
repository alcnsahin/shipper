use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

// ─── Global config: ~/.shipper/config.toml ───────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct GlobalConfig {
    #[serde(default)]
    pub global: GlobalSection,
    pub credentials: Option<Credentials>,
    pub notifications: Option<Notifications>,
}

#[derive(Debug, Deserialize, Default)]
pub struct GlobalSection {
    #[serde(default)]
    pub notify: Vec<String>,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Credentials {
    pub apple: Option<AppleCredentials>,
    pub google: Option<GoogleCredentials>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppleCredentials {
    pub team_id: String,
    pub key_id: String,
    pub issuer_id: String,
    pub key_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GoogleCredentials {
    pub service_account: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct Notifications {
    pub telegram: Option<TelegramConfig>,
    pub slack: Option<SlackConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    pub bot_token_path: String,
    pub chat_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SlackConfig {
    pub webhook_url_path: String,
}

// ─── Project config: ./shipper.toml ──────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub ios: Option<IosConfig>,
    pub android: Option<AndroidConfig>,
    pub versioning: Option<VersioningConfig>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProjectSection {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct IosConfig {
    pub workspace: Option<String>,
    pub project: Option<String>,
    pub scheme: String,
    pub bundle_id: String,
    pub asc_app_id: Option<String>,
    #[serde(default = "default_export_method")]
    pub export_method: String,
    pub provisioning_profile: Option<String>,
    pub code_sign_identity: Option<String>,
    #[serde(default = "default_configuration")]
    pub configuration: String,
    #[serde(default = "default_build_dir")]
    pub build_dir: String,
    /// EAS build profile to read env vars from (default: "production").
    /// Matches a key under `build` in eas.json.
    #[serde(default = "default_build_profile")]
    pub build_profile: String,
}

fn default_export_method() -> String {
    "app-store".to_string()
}

fn default_configuration() -> String {
    "Release".to_string()
}

fn default_build_dir() -> String {
    "build/shipper".to_string()
}

fn default_build_profile() -> String {
    "production".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct AndroidConfig {
    pub project_dir: String,
    pub package_name: String,
    #[serde(default = "default_track")]
    pub track: String,
    pub keystore_path: String,
    pub keystore_alias: String,
    pub keystore_password_path: String,
    pub key_password_path: Option<String>,
    #[serde(default = "default_build_type")]
    pub build_type: String,
    #[serde(default = "default_build_profile")]
    pub build_profile: String,
}

fn default_track() -> String {
    "internal".to_string()
}

fn default_build_type() -> String {
    "bundle".to_string() // "bundle" = AAB, "apk" = APK
}

#[derive(Debug, Deserialize, Clone)]
pub struct VersioningConfig {
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default = "bool_true")]
    pub auto_increment: bool,
}

fn default_strategy() -> String {
    "semver".to_string()
}

fn bool_true() -> bool {
    true
}

impl Default for VersioningConfig {
    fn default() -> Self {
        Self {
            strategy: default_strategy(),
            auto_increment: true,
        }
    }
}

// ─── Loading ─────────────────────────────────────────────────────────────────

/// Merged config ready to use
#[derive(Debug)]
pub struct Config {
    pub global: GlobalConfig,
    pub project: ProjectConfig,
}

impl Config {
    pub fn load() -> Result<Self> {
        let global = load_global_config()?;
        let project = load_project_config()?;
        Ok(Self { global, project })
    }

    pub fn apple_credentials(&self) -> Result<&AppleCredentials> {
        self.global
            .credentials
            .as_ref()
            .and_then(|c| c.apple.as_ref())
            .context("Missing [credentials.apple] in ~/.shipper/config.toml")
    }

    pub fn google_credentials(&self) -> Result<&GoogleCredentials> {
        self.global
            .credentials
            .as_ref()
            .and_then(|c| c.google.as_ref())
            .context("Missing [credentials.google] in ~/.shipper/config.toml")
    }

    pub fn ios_config(&self) -> Result<&IosConfig> {
        self.project
            .ios
            .as_ref()
            .context("Missing [ios] section in shipper.toml")
    }

    pub fn android_config(&self) -> Result<&AndroidConfig> {
        self.project
            .android
            .as_ref()
            .context("Missing [android] section in shipper.toml")
    }

    pub fn notify_channels(&self) -> &[String] {
        &self.global.global.notify
    }

    pub fn telegram_config(&self) -> Option<&TelegramConfig> {
        self.global
            .notifications
            .as_ref()
            .and_then(|n| n.telegram.as_ref())
    }
}

pub fn global_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".shipper")
        .join("config.toml")
}

fn load_global_config() -> Result<GlobalConfig> {
    let path = global_config_path();
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))
}

fn load_project_config() -> Result<ProjectConfig> {
    let path = PathBuf::from("shipper.toml");
    if !path.exists() {
        anyhow::bail!(
            "shipper.toml not found in current directory.\nRun `shipper init` to create one."
        );
    }
    let content = std::fs::read_to_string(&path)
        .context("Failed to read shipper.toml")?;
    toml::from_str(&content).context("Failed to parse shipper.toml")
}

/// Expand ~ in paths
pub fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(&path[2..])
    } else {
        PathBuf::from(path)
    }
}

/// Read a secret from a file path (strips trailing newline)
pub fn read_secret(path: &str) -> Result<String> {
    let p = expand_path(path);
    let content = std::fs::read_to_string(&p)
        .with_context(|| format!("Failed to read secret from {}", p.display()))?;
    Ok(content.trim().to_string())
}

