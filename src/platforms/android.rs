use anyhow::{Context, Result};
use console::style;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::config::{AndroidConfig, Config};
use crate::stores::playstore;
use crate::utils::progress;
use crate::utils::version::{self, AppVersion};

const TOTAL_STEPS: usize = 6;

pub async fn deploy(config: &Config) -> Result<AppVersion> {
    let android = config.android_config()?;
    let google = config.google_credentials()?;

    println!("{}", style("Android Pipeline").bold().underlined());
    println!();

    // Step 1: Version bump
    progress::step(1, TOTAL_STEPS, "Bumping version");
    let app_version = bump_version(config, android)?;
    progress::success(&format!(
        "{} ({})",
        app_version.version_name, app_version.build_number
    ));

    // Step 2: Expo prebuild (if applicable)
    let eas_env = read_eas_env_vars(&android.build_profile);
    if !eas_env.is_empty() {
        println!(
            "  {} Using eas.json env vars from profile \"{}\" ({} vars)",
            style("i").dim(),
            android.build_profile,
            eas_env.len()
        );
    }
    if version::is_expo_project() {
        progress::step(2, TOTAL_STEPS, "Running expo prebuild");
        expo_prebuild(&eas_env).await?;
        progress::success("Expo prebuild complete");
    } else {
        progress::step(2, TOTAL_STEPS, "Expo prebuild — skipped (not an Expo project)");
    }

    // Preflight and keystore after prebuild so android/ directory exists
    preflight_checks(android)?;
    ensure_keystore_setup(android).await?;

    // Step 3: Build
    let artifact_path = if android.build_type == "apk" {
        progress::step(3, TOTAL_STEPS, "Building APK with Gradle");
        let path = build_apk(android).await?;
        progress::success(&format!("APK: {}", path.display()));
        path
    } else {
        progress::step(3, TOTAL_STEPS, "Building AAB with Gradle");
        let path = build_aab(android).await?;
        progress::success(&format!("AAB: {}", path.display()));
        path
    };

    // Step 4: Sign
    progress::step(4, TOTAL_STEPS, "Signing artifact");
    let signed_path = sign_artifact(android, &artifact_path).await?;
    progress::success(&format!("Signed: {}", signed_path.display()));

    // Step 5: Upload to Play Store
    progress::step(5, TOTAL_STEPS, "Uploading to Play Store");
    let version_code = playstore::upload_aab(
        google,
        &android.package_name,
        &android.track,
        &signed_path,
    )
    .await?;
    progress::success(&format!("Uploaded (versionCode: {})", version_code));

    // Step 6: Done
    progress::step(6, TOTAL_STEPS, "Complete");
    progress::success(&format!(
        "{} v{} ({}) → {} track",
        android.package_name,
        app_version.version_name,
        app_version.build_number,
        android.track
    ));

    Ok(app_version)
}

// ─── Expo prebuild ────────────────────────────────────────────────────────────

async fn expo_prebuild(env_vars: &std::collections::HashMap<String, String>) -> Result<()> {
    let output = tokio::process::Command::new("npx")
        .args(["expo", "prebuild", "--platform", "android", "--clean"])
        .envs(env_vars)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run 'npx expo prebuild'")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Expo prebuild failed: {}", stderr.trim());
    }
    Ok(())
}

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

// ─── Preflight ────────────────────────────────────────────────────────────────

fn preflight_checks(android: &AndroidConfig) -> Result<()> {
    let spinner = progress::spinner("Running preflight checks...");

    let project_dir = Path::new(&android.project_dir);
    if !project_dir.exists() {
        anyhow::bail!(
            "Android project directory not found: {}",
            android.project_dir
        );
    }

    // Check gradlew
    let gradlew = project_dir.join("gradlew");
    if !gradlew.exists() {
        anyhow::bail!(
            "gradlew not found in {}. Run from project root.",
            android.project_dir
        );
    }

    // Check apksigner (part of Android Build Tools)
    which::which("apksigner").or_else(|_| which::which("jarsigner"))
        .context("Neither 'apksigner' nor 'jarsigner' found. Install Android SDK.")?;

    spinner.finish_and_clear();

    // Ensure local.properties has sdk.dir so Gradle can find the Android SDK.
    ensure_local_properties(project_dir)?;

    Ok(())
}

/// Writes android/local.properties with sdk.dir if it is missing or incomplete.
/// Resolution order:
///   1. $ANDROID_HOME env var
///   2. $ANDROID_SDK_ROOT env var
///   3. ~/Library/Android/sdk  (macOS Android Studio default)
///   4. ~/Android/Sdk          (Linux Android Studio default)
fn ensure_local_properties(project_dir: &Path) -> Result<()> {
    let props_path = project_dir.join("local.properties");

    // If the file already has sdk.dir, nothing to do.
    if let Ok(content) = std::fs::read_to_string(&props_path) {
        if content.lines().any(|l| l.trim_start().starts_with("sdk.dir")) {
            return Ok(());
        }
    }

    let sdk_path = std::env::var("ANDROID_HOME")
        .ok()
        .or_else(|| std::env::var("ANDROID_SDK_ROOT").ok())
        .map(PathBuf::from)
        .or_else(|| {
            dirs::home_dir().and_then(|h| {
                let candidates = [
                    h.join("Library/Android/sdk"),   // macOS
                    h.join("Android/Sdk"),            // Linux
                ];
                candidates.into_iter().find(|p| p.exists())
            })
        });

    match sdk_path {
        Some(path) => {
            // Append or create local.properties
            let line = format!("sdk.dir={}\n", path.to_string_lossy().replace('\\', "\\\\"));
            let existing = std::fs::read_to_string(&props_path).unwrap_or_default();
            std::fs::write(&props_path, format!("{}{}", existing, line))?;
            println!(
                "  {} Android SDK: {} (written to local.properties)",
                style("i").dim(),
                path.display()
            );
            Ok(())
        }
        None => anyhow::bail!(
            "Android SDK not found. Set ANDROID_HOME or install Android Studio.\n\
             Then re-run: shipper deploy android"
        ),
    }
}

// ─── Keystore setup ───────────────────────────────────────────────────────────

/// Ensures a release keystore exists before signing. If not found, generates one
/// with keytool and saves the password to the configured keystore_password_path.
///
/// ⚠ The generated keystore must be backed up — losing it means you can never
/// update the app on Play Store.
async fn ensure_keystore_setup(android: &AndroidConfig) -> Result<()> {
    let ks_path = crate::config::expand_path(&android.keystore_path);

    if ks_path.exists() {
        return Ok(());
    }

    println!(
        "  {} Keystore not found at {} — generating a new one...",
        style("i").dim(),
        ks_path.display()
    );
    println!(
        "  {} {} Back up this file — losing it means you cannot update the app on Play Store.",
        style("!").yellow().bold(),
        style("Important:").bold()
    );
    println!();

    // Prompt for password
    print!("  Keystore password (min 6 chars): ");
    io::stdout().flush()?;
    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    let password = password.trim().to_string();
    if password.len() < 6 {
        anyhow::bail!("Keystore password must be at least 6 characters");
    }

    // Create parent directory
    if let Some(parent) = ks_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let spinner = progress::spinner("Generating keystore with keytool...");

    let status = Command::new("keytool")
        .args([
            "-genkeypair",
            "-v",
            "-keystore", &ks_path.to_string_lossy(),
            "-alias", &android.keystore_alias,
            "-keyalg", "RSA",
            "-keysize", "2048",
            "-validity", "10000",
            "-storepass", &password,
            "-keypass", &password,
            "-dname", &format!("CN={}, OU=Android, O=Android, L=Android, ST=Android, C=US", android.package_name),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .context("Failed to run keytool — is JDK installed?")?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("keytool failed to generate keystore");
    }

    // Save password to the configured password file
    let pw_path = crate::config::expand_path(&android.keystore_password_path);
    if let Some(parent) = pw_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pw_path, &password)?;
    // Restrict permissions on the password file
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pw_path, std::fs::Permissions::from_mode(0o600))?;
    }

    println!("  {} Keystore generated: {}", style("✓").green().bold(), ks_path.display());
    println!("  {} Password saved to: {}", style("✓").green().bold(), pw_path.display());
    println!(
        "  {} Back up {} now!",
        style("!").yellow().bold(),
        ks_path.display()
    );
    println!();

    Ok(())
}

// ─── Version ─────────────────────────────────────────────────────────────────

fn bump_version(config: &Config, android: &AndroidConfig) -> Result<AppVersion> {
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
        version::write_expo_version_android(app_json, &v)?;
        return Ok(v);
    }

    let gradle_path = Path::new(&android.project_dir)
        .join("app")
        .join("build.gradle");

    if !gradle_path.exists() {
        anyhow::bail!("build.gradle not found at {}", gradle_path.display());
    }

    let mut v = version::read_gradle_version(&gradle_path)?;
    if auto_increment {
        v.bump_build();
    }
    version::write_gradle_version(&gradle_path, &v)?;
    Ok(v)
}

// ─── Gradle build ─────────────────────────────────────────────────────────────

async fn build_aab(android: &AndroidConfig) -> Result<PathBuf> {
    let project_dir = Path::new(&android.project_dir);

    let spinner = progress::spinner("./gradlew bundleRelease (this can take a few minutes)...");

    let output = Command::new("./gradlew")
        .arg("bundleRelease")
        .current_dir(project_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run gradlew bundleRelease")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let errors: Vec<&str> = stderr
            .lines()
            .filter(|l| l.contains("error") || l.contains("FAILED"))
            .take(15)
            .collect();
        anyhow::bail!(
            "Gradle build failed:\n{}",
            if errors.is_empty() { stderr.trim().to_string() } else { errors.join("\n") }
        );
    }

    // Find the generated AAB
    let aab_path = project_dir
        .join("app")
        .join("build")
        .join("outputs")
        .join("bundle")
        .join("release")
        .join("app-release.aab");

    if !aab_path.exists() {
        anyhow::bail!("AAB not found at expected location: {}", aab_path.display());
    }

    Ok(aab_path)
}

async fn build_apk(android: &AndroidConfig) -> Result<PathBuf> {
    let project_dir = Path::new(&android.project_dir);

    let spinner = progress::spinner("./gradlew assembleRelease...");

    let output = Command::new("./gradlew")
        .arg("assembleRelease")
        .current_dir(project_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .context("Failed to run gradlew assembleRelease")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Gradle build failed:\n{}", stderr.trim());
    }

    let apk_path = project_dir
        .join("app")
        .join("build")
        .join("outputs")
        .join("apk")
        .join("release")
        .join("app-release-unsigned.apk");

    if !apk_path.exists() {
        anyhow::bail!("APK not found at: {}", apk_path.display());
    }

    Ok(apk_path)
}

// ─── Signing ─────────────────────────────────────────────────────────────────

async fn sign_artifact(android: &AndroidConfig, artifact_path: &Path) -> Result<PathBuf> {
    let ks_path = crate::config::expand_path(&android.keystore_path);
    let ks_password = crate::config::read_secret(&android.keystore_password_path)?;
    let key_password = if let Some(kp) = &android.key_password_path {
        crate::config::read_secret(kp)?
    } else {
        ks_password.clone()
    };

    let ext = artifact_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("aab");

    let signed_path = artifact_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("app-release-signed.{}", ext));

    // Try apksigner first (preferred for AAB and APK)
    if which::which("apksigner").is_ok() {
        sign_with_apksigner(
            &ks_path,
            &ks_password,
            &key_password,
            &android.keystore_alias,
            artifact_path,
            &signed_path,
        )
        .await?;
    } else {
        // Fallback to jarsigner
        sign_with_jarsigner(
            &ks_path,
            &ks_password,
            &key_password,
            &android.keystore_alias,
            artifact_path,
            &signed_path,
        )
        .await?;
    }

    Ok(signed_path)
}

async fn sign_with_apksigner(
    ks_path: &Path,
    ks_password: &str,
    key_password: &str,
    alias: &str,
    input: &Path,
    output: &Path,
) -> Result<()> {
    let spinner = progress::spinner("Signing with apksigner...");

    let status = Command::new("apksigner")
        .args([
            "sign",
            "--ks",
            &ks_path.to_string_lossy(),
            "--ks-key-alias",
            alias,
            "--ks-pass",
            &format!("pass:{}", ks_password),
            "--key-pass",
            &format!("pass:{}", key_password),
            "--out",
            &output.to_string_lossy(),
            &input.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .context("Failed to run apksigner")?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("apksigner failed with exit code {:?}", status.code());
    }

    Ok(())
}

async fn sign_with_jarsigner(
    ks_path: &Path,
    ks_password: &str,
    key_password: &str,
    alias: &str,
    input: &Path,
    output: &Path,
) -> Result<()> {
    // jarsigner signs in-place, so copy first
    std::fs::copy(input, output)
        .context("Failed to copy artifact for signing")?;

    let spinner = progress::spinner("Signing with jarsigner...");

    let status = Command::new("jarsigner")
        .args([
            "-verbose",
            "-sigalg",
            "SHA256withRSA",
            "-digestalg",
            "SHA-256",
            "-keystore",
            &ks_path.to_string_lossy(),
            "-storepass",
            ks_password,
            "-keypass",
            key_password,
            &output.to_string_lossy(),
            alias,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .context("Failed to run jarsigner")?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("jarsigner failed with exit code {:?}", status.code());
    }

    Ok(())
}
