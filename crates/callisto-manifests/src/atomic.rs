//! Backward-compatible alias for callisto's disk-write primitive.
//!
//! The implementation moved to [`callisto_model::atomic`] so Layer 1 consumers
//! (the changelog writer in particular) can reach the filesystem without
//! depending on this crate. `callisto_manifests::atomic::atomic_write` remains
//! valid and resolves to the same function.

pub use callisto_model::atomic::{atomic_write, ChangesetStorage};

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

    /// The re-export must be the *same* function item as the one in
    /// `callisto-model`, not a wrapper that could drift. Comparing the two as
    /// function pointers proves the alias resolves through, so the three tests
    /// above are exercising the moved implementation.
    #[test]
    fn re_export_resolves_to_the_model_implementation() {
        let via_alias: fn(&std::path::Path, &str, &ApplyPermit) -> std::io::Result<()> =
            atomic_write;
        let via_model: fn(&std::path::Path, &str, &ApplyPermit) -> std::io::Result<()> =
            callisto_model::atomic::atomic_write;
        assert!(std::ptr::fn_addr_eq(via_alias, via_model));
    }
}
