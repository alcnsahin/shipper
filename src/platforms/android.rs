use anyhow::{Context, Result};
use console::style;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::config::{AndroidConfig, Config};
use crate::stores::playstore;
use crate::utils::progress;
use crate::utils::version::{self, AppVersion};

const TOTAL_STEPS: usize = 5;

pub async fn deploy(config: &Config) -> Result<AppVersion> {
    let android = config.android_config()?;
    let google = config.google_credentials()?;

    preflight_checks(android)?;

    println!("{}", style("Android Pipeline").bold().underlined());
    println!();

    // Step 1: Version bump
    progress::step(1, TOTAL_STEPS, "Bumping version");
    let app_version = bump_version(config, android)?;
    progress::success(&format!(
        "{} ({})",
        app_version.version_name, app_version.build_number
    ));

    // Step 2: Build
    let artifact_path = if android.build_type == "apk" {
        progress::step(2, TOTAL_STEPS, "Building APK with Gradle");
        let path = build_apk(android).await?;
        progress::success(&format!("APK: {}", path.display()));
        path
    } else {
        progress::step(2, TOTAL_STEPS, "Building AAB with Gradle");
        let path = build_aab(android).await?;
        progress::success(&format!("AAB: {}", path.display()));
        path
    };

    // Step 3: Sign
    progress::step(3, TOTAL_STEPS, "Signing artifact");
    let signed_path = sign_artifact(android, &artifact_path).await?;
    progress::success(&format!("Signed: {}", signed_path.display()));

    // Step 4: Upload to Play Store
    progress::step(4, TOTAL_STEPS, "Uploading to Play Store");
    let version_code = playstore::upload_aab(
        google,
        &android.package_name,
        &android.track,
        &signed_path,
    )
    .await?;
    progress::success(&format!("Uploaded (versionCode: {})", version_code));

    // Step 5: Done
    progress::step(5, TOTAL_STEPS, "Complete");
    progress::success(&format!(
        "{} v{} ({}) → {} track",
        android.package_name,
        app_version.version_name,
        app_version.build_number,
        android.track
    ));

    Ok(app_version)
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

    // Check keystore
    let ks_path = crate::config::expand_path(&android.keystore_path);
    if !ks_path.exists() {
        anyhow::bail!(
            "Keystore not found: {}",
            ks_path.display()
        );
    }

    // Check apksigner (part of Android Build Tools)
    which::which("apksigner").or_else(|_| which::which("jarsigner"))
        .context("Neither 'apksigner' nor 'jarsigner' found. Install Android SDK.")?;

    spinner.finish_and_clear();
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
