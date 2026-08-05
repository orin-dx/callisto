//! Shared dry-run invariant harness.
//!
//! Every write-capable callisto command has the same contract under
//! `--dry-run`: compute and report what *would* happen, mutate nothing. That
//! contract was previously asserted ad hoc, per command, against whichever
//! specific file the test author happened to think of (`add` checked for a
//! `.changeset/*.md`; `version` checked one manifest's version string). Bugs
//! slipped through the gaps -- `pre enter`/`pre exit` and `init` never
//! consulted the dry-run flag at all, and no per-command assertion was
//! looking at the files they touched.
//!
//! [`assert_no_disk_mutation`] replaces the per-file guesswork with a whole
//! tree comparison: every file's path and content hash before the closure,
//! versus after. Content hashes rather than mtimes deliberately -- an atomic
//! rewrite of identical bytes is not a semantic mutation, and mtime-based
//! comparison would report it as one.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// A content-addressed snapshot of every file under a directory tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    root: PathBuf,
    entries: BTreeMap<PathBuf, u64>,
}

impl TreeSnapshot {
    /// Walks `root` recursively, hashing every regular file's bytes.
    ///
    /// The `.git` directory is included: `git add`, `git rm --cached`, and
    /// `git tag` are side effects this harness is explicitly meant to catch,
    /// and they land in `.git/index` and `.git/refs`.
    pub fn capture(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let mut entries = BTreeMap::new();
        collect(&root, &root, &mut entries);
        Self { root, entries }
    }

    /// Paths present in `self` but not `other`, and vice versa, plus paths
    /// whose contents differ -- all relative to the snapshot root.
    fn diff(&self, other: &Self) -> Vec<String> {
        let mut out = Vec::new();
        for (path, hash) in &self.entries {
            match other.entries.get(path) {
                None => out.push(format!("deleted: {}", path.display())),
                Some(other_hash) if other_hash != hash => {
                    out.push(format!("modified: {}", path.display()));
                }
                Some(_) => {}
            }
        }
        for path in other.entries.keys() {
            if !self.entries.contains_key(path) {
                out.push(format!("created: {}", path.display()));
            }
        }
        out.sort();
        out
    }
}

fn collect(root: &Path, dir: &Path, entries: &mut BTreeMap<PathBuf, u64>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => collect(root, &path, entries),
            Ok(ft) if ft.is_file() => {
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let mut hasher = DefaultHasher::new();
                bytes.hash(&mut hasher);
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                let rel_str = rel.to_string_lossy();
                if rel_str.contains(".git/") && rel_str.ends_with(".lock") {
                    continue;
                }
                let _prev = entries.insert(rel, hasher.finish());
            }
            _ => {}
        }
    }
}

/// Runs `operation` and asserts it left every file under `root` byte-identical,
/// created nothing, and deleted nothing.
///
/// This is the uniform invariant for every command invoked with `--dry-run`
/// (equivalently: any code path that failed to obtain an `ApplyPermit`).
///
/// Returns whatever `operation` returned, so a test can additionally assert on
/// the command's reported preview output.
///
/// # Panics
///
/// Panics if any file under `root` was created, deleted, or had its contents changed
/// during `operation`. The panic message lists every offending path.
pub fn assert_no_disk_mutation<T>(root: impl AsRef<Path>, operation: impl FnOnce() -> T) -> T {
    let root = root.as_ref();
    let before = TreeSnapshot::capture(root);
    let result = operation();
    let after = TreeSnapshot::capture(root);

    let changes = before.diff(&after);
    assert!(
        changes.is_empty(),
        "dry-run mutated the workspace at `{}`:\n  {}",
        before.root.display(),
        changes.join("\n  ")
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_when_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

        let out = assert_no_disk_mutation(dir.path(), || 42);
        assert_eq!(out, 42);
    }

    #[test]
    #[should_panic(expected = "created: new.txt")]
    fn detects_a_created_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_no_disk_mutation(dir.path(), || {
            std::fs::write(dir.path().join("new.txt"), "surprise").unwrap();
        });
    }

    #[test]
    #[should_panic(expected = "modified: a.txt")]
    fn detects_a_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before").unwrap();
        assert_no_disk_mutation(dir.path(), || {
            std::fs::write(dir.path().join("a.txt"), "after").unwrap();
        });
    }

    #[test]
    #[should_panic(expected = "deleted: a.txt")]
    fn detects_a_deleted_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "gone soon").unwrap();
        assert_no_disk_mutation(dir.path(), || {
            std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        });
    }

    /// A rewrite of byte-identical content is not a semantic mutation and must
    /// not trip the assertion -- this is why the snapshot hashes contents
    /// instead of comparing mtimes.
    #[test]
    fn ignores_a_rewrite_of_identical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "same").unwrap();
        assert_no_disk_mutation(dir.path(), || {
            std::fs::write(&path, "same").unwrap();
        });
    }

    #[test]
    fn walks_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("deep.txt"), "x").unwrap();

        let snap = TreeSnapshot::capture(dir.path());
        assert!(snap.entries.contains_key(Path::new("a/b/c/deep.txt")));
    }
}
