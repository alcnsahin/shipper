use anyhow::Result;
use console::style;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Credential filenames (non-keystore) that belong under ios/keys/
const IOS_CREDENTIAL_FILENAMES: &[&str] = &["credentials.json"];

/// Scans the current directory for stray credential/keystore files and moves them
/// into the correct project-scoped subdirectory under `~/.shipper/{project_name}/`:
///
/// - `*.keystore`      → `~/.shipper/{project_name}/android/keys/`
/// - `credentials.json`→ `~/.shipper/{project_name}/ios/keys/`
///
/// If the destination already exists, a timestamped `.bak` copy is created before
/// overwriting. When a moved keystore matches the `android_keystore_path` in
/// `shipper.toml`, the file is updated on disk so subsequent runs resolve the new
/// location correctly.
pub fn migrate_stray_credentials(
    project_name: &str,
    android_keystore_path: Option<&str>,
) -> Result<()> {
    let android_keys_dir = project_android_keys_dir(project_name);
    let ios_keys_dir = project_ios_keys_dir(project_name);

    let stray_files = collect_stray_files(Path::new("."))?;
    if stray_files.is_empty() {
        return Ok(());
    }

    println!(
        "  {} Found stray credential files — migrating to ~/.shipper/{}/",
        style("→").bold().cyan(),
        project_name
    );

    std::fs::create_dir_all(&android_keys_dir)?;
    std::fs::create_dir_all(&ios_keys_dir)?;

    for src in &stray_files {
        let filename = src
            .file_name()
            .expect("file always has a name")
            .to_string_lossy();

        let dest_dir = if is_ios_credential(&filename) {
            &ios_keys_dir
        } else {
            &android_keys_dir
        };

        let dest = dest_dir.join(filename.as_ref());

        // Backup existing destination file before overwriting
        if dest.exists() {
            let ts = timestamp();
            let backup = dest_dir.join(format!("{}.bak.{}", filename, ts));
            std::fs::copy(&dest, &backup)?;
            println!(
                "    {} backed up existing {} → {}",
                style("↩").dim(),
                filename,
                backup.file_name().unwrap().to_string_lossy()
            );
        }

        std::fs::copy(src, &dest)?;
        std::fs::remove_file(src)?;

        println!(
            "    {} {} → {}",
            style("✓").green(),
            src.display(),
            dest.display()
        );

        // If this keystore is the one referenced in shipper.toml, update the path.
        if let Some(configured_path) = android_keystore_path {
            if is_same_keystore(src, configured_path) {
                let new_path = dest.to_string_lossy();
                if let Err(e) = update_shipper_toml_keystore_path(configured_path, &new_path) {
                    println!(
                        "    {} Could not update shipper.toml automatically: {}",
                        style("!").yellow(),
                        e
                    );
                    println!(
                        "    {} Please update keystore_path to: {}",
                        style("!").yellow(),
                        new_path
                    );
                } else {
                    println!(
                        "    {} Updated shipper.toml keystore_path → {}",
                        style("✓").green(),
                        new_path
                    );
                }
            }
        }
    }

    println!();
    Ok(())
}

// ─── Directory helpers ────────────────────────────────────────────────────────

pub fn project_android_keys_dir(project_name: &str) -> PathBuf {
    shipper_home().join(project_name).join("android").join("keys")
}

pub fn project_ios_keys_dir(project_name: &str) -> PathBuf {
    shipper_home().join(project_name).join("ios").join("keys")
}

fn shipper_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".shipper")
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn is_ios_credential(filename: &str) -> bool {
    IOS_CREDENTIAL_FILENAMES.contains(&filename)
}

fn collect_stray_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let is_keystore = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "keystore")
            .unwrap_or(false);

        let is_ios_cred = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| IOS_CREDENTIAL_FILENAMES.contains(&n))
            .unwrap_or(false);

        if is_keystore || is_ios_cred {
            found.push(path);
        }
    }

    Ok(found)
}

/// Returns true if `src` (relative path in project root) resolves to the same
/// file as `configured_path` from shipper.toml (which may contain `~/`).
fn is_same_keystore(src: &Path, configured_path: &str) -> bool {
    let configured = crate::config::expand_path(configured_path);

    let src_canon = src.canonicalize().ok();
    let cfg_canon = configured.canonicalize().ok();

    if let (Some(a), Some(b)) = (src_canon, cfg_canon) {
        return a == b;
    }

    // Fallback: compare filenames
    src.file_name() == configured.file_name()
}

/// Replaces the `keystore_path` value in `shipper.toml` on disk.
fn update_shipper_toml_keystore_path(old_path: &str, new_path: &str) -> Result<()> {
    let toml_path = PathBuf::from("shipper.toml");
    let content = std::fs::read_to_string(&toml_path)?;

    let old_quoted = format!("\"{}\"", old_path);
    let new_quoted = format!("\"{}\"", new_path);

    if !content.contains(&old_quoted) {
        anyhow::bail!(
            "keystore_path \"{}\" not found in shipper.toml — please update it manually to: {}",
            old_path,
            new_path
        );
    }

    let updated = content.replacen(&old_quoted, &new_quoted, 1);
    std::fs::write(&toml_path, updated)?;
    Ok(())
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
