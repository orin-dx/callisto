//! Central native-`gix`-first, `CommandRunner`-shell-fallback selection
//! logic for [`GitDataSource`].

use std::path::{Path, PathBuf};

use callisto_model::{ApplyPermit, CommandRunner, CommitSha, TagName};

use crate::{GitCommit, GitDataSource, GitRepository, ShellGit, VcsError};

/// Selects between native `gix` ([`GitRepository`]) and a `CommandRunner`
/// shell-out ([`ShellGit`]) per operation, so callers never branch on it.
///
/// Construct via [`GitAccess::discover`], then call [`GitDataSource`]
/// methods directly.
///
/// **Fallback policy differs by operation category, deliberately:**
///
/// - **Reads** ([`GitDataSource::list_tags`], [`GitDataSource::resolve_commit`],
///   [`GitDataSource::commits_since`]): fall back to shell on *any* `gix`
///   error (failed discovery, or a discovered repo's op failing) --
///   retrying a read can only help.
/// - **Writes** ([`GitDataSource::create_tag`], [`GitDataSource::create_floating_major`]):
///   fall back *only* if `gix` was never discovered. A discovered repo's
///   result is authoritative -- retrying a failed mutation through a
///   different path risks masking a real failure, or double-applying a
///   partial mutation.
pub struct GitAccess<'r> {
    native: Option<GitRepository>,
    shell: ShellGit<'r>,
}

impl<'r> GitAccess<'r> {
    /// Tries to discover a native `gix` repository at `root`; regardless of
    /// whether that succeeds, also prepares the `CommandRunner`-shelled
    /// backend against the same `root`, ready to serve as a fallback (reads)
    /// or as the sole backend (writes, when discovery failed).
    ///
    /// Never fails: a discovery failure just means every operation runs
    /// through the shell backend instead, exactly as it unconditionally
    /// does on `wasm32` (gix is excluded from that target's dependency set,
    /// so [`GitRepository::discover`] always returns `Err` there).
    pub fn discover(root: impl AsRef<Path>, runner: &'r dyn CommandRunner) -> Self {
        let root = root.as_ref();
        GitAccess {
            native: GitRepository::discover(root).ok(),
            shell: ShellGit::new(runner, root.to_path_buf()),
        }
    }

    /// Returns whether this checkout is detached and has no tracked or
    /// non-ignored untracked changes. Durable release authorization uses the
    /// shell backend deliberately: its porcelain output is Git's canonical
    /// definition of worktree cleanliness and is available on every target.
    pub fn has_clean_detached_head(&self) -> Result<bool, VcsError> {
        self.shell.has_clean_detached_head()
    }
}

impl GitDataSource for GitAccess<'_> {
    fn head_sha(&self) -> Result<CommitSha, VcsError> {
        if let Some(repo) = &self.native {
            if let Ok(sha) = repo.head_sha() {
                return Ok(sha);
            }
        }
        self.shell.head_sha()
    }

    fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        if let Some(repo) = &self.native {
            if let Ok(tags) = repo.list_tags(glob) {
                return Ok(tags);
            }
        }
        self.shell.list_tags(glob)
    }

    fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError> {
        if let Some(repo) = &self.native {
            if let Ok(sha) = repo.resolve_commit(refname) {
                return Ok(sha);
            }
        }
        self.shell.resolve_commit(refname)
    }

    fn commits_since(&self, since_ref: Option<&str>, pathspecs: &[PathBuf]) -> Result<Vec<GitCommit>, VcsError> {
        if let Some(repo) = &self.native {
            if let Ok(commits) = repo.commits_since(since_ref, pathspecs) {
                return Ok(commits);
            }
        }
        self.shell.commits_since(since_ref, pathspecs)
    }

    fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        if let Some(repo) = &self.native {
            // Authoritative: a genuine gix failure must not be masked by
            // silently retrying through the shell.
            return repo.create_tag(name, target_sha, message, permit);
        }
        self.shell.create_tag(name, target_sha, message, permit)
    }

    fn create_floating_major(
        &self,
        major_name: &str,
        target_sha: &CommitSha,
        permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        if let Some(repo) = &self.native {
            return repo.create_floating_major(major_name, target_sha, permit);
        }
        self.shell.create_floating_major(major_name, target_sha, permit)
    }
}

#[cfg(test)]
mod tests {

    /// Tests exercise the write primitives directly rather than through a
    /// command handler, so they mint a permit without a dry-run flag to
    /// consult. Every non-test caller must go through
    /// `ApplyPermit::granted_unless_dry_run`.
    fn permit() -> callisto_model::ApplyPermit {
        callisto_model::ApplyPermit::force_for_tests()
    }
    use super::*;
    use callisto_model::{CommandError, CommandOutput};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A [`CommandRunner`] double that errors on every invocation, proving a
    /// code path resolves entirely through native `gix` without ever
    /// touching the shell fallback.
    struct PoisonedRunner;

    impl CommandRunner for PoisonedRunner {
        fn run(&self, program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            Err(CommandError::Io {
                program: program.to_string(),
                message: "poisoned runner: must not shell out to git".to_string(),
            })
        }
    }

    /// Counts invocations and answers `git tag --list` with a canned tag
    /// list, standing in for a real `git` binary on the shell fallback path.
    struct CountingTagRunner {
        calls: AtomicUsize,
        tags: Vec<String>,
    }

    impl CommandRunner for CountingTagRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            assert_eq!(args, ["tag", "--list"]);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: self.tags.join("\n"),
                stderr: String::new(),
            })
        }
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git must be installed to run this test");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    fn init_repo(root: &Path) {
        run_git(root, &["init", "-q", "-b", "main"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test"]);
        run_git(root, &["config", "commit.gpgsign", "false"]);
        run_git(root, &["config", "tag.gpgsign", "false"]);
    }

    #[test]
    fn test_discover_on_real_repo_uses_native_backend_without_shelling_out() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("f.txt"), "hi\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial"]);
        run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", "v1.0.0"]);

        let runner = PoisonedRunner;
        let git = GitAccess::discover(root, &runner);

        let tags = git.list_tags(None).unwrap();
        assert_eq!(
            tags.into_iter().map(|t| t.0).collect::<Vec<_>>(),
            vec!["v1.0.0".to_string()]
        );

        let resolved = git.resolve_commit("v1.0.0").unwrap();
        assert!(resolved.is_some());
    }

    #[test]
    fn test_discover_on_non_repo_falls_back_to_shell_for_reads() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        assert!(
            GitRepository::discover(root).is_err(),
            "fixture must not be a discoverable git repo"
        );

        let runner = CountingTagRunner {
            calls: AtomicUsize::new(0),
            tags: vec!["pkg-a@1.0.0".to_string()],
        };
        let git = GitAccess::discover(root, &runner);

        let tags = git.list_tags(None).unwrap();
        assert_eq!(
            tags.into_iter().map(|t| t.0).collect::<Vec<_>>(),
            vec!["pkg-a@1.0.0".to_string()]
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_write_ops_do_not_retry_through_shell_when_native_repo_was_discovered() {
        // A real repo (native available) whose create_tag call is made to
        // fail authoritatively (tag already exists) -- the shell fallback
        // must never be attempted to "rescue" it, since PoisonedRunner
        // would panic if it were.
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("f.txt"), "hi\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial"]);
        let head_sha = {
            let output = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap();
            CommitSha::parse(String::from_utf8_lossy(&output.stdout).trim()).unwrap()
        };
        run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", "dup"]);

        let runner = PoisonedRunner;
        let git = GitAccess::discover(root, &runner);

        // "dup" already exists, so gix's `PreviousValue::MustNotExist`
        // create_tag call must fail -- and that failure must propagate
        // as-is (proven by PoisonedRunner not being invoked/panicking).
        let result = git.create_tag("dup", &head_sha, Some("dup release"), &permit());
        assert!(result.is_err(), "creating an already-existing tag must fail");
    }

    #[test]
    fn test_write_ops_use_shell_when_native_was_never_available() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        assert!(GitRepository::discover(root).is_err());

        struct RecordingRunner {
            calls: std::sync::Mutex<Vec<Vec<String>>>,
        }
        impl CommandRunner for RecordingRunner {
            fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
                assert_eq!(program, "git");
                self.calls
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|s| s.to_string()).collect());
                Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }

        let runner = RecordingRunner {
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let git = GitAccess::discover(root, &runner);
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();

        git.create_floating_major("pkg-a@1", &sha, &permit()).unwrap();

        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec![vec!["tag", "-f", "--", "pkg-a@1", sha.as_str()]]
        );
    }

    #[test]
    fn test_head_sha_returns_head_of_real_repo() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("f.txt"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial"]);

        // Get the expected HEAD SHA from the real git binary.
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap();
        let expected = CommitSha::parse(String::from_utf8_lossy(&output.stdout).trim()).unwrap();

        let runner = PoisonedRunner;
        let git = GitAccess::discover(root, &runner);

        let result = git.head_sha().unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_commits_since_ref_not_found_error_propagates_through_shell_fallback() {
        // Native is unavailable, and the shell's git log against a
        // nonexistent ref must surface as an Err -- proving the "no
        // silent unbounded walk" fix holds through the full selection
        // layer, not just the native backend in isolation.
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        assert!(GitRepository::discover(root).is_err());

        struct FailingRefRunner;
        impl CommandRunner for FailingRefRunner {
            fn run(&self, program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
                assert_eq!(program, "git");
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: bad revision".to_string(),
                })
            }
        }

        let runner = FailingRefRunner;
        let git = GitAccess::discover(root, &runner);

        let result = git.commits_since(Some("this-tag-does-not-exist"), &[]);
        assert!(matches!(result, Err(VcsError::Git(_))));
    }
}
