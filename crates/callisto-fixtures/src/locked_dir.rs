//! A directory whose contents cannot be searched, to prove a failed look is not read as "absent".

use std::path::{Path, PathBuf};

/// Restores the directory's permissions on drop.
pub struct LockedDir {
    path: PathBuf,
}

impl LockedDir {
    /// Locks `path` (mode 000), or returns `None` where permission bits do not bind (root, non-Unix).
    pub fn lock(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).ok()?;
            let guard = Self {
                path: path.to_path_buf(),
            };
            // A listing that still works means the permission bits are not enforced for this user.
            if std::fs::read_dir(path).is_ok() {
                return None;
            }
            Some(guard)
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }
}

impl Drop for LockedDir {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            drop(std::fs::set_permissions(
                &self.path,
                std::fs::Permissions::from_mode(0o755),
            ));
        }
        #[cfg(not(unix))]
        {
            let _ = &self.path;
        }
    }
}
