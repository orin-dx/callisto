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

/// Trait for durable changeset and manifest storage operations.
pub trait ChangesetStorage {
    /// Writes content atomically with parent and grandparent directory journal flushing.
    ///
    /// # Errors
    ///
    /// Returns `Err` on any I/O failure: directory creation, temp-file creation, write, sync, or persist.
    fn atomic_write_durable(&self, content: &str, permit: &ApplyPermit) -> io::Result<()>;
}

impl ChangesetStorage for Path {
    fn atomic_write_durable(&self, content: &str, permit: &ApplyPermit) -> io::Result<()> {
        atomic_write(self, content, permit)
    }
}

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
/// flushing, fsyncing, or persisting (renaming) to `path`.
pub fn atomic_write(path: &Path, content: &str, permit: &ApplyPermit) -> io::Result<()> {
    let _permit = permit;
    let raw_parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if raw_parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        raw_parent
    };
    std::fs::create_dir_all(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;

    if let Ok(parent_file) = std::fs::File::open(parent) {
        let _res = parent_file.sync_all();
    }
    if let Some(grandparent) = parent.parent() {
        if !grandparent.as_os_str().is_empty() {
            if let Ok(gp_file) = std::fs::File::open(grandparent) {
                let _res = gp_file.sync_all();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApplyPermit;
    use tempfile::tempdir;

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

    /// `ChangesetStorage` is the method-call spelling of the same primitive;
    /// it must land the identical bytes at the identical path.
    #[test]
    fn changeset_storage_trait_writes_through_to_atomic_write() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("CHANGELOG.md");

        target
            .as_path()
            .atomic_write_durable("# pkg\n", &ApplyPermit::force_for_tests())
            .unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "# pkg\n");
    }
}
