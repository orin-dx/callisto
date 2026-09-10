//! Process-scoped advisory locks for durable release execution.
//!
//! The lock is operational evidence, never part of a serialized intent. It
//! prevents cooperating Callisto processes from validating and executing the
//! same checkout concurrently; Git trust is still re-observed before effects.

#[cfg(not(target_arch = "wasm32"))]
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use fs2::FileExt;
#[cfg(not(target_arch = "wasm32"))]
use sha2::{Digest, Sha256};

use crate::VcsError;

/// An exclusive OS-backed advisory lock held for one canonical workspace.
#[derive(Debug)]
pub struct ReleaseWorkspaceLock {
    #[cfg(not(target_arch = "wasm32"))]
    file: File,
    path: PathBuf,
}

impl ReleaseWorkspaceLock {
    /// Acquires the workspace lock outside the checkout.
    ///
    /// `state_directory` exists for CI and tests. Without it, the lock lives
    /// under the platform's application-state location.
    pub fn acquire(canonical_root: &Path, state_directory: Option<&Path>) -> Result<Self, VcsError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (canonical_root, state_directory);
            return Err(VcsError::Git(
                "durable release execution is unavailable on wasm32".to_string(),
            ));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
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
            file.try_lock_exclusive().map_err(classify_lock_acquire_error)?;
            Ok(Self { file, path })
        }
    }

    /// Returns the operational lock location for diagnostics and tests.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ReleaseWorkspaceLock {
    #[cfg(not(target_arch = "wasm32"))]
    fn drop(&mut self) {
        drop(self.file.unlock());
    }
    #[cfg(target_arch = "wasm32")]
    fn drop(&mut self) {}
}

/// Classifies a [`fs2::FileExt::try_lock_exclusive`] failure as either the
/// expected "another process already holds this lock" contention (matched
/// against [`fs2::lock_contended_error`]'s `ErrorKind`, the cross-platform
/// signal fs2 itself uses to identify contention) or a genuine I/O failure
/// (permission denied, disk full, filesystem lacks locking support, etc.),
/// which is reported with its real cause instead of being misattributed to
/// a held lock.
#[cfg(not(target_arch = "wasm32"))]
fn classify_lock_acquire_error(error: std::io::Error) -> VcsError {
    if error.kind() == fs2::lock_contended_error().kind() {
        VcsError::Git("another Callisto release already holds this workspace lock".to_string())
    } else {
        VcsError::Git(format!("could not acquire release workspace lock: {error}"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(target_arch = "wasm32")]
    {
        return Err(VcsError::Git(
            "durable release execution is unavailable on wasm32".to_string(),
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
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

    /// A genuinely held lock (the true contention case) must still be
    /// reported as "another release holds the lock" -- the only case for
    /// which that message is accurate.
    #[test]
    fn actually_held_lock_reports_another_release_holds_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace-held");
        std::fs::create_dir(&root).unwrap();
        let _first = ReleaseWorkspaceLock::acquire(&root, Some(directory.path())).unwrap();

        let err = ReleaseWorkspaceLock::acquire(&root, Some(directory.path())).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("another Callisto release already holds this workspace lock"),
            "expected the true contention message, got: {message}"
        );
    }

    /// A non-contention I/O failure (permission denied, disk full, no lock
    /// support, etc.) must report its own real cause, not be force-fit into
    /// the "another release holds the lock" message.
    #[test]
    fn non_contention_io_error_reports_its_own_cause_not_another_holder() {
        let permission_error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let vcs_error = classify_lock_acquire_error(permission_error);
        let message = vcs_error.to_string();
        assert!(
            !message.contains("another Callisto release already holds this workspace lock"),
            "a permission error must not be misreported as another holder, got: {message}"
        );
        assert!(
            message.contains("permission denied"),
            "expected the real cause in the message, got: {message}"
        );
    }
}
