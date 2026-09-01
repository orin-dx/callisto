//! Process-scoped advisory locks for durable release execution.
//!
//! The lock is operational evidence, never part of a serialized intent. It
//! prevents cooperating Callisto processes from validating and executing the
//! same checkout concurrently; Git trust is still re-observed before effects.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::VcsError;

/// An exclusive OS-backed advisory lock held for one canonical workspace.
#[derive(Debug)]
pub struct ReleaseWorkspaceLock {
    file: File,
    path: PathBuf,
}

impl ReleaseWorkspaceLock {
    /// Acquires the workspace lock outside the checkout.
    ///
    /// `state_directory` exists for CI and tests. Without it, the lock lives
    /// under the platform's application-state location.
    pub fn acquire(canonical_root: &Path, state_directory: Option<&Path>) -> Result<Self, VcsError> {
        let path = lock_path(canonical_root, state_directory)?;
        let parent = path.parent().expect("lock path always has a parent");
        fs::create_dir_all(parent)
            .map_err(|_error| VcsError::Git("could not create release state directory".to_string()))?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|_error| VcsError::Git("could not open release workspace lock".to_string()))?;
        file.try_lock_exclusive().map_err(|_error| {
            VcsError::Git("another Callisto release already holds this workspace lock".to_string())
        })?;
        Ok(Self { file, path })
    }

    /// Returns the operational lock location for diagnostics and tests.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ReleaseWorkspaceLock {
    fn drop(&mut self) {
        drop(self.file.unlock());
    }
}

fn lock_path(canonical_root: &Path, state_directory: Option<&Path>) -> Result<PathBuf, VcsError> {
    let base = match state_directory {
        Some(path) => path.to_path_buf(),
        None => platform_state_directory()?,
    };
    let digest = Sha256::digest(canonical_root.to_string_lossy().as_bytes());
    let key = format!("{:x}", digest);
    Ok(base.join("callisto").join("release-locks").join(format!("{key}.lock")))
}

/// Resolves Callisto's platform application-state base directory.
///
/// This is operational routing only; callers must still key state by a
/// canonical workspace and durable intent rather than treating the location
/// as release authority.
pub fn platform_state_directory() -> Result<PathBuf, VcsError> {
    #[cfg(target_os = "macos")]
    let candidate = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let candidate = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(all(unix, not(target_os = "macos")))]
    let candidate = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    candidate.ok_or_else(|| VcsError::Git("could not determine platform release state directory".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_releases_when_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        let first = ReleaseWorkspaceLock::acquire(&root, Some(directory.path())).unwrap();
        assert!(ReleaseWorkspaceLock::acquire(&root, Some(directory.path())).is_err());
        let path = first.path().to_path_buf();
        drop(first);
        let second = ReleaseWorkspaceLock::acquire(&root, Some(directory.path())).unwrap();
        assert_eq!(second.path(), path);
    }
}
