//! Git-backed test doubles and repo-setup helpers shared across crates'
//! test suites.

use std::path::Path;

use callisto_model::{CommandError, CommandOutput, CommandRunner};

/// A [`CommandRunner`] double that errors on every invocation.
///
/// Used to prove that a code path resolves git state without shelling out
/// through `CommandRunner` at all (e.g. via `callisto_vcs::GitRepository`/gix
/// instead) -- a runner that always fails must not affect the result.
pub struct PoisonedRunner;

impl CommandRunner for PoisonedRunner {
    fn run(
        &self,
        program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        Err(CommandError::Io {
            program: program.to_string(),
            message: "poisoned runner: this code path must not shell out to git".to_string(),
        })
    }
}

/// Runs `git` with `args` in `dir`, panicking if `git` is unavailable or the
/// invocation fails. Test-only helper for building fixture repos.
pub fn run_git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git must be installed to run this test");
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}

/// Initializes `root` as a git repo with a fixed test identity
/// (`test@example.com` / `Test`), suitable for building fixture repos across
/// test suites.
pub fn init_repo(root: &Path) {
    run_git(root, &["init", "-q"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
}
