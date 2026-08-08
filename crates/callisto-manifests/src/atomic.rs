use std::io::{self, Write};
use std::path::Path;

use callisto_model::ApplyPermit;
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
/// consulted the dry-run flag. See [`callisto_model::permit`].
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
    use callisto_model::ApplyPermit;
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

        atomic_write(
            &target,
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n",
            &permit,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n"
        );

        atomic_write(
            &target,
            "[package]\nname = \"foo\"\nversion = \"1.0.1\"\n",
            &permit,
        )
        .unwrap();
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

    /// Spec: concurrent `atomic_write` calls targeting the same path from
    /// multiple threads must not corrupt the file. The last writer wins
    /// (last-write-wins semantics), but the file must exist after all threads
    /// finish and its content must be exactly one complete, untruncated write
    /// -- never an interleaved mix of bytes from two concurrent writes.
    ///
    /// `atomic_write` achieves this by writing to a fresh `NamedTempFile` and
    /// then atomically renaming it into place, which is guaranteed to be an
    /// atomic replace on POSIX filesystems.
    #[test]
    fn concurrent_atomic_write_to_same_path_does_not_corrupt_file() {
        let dir = tempdir().unwrap();
        let target = std::sync::Arc::new(dir.path().join("shared.txt"));

        const THREAD_COUNT: usize = 10;
        let permit = std::sync::Arc::new(ApplyPermit::force_for_tests());

        let handles: Vec<_> = (0..THREAD_COUNT)
            .map(|i| {
                let path = std::sync::Arc::clone(&target);
                let p = std::sync::Arc::clone(&permit);
                std::thread::spawn(move || {
                    let content = format!("thread-{i}\n");
                    atomic_write(&path, &content, &p).expect("atomic_write must not fail");
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread must not panic");
        }

        // The file must exist.
        assert!(target.exists(), "file must exist after all threads finish");

        // The file must contain exactly one complete thread's content.
        let final_content = std::fs::read_to_string(target.as_ref()).unwrap();
        assert!(
            !final_content.is_empty(),
            "file content must not be empty after concurrent writes"
        );
        assert!(
            (0..THREAD_COUNT).any(|i| final_content == format!("thread-{i}\n")),
            "file content must be exactly one valid thread write, got: {:?}",
            final_content
        );
    }
}
