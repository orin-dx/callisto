//! Callisto's single disk-write primitive.
//!
//! This lives in Layer 1 (`callisto-model`) rather than alongside the manifest
//! editors because durable file replacement is not a manifest concern: the
//! changelog writer, the pre-mode state file, and the config scaffolder all
//! need it too. Hosting it here means those callers do not have to take a
//! dependency on `callisto-manifests` purely to reach the filesystem.

use std::io::{self, Write};
use std::path::Path;

use crate::ApplyPermit;
use tempfile::NamedTempFile;

/// Durably replaces `path`'s contents with `content`.
///
/// The [`ApplyPermit`] is unused at runtime and exists purely as a compile-time
/// obligation: this is callisto's single disk-write primitive, so requiring a
/// permit here means no code path can reach the filesystem without having first
/// consulted the dry-run flag. See [`crate::permit`].
///
/// # Errors
///
/// Returns the first I/O error encountered: creating parent directories, writing the temp file,
/// flushing, fsyncing, persisting (renaming) to `path`, or syncing the directory that holds it.
pub fn atomic_write(path: &Path, content: &str, permit: &ApplyPermit) -> io::Result<()> {
    let _permit = permit;
    let raw_parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if raw_parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        raw_parent
    };
    let created_parent = !parent.try_exists()?;
    std::fs::create_dir_all(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;

    sync_directory(parent)?;
    // A directory this call created is itself a new entry in its parent.
    if created_parent {
        if let Some(grandparent) = parent.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            sync_directory(grandparent)?;
        }
    }
    Ok(())
}

/// Flushes `dir`'s entries so a rename survives power loss. Windows cannot open a directory, and a filesystem that
/// cannot sync one reports `Unsupported` or `InvalidInput`; any other failure means the rename may not be durable.
fn sync_directory(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        match std::fs::File::open(dir)?.sync_all() {
            Err(error) if matches!(error.kind(), io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput) => Ok(()),
            result => result,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Fails now if [`atomic_write`] to `path` could not succeed, leaving no file behind.
/// A pre-existing regular file at `path` is accepted because the write replaces it.
///
/// # Errors
///
/// Returns the I/O error creating the parent directories or a temp file beside `path`, or
/// `IsADirectory`-style `InvalidInput` when `path` is a directory.
pub fn probe_atomic_write(path: &Path, permit: &ApplyPermit) -> io::Result<()> {
    let _permit = permit;
    if path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination is a directory",
        ));
    }
    let raw_parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if raw_parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        raw_parent
    };
    std::fs::create_dir_all(parent)?;
    NamedTempFile::new_in(parent).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApplyPermit;
    use tempfile::tempdir;

    #[test]
    fn probe_leaves_no_file_and_accepts_existing_target() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("new/receipt.json");
        let permit = ApplyPermit::force_for_tests();
        probe_atomic_write(&target, &permit).unwrap();
        assert_eq!(std::fs::read_dir(target.parent().unwrap()).unwrap().count(), 0);
        std::fs::write(&target, "x").unwrap();
        probe_atomic_write(&target, &permit).unwrap();
        assert!(probe_atomic_write(dir.path(), &permit).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_sync_failure_is_reported() {
        let dir = tempdir().unwrap();
        assert!(sync_directory(dir.path()).is_ok());
        let error = sync_directory(&dir.path().join("gone")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn atomic_write_creates_file_with_correct_content() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("output.txt");
        let content = "callisto atomic write test payload\n";

        atomic_write(&target, content, &ApplyPermit::force_for_tests()).unwrap();

        assert!(target.exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), content);
    }

    #[test]
    fn atomic_write_over_existing_file_replaces_content() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("Cargo.toml");
        let permit = ApplyPermit::force_for_tests();

        atomic_write(&target, "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n", &permit).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n"
        );

        atomic_write(&target, "[package]\nname = \"foo\"\nversion = \"1.0.1\"\n", &permit).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "[package]\nname = \"foo\"\nversion = \"1.0.1\"\n"
        );
    }

    #[test]
    fn atomic_write_creates_missing_parent_directories() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("deep/nested/dir/file.txt");

        atomic_write(&target, "content\n", &ApplyPermit::force_for_tests()).unwrap();

        assert!(target.exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "content\n");
    }

    /// The temp file is created inside the destination directory and consumed
    /// by `persist`, so a completed write must leave the directory holding
    /// exactly the target file — no `.tmpXXXXXX` siblings.
    #[test]
    fn atomic_write_leaves_no_temp_files_behind() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("nested/report.json");

        atomic_write(&target, "{}\n", &ApplyPermit::force_for_tests()).unwrap();

        let entries: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("report.json")]);
    }
}
