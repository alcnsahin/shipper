//! Opaque secret wrapper that redacts its contents in `Debug`/`Display`.
//!
//! The inner string is only accessible via the explicit [`Secret::expose`]
//! method, which makes accidental leaks into logs, `anyhow` error chains, or
//! `tracing` spans a compile-time-visible operation rather than a silent one.
//!
//! Secrets are loaded via [`Secret::read_from_file`], which additionally
//! refuses to read files whose POSIX mode grants any group or world bits —
//! `chmod 600` (owner read/write only) is required.

use anyhow::{Context, Result};
use std::fmt;
use std::path::Path;

/// A string containing sensitive material (tokens, passwords, private keys).
///
/// Use [`Secret::expose`] only at the final boundary where the secret must
/// leave the process (HTTP request body, subprocess stdin, etc.).
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wrap an in-memory string as a secret. Prefer [`Secret::read_from_file`]
    /// whenever the value originates on disk so that mode checks apply.
    ///
    /// Currently only used by tests; non-file constructors (env vars, Keychain)
    /// will wire in through this entry point in later phases.
    #[cfg(test)]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Read the secret from a file, trimming trailing whitespace.
    ///
    /// On Unix targets the file mode must be `0o600` or stricter (no group
    /// or world bits). A permissive mode is treated as a hard error with a
    /// `chmod 600 <path>` remediation hint, because a leaked keystore
    /// password or App Store Connect key is not something to recover from
    /// with a warning.
    pub fn read_from_file(path: &Path) -> Result<Self> {
        enforce_owner_only_mode(path)?;
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read secret from {}", path.display()))?;
        Ok(Self(content.trim().to_string()))
    }

    /// Reveal the raw secret. Callers should keep the returned `&str`
    /// short-lived and must not log or format it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(unix)]
fn enforce_owner_only_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat secret file {}", path.display()))?;
    let mode = meta.mode() & 0o777;
    if mode & 0o077 != 0 {
        anyhow::bail!(
            "Secret file {} has permissive mode {:04o}; run `chmod 600 {}` to restrict it",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn enforce_owner_only_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn debug_and_display_redact() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{:?}", s), "Secret([redacted])");
        assert_eq!(format!("{}", s), "[redacted]");
        assert_eq!(s.expose(), "hunter2");
    }

    #[cfg(unix)]
    #[test]
    fn read_from_file_trims_and_accepts_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("token");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"abc\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

        let s = Secret::read_from_file(&p).unwrap();
        assert_eq!(s.expose(), "abc");
    }

    #[cfg(unix)]
    #[test]
    fn read_from_file_rejects_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("token");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"abc\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = Secret::read_from_file(&p).unwrap_err().to_string();
        assert!(err.contains("chmod 600"), "error was: {err}");
    }
}
