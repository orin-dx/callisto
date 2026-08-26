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
    fn run(&self, program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
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
///
/// Hermetic against the ambient machine's global git config: pins the
/// initial branch name (a developer machine's `init.defaultBranch` may not
/// be `main`) and disables commit/tag signing locally (a developer machine
/// may have `commit.gpgsign`/`tag.gpgsign` set globally -- under a test
/// runner that gives child processes no controlling TTY, like `cargo
/// nextest`, a real signing attempt can't prompt `pinentry` and hangs for
/// minutes instead of failing fast, or fails outright).
pub fn init_repo(root: &Path) {
    run_git(root, &["init", "-q", "-b", "main"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    run_git(root, &["config", "tag.gpgsign", "false"]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a `GIT_CONFIG_GLOBAL`-overriding config file that pins
    /// `commit.gpgsign`/`tag.gpgsign` to `true` and points `gpg.program` at a
    /// path that doesn't exist. This "poisons" whatever git command it's
    /// applied to as if it were running on a developer machine with commit
    /// signing enabled globally, without ever touching the real
    /// `~/.gitconfig` -- and because the fake gpg binary doesn't exist, a
    /// poisoned command fails immediately (`cannot exec`) rather than
    /// hanging on a real `pinentry` prompt, keeping this test fast.
    fn write_poisoned_global_config(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("poisoned.gitconfig");
        std::fs::write(
            &path,
            "[commit]\n\tgpgsign = true\n[tag]\n\tgpgsign = true\n[gpg]\n\tprogram = /nonexistent-gpg-xyz-binary\n",
        )
        .unwrap();
        path
    }

    /// Spec: `init_repo` must make the fixture repo hermetic against the
    /// *ambient* global git config -- specifically `commit.gpgsign` -- not
    /// just set up an identity. Reproduces the real bug directly: a `git
    /// commit` run against an `init_repo`-created repo, with
    /// `GIT_CONFIG_GLOBAL` pointed at a config that forces `commit.gpgsign =
    /// true` and an unusable `gpg.program`, must still succeed, because
    /// `init_repo`'s own local `commit.gpgsign = false` must take precedence
    /// over the poisoned global. Before the fix, this fails with "gpg failed
    /// to sign the data" / "cannot exec" instead of succeeding.
    #[test]
    fn init_repo_is_hermetic_against_ambient_commit_signing_config() {
        let workdir = tempfile::tempdir().unwrap();
        let repo_root = workdir.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);

        std::fs::write(repo_root.join("f.txt"), "hello\n").unwrap();
        run_git(&repo_root, &["add", "."]);

        let poisoned_config = write_poisoned_global_config(workdir.path());
        let status = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "test"])
            .current_dir(&repo_root)
            .env("GIT_CONFIG_GLOBAL", &poisoned_config)
            .status()
            .expect("git must be installed to run this test");

        assert!(
            status.success(),
            "init_repo-created repos must commit successfully even when the ambient global git \
             config forces commit signing on -- init_repo must set commit.gpgsign=false locally"
        );
    }
}
