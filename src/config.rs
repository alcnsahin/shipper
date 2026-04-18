use crate::utils::secret::Secret;
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
    /// `tracing`-style level filter (`error`/`warn`/`info`/`debug`/`trace`).
    /// Overridden by `RUST_LOG` and the `--verbose` CLI flag; see
    /// `utils::logger::init` for precedence.
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
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    pub bot_token_path: String,
    pub chat_id: String,
}

// ─── Project config: ./shipper.toml ──────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub ios: Option<IosConfig>,
    pub android: Option<AndroidConfig>,
    pub versioning: Option<VersioningConfig>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
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
    /// TestFlight beta group names to distribute the build to after processing.
    /// When present, the build is automatically submitted for beta review and
    /// added to each listed group.
    #[serde(default)]
    pub testflight_groups: Vec<String>,
}

fn default_export_method() -> String {
    "app-store-connect".to_string()
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
#[serde(deny_unknown_fields)]
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
    /// Staged rollout fraction for the production track (0.0, 1.0].
    /// When set on the production track, the release status becomes
    /// `inProgress` with the given `userFraction`. Omit or set to 1.0
    /// for a full (100%) rollout. Ignored on non-production tracks.
    pub rollout_fraction: Option<f64>,
}

fn default_track() -> String {
    "internal".to_string()
}

fn default_build_type() -> String {
    "bundle".to_string() // "bundle" = AAB, "apk" = APK
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
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

// ─── Validation ─────────────────────────────────────────────────────────────

const VALID_TRACKS: &[&str] = &["internal", "alpha", "beta", "production"];
const VALID_BUILD_TYPES: &[&str] = &["bundle", "apk"];
const VALID_EXPORT_METHODS: &[&str] = &[
    "app-store-connect",
    "app-store",
    "development",
    "ad-hoc",
    "enterprise",
];
const VALID_STRATEGIES: &[&str] = &["semver"];

impl ProjectConfig {
    /// Check semantic constraints that `serde` cannot express.
    /// Returns a list of human-readable warnings/errors.
    pub(crate) fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.project.name.trim().is_empty() {
            errors.push("[project] name must not be empty".into());
        }

        if let Some(ios) = &self.ios {
            if ios.scheme.trim().is_empty() {
                errors.push("[ios] scheme must not be empty".into());
            }
            if ios.bundle_id.trim().is_empty() {
                errors.push("[ios] bundle_id must not be empty".into());
            }
            if !VALID_EXPORT_METHODS.contains(&ios.export_method.as_str()) {
                errors.push(format!(
                    "[ios] export_method \"{}\" is invalid — expected one of: {}",
                    ios.export_method,
                    VALID_EXPORT_METHODS.join(", ")
                ));
            }
        }

        if let Some(android) = &self.android {
            if android.package_name.trim().is_empty() {
                errors.push("[android] package_name must not be empty".into());
            }
            if !VALID_TRACKS.contains(&android.track.as_str()) {
                errors.push(format!(
                    "[android] track \"{}\" is invalid — expected one of: {}",
                    android.track,
                    VALID_TRACKS.join(", ")
                ));
            }
            if !VALID_BUILD_TYPES.contains(&android.build_type.as_str()) {
                errors.push(format!(
                    "[android] build_type \"{}\" is invalid — expected one of: {}",
                    android.build_type,
                    VALID_BUILD_TYPES.join(", ")
                ));
            }
            if let Some(fraction) = android.rollout_fraction {
                if fraction <= 0.0 || fraction > 1.0 {
                    errors.push(format!(
                        "[android] rollout_fraction {fraction} is out of range — must be (0.0, 1.0]"
                    ));
                }
                if android.track != "production" && fraction < 1.0 {
                    errors.push(format!(
                        "[android] rollout_fraction is only supported on the production track (current: \"{}\")",
                        android.track
                    ));
                }
            }
        }

        if let Some(versioning) = &self.versioning {
            if !VALID_STRATEGIES.contains(&versioning.strategy.as_str()) {
                errors.push(format!(
                    "[versioning] strategy \"{}\" is invalid — expected one of: {}",
                    versioning.strategy,
                    VALID_STRATEGIES.join(", ")
                ));
            }
        }

        errors
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
    /// Build a `Config` reusing an already-parsed `GlobalConfig`. `main` loads
    /// the global config up-front to bootstrap the logger and threads it here,
    /// so only the project config is read on this path.
    pub fn with_global(global: GlobalConfig) -> Result<Self> {
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

/// Load `~/.shipper/config.toml` or fall back to a default config.
///
/// Failures to read or parse are swallowed (return default) because this is
/// called before the logger is fully configured and we never want config
/// parse noise to break the CLI at boot. Errors surface later on the
/// `Config::load()` path where they can be reported properly.
pub fn load_global_or_default() -> GlobalConfig {
    load_global_config().unwrap_or_default()
}

fn load_global_config() -> Result<GlobalConfig> {
    let path = global_config_path();
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))
}

fn load_project_config() -> Result<ProjectConfig> {
    let path = PathBuf::from("shipper.toml");
    if !path.exists() {
        anyhow::bail!(
            "shipper.toml not found in current directory.\nRun `shipper init` to create one."
        );
    }
    let content = std::fs::read_to_string(&path).context("Failed to read shipper.toml")?;
    let config: ProjectConfig = toml::from_str(&content).context("Failed to parse shipper.toml")?;

    let errors = config.validate();
    if !errors.is_empty() {
        anyhow::bail!(
            "shipper.toml validation failed:\n  • {}",
            errors.join("\n  • ")
        );
    }

    Ok(config)
}

/// Parse and validate shipper.toml without running a deploy.
/// Returns `(config, warnings)` on success; errors on parse/validation failure.
pub(crate) fn validate_project_config() -> Result<ProjectConfig> {
    load_project_config()
}

/// Expand ~ in paths
pub fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Read a secret from a file path, trimming trailing whitespace.
///
/// On Unix, refuses files that are group- or world-readable; run
/// `chmod 600 <path>` to restrict. Callers receive an opaque [`Secret`]
/// that redacts itself in logs and error chains.
pub fn read_secret(path: &str) -> Result<Secret> {
    let p = expand_path(path);
    Secret::read_from_file(&p).with_context(|| format!("reading secret at {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_path_expands_tilde_prefix() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_path("~/foo/bar"), home.join("foo/bar"));
    }

    #[test]
    fn expand_path_passes_through_absolute_path() {
        assert_eq!(expand_path("/etc/hosts"), PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn expand_path_passes_through_relative_path() {
        assert_eq!(expand_path("foo/bar"), PathBuf::from("foo/bar"));
    }

    #[test]
    fn expand_path_does_not_expand_tilde_without_slash() {
        // "~user/..." is not supported yet (Faz 5.7); must be passed through verbatim.
        assert_eq!(expand_path("~user/foo"), PathBuf::from("~user/foo"));
    }

    #[cfg(unix)]
    #[test]
    fn read_secret_trims_trailing_newline() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "hunter2\n").unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        let secret = read_secret(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(secret.expose(), "hunter2");
    }

    // ── Validation tests ────────────────────────────────────────────────

    #[test]
    fn valid_minimal_config() {
        let toml = r#"
            [project]
            name = "myapp"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.validate().is_empty());
    }

    #[test]
    fn empty_project_name_fails() {
        let config = ProjectConfig {
            project: ProjectSection {
                name: "".to_string(),
            },
            ..Default::default()
        };
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("name must not be empty")));
    }

    #[test]
    fn invalid_track_fails() {
        let toml = r#"
            [project]
            name = "myapp"

            [android]
            project_dir = "android"
            package_name = "com.example.app"
            track = "staging"
            keystore_path = "ks.jks"
            keystore_alias = "key"
            keystore_password_path = "pw"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("track")));
    }

    #[test]
    fn unknown_key_rejected() {
        let toml = r#"
            [project]
            name = "myapp"
            typo_field = "oops"
        "#;
        let result = toml::from_str::<ProjectConfig>(toml);
        assert!(result.is_err());
    }

    #[test]
    fn valid_full_config() {
        let toml = r#"
            [project]
            name = "myapp"

            [ios]
            scheme = "MyApp"
            bundle_id = "com.example.app"

            [android]
            project_dir = "android"
            package_name = "com.example.app"
            keystore_path = "ks.jks"
            keystore_alias = "key"
            keystore_password_path = "pw"

            [versioning]
            strategy = "semver"
            auto_increment = true
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.validate().is_empty());
    }

    // ── Faz 9: TestFlight groups ────────────────────────────────────────

    #[test]
    fn testflight_groups_empty_by_default() {
        let toml = r#"
            [project]
            name = "myapp"

            [ios]
            scheme = "MyApp"
            bundle_id = "com.example.app"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.ios.as_ref().unwrap().testflight_groups.is_empty());
    }

    #[test]
    fn testflight_groups_deserializes_list() {
        let toml = r#"
            [project]
            name = "myapp"

            [ios]
            scheme = "MyApp"
            bundle_id = "com.example.app"
            testflight_groups = ["Internal Testers", "QA Team"]
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let groups = &config.ios.as_ref().unwrap().testflight_groups;
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], "Internal Testers");
        assert_eq!(groups[1], "QA Team");
    }

    // ── Faz 9: rollout_fraction ─────────────────────────────────────────

    #[test]
    fn rollout_fraction_out_of_range_fails() {
        let toml = r#"
            [project]
            name = "myapp"

            [android]
            project_dir = "android"
            package_name = "com.example.app"
            track = "production"
            keystore_path = "ks.jks"
            keystore_alias = "key"
            keystore_password_path = "pw"
            rollout_fraction = 1.5
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("rollout_fraction")));
    }

    #[test]
    fn rollout_fraction_on_non_production_track_fails() {
        let toml = r#"
            [project]
            name = "myapp"

            [android]
            project_dir = "android"
            package_name = "com.example.app"
            track = "internal"
            keystore_path = "ks.jks"
            keystore_alias = "key"
            keystore_password_path = "pw"
            rollout_fraction = 0.1
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("production track")));
    }

    #[test]
    fn rollout_fraction_valid_on_production() {
        let toml = r#"
            [project]
            name = "myapp"

            [android]
            project_dir = "android"
            package_name = "com.example.app"
            track = "production"
            keystore_path = "ks.jks"
            keystore_alias = "key"
            keystore_password_path = "pw"
            rollout_fraction = 0.1
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.validate().is_empty());
    }
}
