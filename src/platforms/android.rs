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

    // Check for a previously built signed artifact and offer to skip the build
    if let Some(existing) = find_existing_signed_artifact(android) {
        if prompt_reuse_artifact(&existing)? {
            let version = read_current_version(android)?;
            progress::step(5, TOTAL_STEPS, "Uploading to Play Store");
            let version_code = playstore::upload_aab(
                google,
                &android.package_name,
                &android.track,
                &existing,
            )
            .await?;
            progress::success(&format!("Uploaded (versionCode: {})", version_code));
            progress::step(6, TOTAL_STEPS, "Complete");
            progress::success(&format!(
                "{} v{} ({}) → {} track",
                android.package_name,
                version.version_name,
                version.build_number,
                android.track
            ));
            return Ok(version);
        }
    }

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
    patch_gradle_properties(android)?;

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
        "  {} Keystore not found at {}",
        style("!").yellow().bold(),
        ks_path.display()
    );
    println!();
    println!(
        "  {} {}",
        style("WARNING:").red().bold(),
        style("Generating a NEW keystore will produce a different signing fingerprint.").bold()
    );
    println!(
        "  {}",
        style("If your app is already published on Play Store, uploading a build signed").dim()
    );
    println!(
        "  {}",
        style("with a new keystore will be REJECTED.").dim()
    );
    println!();
    print!("  Continue and generate a new keystore? [y/N] ");
    io::stdout().flush()?;
    let mut confirm = String::new();
    io::stdin().read_line(&mut confirm)?;
    match confirm.trim().to_lowercase().as_str() {
        "y" | "yes" => {}
        _ => {
            anyhow::bail!(
                "Keystore generation cancelled. \
                Place the correct keystore at {} and re-run.",
                ks_path.display()
            );
        }
    }
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

// ─── Existing artifact detection ─────────────────────────────────────────────

fn find_existing_signed_artifact(android: &AndroidConfig) -> Option<PathBuf> {
    let ext = if android.build_type == "apk" { "apk" } else { "aab" };
    let path = Path::new(&android.project_dir)
        .join("app/build/outputs")
        .join(if ext == "aab" { "bundle/release" } else { "apk/release" })
        .join(format!("app-release-signed.{}", ext));
    if path.exists() { Some(path) } else { None }
}

fn prompt_reuse_artifact(path: &Path) -> Result<bool> {
    println!(
        "  {} Found existing signed artifact: {}",
        style("?").cyan().bold(),
        path.display()
    );
    print!("  {} Skip rebuild and upload this? [Y/n] ", style("→").cyan());
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}

fn read_current_version(android: &AndroidConfig) -> Result<AppVersion> {
    if version::is_expo_project() {
        return version::read_expo_version(Path::new("app.json"));
    }
    let gradle_path = Path::new(&android.project_dir)
        .join("app")
        .join("build.gradle");
    version::read_gradle_version(&gradle_path)
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

// ─── Patch Gradle files ───────────────────────────────────────────────────────
// expo prebuild regenerates gradle.properties and gradle-wrapper.properties,
// so apply compatibility fixes after it runs.

fn patch_gradle_properties(_android: &AndroidConfig) -> Result<()> {
    Ok(())
}

// ─── JDK detection ────────────────────────────────────────────────────────────
// React Native requires JDK 17 or 21. JDK 22+ is not compatible with the Gradle
// versions that work with RN's native CMake builds.

fn resolve_java_home() -> Result<String> {
    if let Some(home) = find_compat_java_home() {
        progress::info(&format!("Java: {}", home));
        return Ok(home);
    }
    anyhow::bail!(
        "JDK 17 or 21 not found. React Native requires JDK 17 or 21.\n\
         Install with: brew install --cask temurin@21\n\
         Then retry."
    )
}

fn find_compat_java_home() -> Option<String> {
    // Prefer JDK 21, fall back to 17
    for version in ["21", "17"] {
        if let Some(home) = try_java_home(version) {
            return Some(home);
        }
    }
    None
}

fn try_java_home(major: &str) -> Option<String> {
    // macOS: /usr/libexec/java_home -v <major> returns "major or higher",
    // so we must verify the actual major version of the returned path.
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/libexec/java_home")
            .arg("-v")
            .arg(major)
            .output()
            .ok()?;
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() && java_major_version(&path).as_deref() == Some(major) {
                return Some(path);
            }
        }
    }

    // Linux: check common paths
    #[cfg(target_os = "linux")]
    {
        let candidates = [
            format!("/usr/lib/jvm/java-{}-openjdk-amd64", major),
            format!("/usr/lib/jvm/java-{}-openjdk", major),
            format!("/usr/lib/jvm/temurin-{}", major),
            format!("/usr/lib/jvm/java-{}", major),
        ];
        for path in &candidates {
            if std::path::Path::new(path).join("bin/java").exists() {
                return Some(path.clone());
            }
        }
    }

    None
}

/// Read the major Java version from `$JAVA_HOME/release` (e.g. "21" from JAVA_VERSION="21.0.4").
fn java_major_version(java_home: &str) -> Option<String> {
    let release = std::fs::read_to_string(
        std::path::Path::new(java_home).join("release")
    ).ok()?;
    for line in release.lines() {
        if line.starts_with("JAVA_VERSION=") {
            // JAVA_VERSION="21.0.4"  or  JAVA_VERSION=17
            let val = line.trim_start_matches("JAVA_VERSION=").trim_matches('"');
            let major = val.split('.').next()?;
            return Some(major.to_string());
        }
    }
    None
}

// ─── Gradle build ─────────────────────────────────────────────────────────────

async fn build_aab(android: &AndroidConfig) -> Result<PathBuf> {
    let project_dir = Path::new(&android.project_dir);

    let java_home = resolve_java_home()?;

    let spinner = progress::spinner("./gradlew bundleRelease (this can take a few minutes)...");

    let mut cmd = Command::new("./gradlew");
    cmd.arg("bundleRelease")
        .current_dir(project_dir)
        .env("JAVA_HOME", &java_home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = cmd.output()
        .await
        .context("Failed to run gradlew bundleRelease")?;

    spinner.finish_and_clear();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Gradle prints the failure summary at the end of stdout; show the last 60 lines
        let combined = format!("{}\n{}", stdout, stderr);
        let all_lines: Vec<&str> = combined.lines().collect();
        let tail: Vec<&str> = all_lines.iter().rev().take(60).copied().collect::<Vec<_>>().into_iter().rev().collect();
        anyhow::bail!("Gradle build failed:\n{}", tail.join("\n"));
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

    let java_home = resolve_java_home()?;

    let spinner = progress::spinner("./gradlew assembleRelease...");

    let output = Command::new("./gradlew")
        .arg("assembleRelease")
        .current_dir(project_dir)
        .env("JAVA_HOME", &java_home)
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

    let is_aab = ext == "aab";

    if is_aab {
        // apksigner does not support AAB files.
        // Gradle's bundleRelease may have already signed the AAB (debug or release key).
        // Strip any existing JAR signatures first so we end up with exactly one
        // certificate chain signed by the correct release keystore.
        sign_aab_release(
            &ks_path,
            &ks_password,
            &key_password,
            &android.keystore_alias,
            artifact_path,
            &signed_path,
        )
        .await?;
        return Ok(signed_path);
    }

    // APK: try apksigner first (preferred), fall back to jarsigner
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

/// Signs an AAB with the release keystore using jarsigner.
///
/// Any existing JAR signatures (e.g. Gradle debug key, prior release key) are
/// stripped before signing so Play Store sees exactly one certificate chain.
async fn sign_aab_release(
    ks_path: &Path,
    ks_password: &str,
    key_password: &str,
    alias: &str,
    input: &Path,
    output: &Path,
) -> Result<()> {
    std::fs::copy(input, output).context("Failed to copy AAB for signing")?;

    // Strip any META-INF signature files to avoid multiple certificate chains.
    // Errors are intentionally ignored — the AAB may already be unsigned.
    let _ = Command::new("zip")
        .args([
            "-d",
            &output.to_string_lossy(),
            "META-INF/*.SF",
            "META-INF/*.RSA",
            "META-INF/*.DSA",
            "META-INF/*.EC",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    let spinner = progress::spinner("Signing AAB with jarsigner...");

    let status = Command::new("jarsigner")
        .args([
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
        anyhow::bail!("jarsigner failed to sign AAB");
    }

    Ok(())
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
