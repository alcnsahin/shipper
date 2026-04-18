//! File-based deploy lock to prevent concurrent `shipper deploy` runs
//! for the same project.
//!
//! The lock file lives at `~/.shipper/<project>/deploy.lock` and contains
//! the PID of the holder. On acquisition the PID is checked — if the
//! process is gone the stale lock is replaced. The lock is released on
//! [`Drop`] so it is cleaned up even on early `?` returns.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// RAII deploy lock. Removing the file on [`Drop`] releases the lock.
pub(crate) struct DeployLock {
    path: PathBuf,
}

impl DeployLock {
    /// Try to acquire the deploy lock for `project_name`.
    ///
    /// Returns `Ok(lock)` if the lock was acquired, or an error if another
    /// `shipper deploy` process is already running for this project.
    pub(crate) fn acquire(project_name: &str) -> Result<Self> {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".shipper")
            .join(project_name);
        fs::create_dir_all(&dir)?;

        let path = dir.join("deploy.lock");

        // Check for an existing lock.
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(pid) = content.trim().parse::<u32>() {
                    if is_process_alive(pid) {
                        anyhow::bail!(
                            "Another shipper deploy is already running for this project (PID {pid}).\n\
                             If you're sure no other deploy is running, remove:\n  {}",
                            path.display()
                        );
                    }
                    // Stale lock — process is gone.
                    tracing::debug!(pid, "removing stale deploy lock");
                }
            }
        }

        // Write our PID.
        let my_pid = std::process::id();
        fs::write(&path, my_pid.to_string())
            .with_context(|| format!("Failed to write deploy lock: {}", path.display()))?;

        Ok(Self { path })
    }
}

impl Drop for DeployLock {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

fn is_process_alive(pid: u32) -> bool {
    // `kill -0 <pid>` checks existence without sending a signal.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let lock = DeployLock::acquire("_test_lock_project").unwrap();
        assert!(lock.path.exists());
        let content = fs::read_to_string(&lock.path).unwrap();
        assert_eq!(content.trim(), std::process::id().to_string());

        let path = lock.path.clone();
        drop(lock);
        assert!(!path.exists());
    }

    #[test]
    fn double_acquire_fails() {
        let lock = DeployLock::acquire("_test_lock_double").unwrap();
        let result = DeployLock::acquire("_test_lock_double");
        assert!(result.is_err());
        drop(lock);
    }

    #[test]
    fn stale_lock_is_replaced() {
        // Write a lock file with a PID that doesn't exist.
        let dir = dirs::home_dir().unwrap().join(".shipper/_test_lock_stale");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deploy.lock");
        fs::write(&path, "999999999").unwrap(); // Very unlikely to be alive.

        let lock = DeployLock::acquire("_test_lock_stale").unwrap();
        assert!(lock.path.exists());
        drop(lock);
    }
}
