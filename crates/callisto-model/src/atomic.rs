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
    write_durably(path, content, true, permit)
}

/// [`atomic_write`] that never replaces: it fails with `AlreadyExists` if anything is at `path` when the file is put in
/// place, including a file created by another process after this call began. The content is durable before it appears.
///
/// # Errors
///
/// `AlreadyExists` when `path` is taken, otherwise the same errors as [`atomic_write`].
pub fn atomic_write_new(path: &Path, content: &str, permit: &ApplyPermit) -> io::Result<()> {
    write_durably(path, content, false, permit)
}

fn write_durably(path: &Path, content: &str, replace: bool, permit: &ApplyPermit) -> io::Result<()> {
    let _permit = permit;
    let raw_parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if raw_parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        raw_parent
    };
    let created_parent = !parent.try_exists()?;
    std::fs::create_dir_all(parent)?;

    let mut temp = temp_file_in(parent)?;
    if replace {
        match_existing_mode(temp.as_file(), path)?;
    }
    temp.write_all(content.as_bytes())?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    if replace {
        temp.persist(path).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|e| e.error)?;
    }

    sync_directory(parent)?;
    // A directory this call created is itself a new entry in its parent.
    if created_parent && let Some(grandparent) = parent.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        sync_directory(grandparent)?;
    }
    Ok(())
}

/// A temp file whose mode follows the process umask like any new file, instead of `tempfile`'s owner-only `0600`.
fn temp_file_in(dir: &Path) -> io::Result<NamedTempFile> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o666))
            .tempfile_in(dir)
    }
    #[cfg(not(unix))]
    {
        NamedTempFile::new_in(dir)
    }
}

/// Gives the temp file the mode of the file it replaces; a new file keeps the umask-derived mode.
fn match_existing_mode(temp: &std::fs::File, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(target) {
            Ok(existing) => {
                temp.set_permissions(std::fs::Permissions::from_mode(existing.permissions().mode() & 0o7777))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (temp, target);
        Ok(())
    }
}

/// Flushes `dir`'s entries so a rename survives power loss. A directory that may not be opened for reading (a `-wx`
/// mode, `PermissionDenied`) or a filesystem that cannot sync one (`Unsupported`, `InvalidInput`, macOS `ENOTSUP`) is
/// skipped: the file is already in place and failing the command now would misreport a write that happened. Any other
/// failure, whether opening or syncing, an I/O error above all, means the rename may not be durable and is reported.
fn sync_directory(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let handle = match std::fs::File::open(dir) {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied || sync_unsupported(&error) => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match handle.sync_all() {
            Err(error) if sync_unsupported(&error) => Ok(()),
            result => result,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Fails now if [`atomic_write`] to `path` could not succeed, leaving nothing behind: no file and no directory.
/// A pre-existing regular file at `path` is accepted because the write replaces it. When the parent directory does not
/// exist yet, the nearest one that does must take a new file, which is what creating the missing ones needs.
///
/// # Errors
///
/// Returns the I/O error creating a temp file beside `path` (or in its nearest existing parent), or
/// `IsADirectory`-style `InvalidInput` when `path` is a directory or a parent is not.
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
    let existing = parent
        .ancestors()
        .find(|ancestor| ancestor.try_exists().unwrap_or(false))
        .unwrap_or_else(|| Path::new("."));
    if !existing.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a parent of the destination is not a directory",
        ));
    }
    NamedTempFile::new_in(existing)?;
    sync_directory(existing)
}

/// `std` maps `EOPNOTSUPP` to `Unsupported` but leaves macOS's distinct `ENOTSUP` (45) uncategorized.
#[cfg(unix)]
fn sync_unsupported(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput)
        || (cfg!(target_os = "macos") && error.raw_os_error() == Some(45))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApplyPermit;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn a_rewritten_file_keeps_its_mode_and_a_new_file_follows_the_umask() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let permit = ApplyPermit::force_for_tests();
        let mode_of = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;

        for mode in [0o755, 0o640, 0o600] {
            let existing = dir.path().join(format!("existing-{mode:o}"));
            std::fs::write(&existing, "old").unwrap();
            std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(mode)).unwrap();
            atomic_write(&existing, "new", &permit).unwrap();
            assert_eq!(std::fs::read_to_string(&existing).unwrap(), "new");
            assert_eq!(mode_of(&existing), mode);
        }

        let created = dir.path().join("nested/created");
        atomic_write(&created, "x", &permit).unwrap();
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "x").unwrap();
        assert_eq!(mode_of(&created), mode_of(&plain));
    }

    #[test]
    fn atomic_write_new_never_replaces_what_is_there() {
        let dir = tempdir().unwrap();
        let permit = ApplyPermit::force_for_tests();
        let target = dir.path().join("changeset.md");
        atomic_write_new(&target, "first", &permit).unwrap();
        let error = atomic_write_new(&target, "second", &permit).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "first");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "a refused write leaves no temp file behind"
        );
    }

    /// Many writers choosing the same name at once: exactly one wins, the rest are told, and nobody is overwritten.
    #[test]
    fn concurrent_exclusive_writes_to_one_name_have_one_winner() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("changeset.md");
        let outcomes: Vec<_> = std::thread::scope(|scope| {
            (0..16)
                .map(|writer| {
                    let target = &target;
                    scope.spawn(move || {
                        atomic_write_new(target, &format!("writer {writer}"), &ApplyPermit::force_for_tests())
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert!(outcomes
            .iter()
            .filter_map(|outcome| outcome.as_ref().err())
            .all(|error| error.kind() == io::ErrorKind::AlreadyExists));
        assert!(std::fs::read_to_string(&target).unwrap().starts_with("writer "));
    }

    #[test]
    fn probe_leaves_no_file_or_directory_and_accepts_existing_target() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("new/receipt.json");
        let permit = ApplyPermit::force_for_tests();
        probe_atomic_write(&target, &permit).unwrap();
        assert!(
            !target.parent().unwrap().exists(),
            "the probe must not create directories"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "x").unwrap();
        probe_atomic_write(&target, &permit).unwrap();
        assert!(probe_atomic_write(dir.path(), &permit).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_opened_or_synced_is_skipped_but_an_io_failure_is_reported() {
        let dir = tempdir().unwrap();
        assert!(sync_directory(dir.path()).is_ok());
        // A directory that should hold the file and cannot be found is not a skippable limitation.
        assert!(sync_directory(&dir.path().join("gone")).is_err());
        assert!(sync_unsupported(&io::Error::from(io::ErrorKind::Unsupported)));
        assert!(sync_unsupported(&io::Error::from(io::ErrorKind::InvalidInput)));
        #[cfg(target_os = "macos")]
        assert!(sync_unsupported(&io::Error::from_raw_os_error(45)));
        // EIO: the rename may not be durable, so it is reported.
        assert!(!sync_unsupported(&io::Error::from_raw_os_error(5)));
    }

    /// A directory the process may write to but not read (`-wx`) is skipped, as the file is in place regardless.
    #[cfg(unix)]
    #[test]
    fn a_write_only_directory_is_skipped_not_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let write_only = dir.path().join("wx");
        std::fs::create_dir(&write_only).unwrap();
        std::fs::set_permissions(&write_only, std::fs::Permissions::from_mode(0o300)).unwrap();
        let result = sync_directory(&write_only);
        std::fs::set_permissions(&write_only, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_ok(), "{result:?}");
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
