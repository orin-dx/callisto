use std::path::{Path, PathBuf};

use callisto_model::{CommandRunner, CommitSha};
use callisto_vcs::{GitAccess, GitDataSource};

use crate::{parse_commit, ConventionalError, ParsedCommit};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceWindow {
    SinceCommit(CommitSha),
    FullHistory,
}

/// Fetches commits reachable from `HEAD` (down to `window`'s lower bound,
/// exclusive), scoped to those touching at least one of `pathspecs`.
///
/// Delegates entirely to [`GitAccess`], which tries native gix discovery
/// first (cheap and side-effect-free) and falls back to a `CommandRunner`-
/// shelled `git log --no-merges` when gix is unavailable -- most notably on
/// `wasm32`, where gix is excluded from that target's dependency set. Either
/// way the result comes back as the same `callisto_vcs::GitCommit` shape,
/// so the raw-message reconstruction and conventional-commit parsing below
/// runs identically regardless of which backend served the request.
pub fn fetch_commits(
    runner: &dyn CommandRunner,
    cwd: &Path,
    window: &InferenceWindow,
    pathspecs: &[PathBuf],
) -> Result<Vec<ParsedCommit>, ConventionalError> {
    let since_ref = match window {
        InferenceWindow::SinceCommit(sha) => Some(sha.as_str().to_string()),
        InferenceWindow::FullHistory => None,
    };

    let git = GitAccess::discover(cwd, runner);
    let commits = git.commits_since(since_ref.as_deref(), pathspecs)?;

    Ok(commits
        .into_iter()
        .map(|c| {
            let message = match c.body {
                Some(body) => format!("{}\n\n{}", c.summary, body),
                None => c.summary,
            };
            parse_commit(c.sha, &message)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_fixtures::git::{init_repo, run_git, PoisonedRunner};
    use callisto_model::{CommandError, CommandOutput};

    /// Spec: `fetch_commits` must resolve commits via
    /// `callisto_vcs::GitRepository` (gix), not by shelling out through the
    /// `CommandRunner` -- a runner that fails on every call must not
    /// prevent commits from being fetched, and pathspec filtering must
    /// still scope results to the given paths.
    #[test]
    fn test_fetch_commits_does_not_shell_out_and_filters_by_pathspec() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::create_dir_all(root.join("crates/pkg-a")).unwrap();
        std::fs::write(root.join("crates/pkg-a/file.txt"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add pkg-a file"]);

        std::fs::create_dir_all(root.join("crates/pkg-b")).unwrap();
        std::fs::write(root.join("crates/pkg-b/file.txt"), "b\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add pkg-b file"]);

        let runner = PoisonedRunner;
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];
        let commits = fetch_commits(&runner, root, &InferenceWindow::FullHistory, &pathspecs)
            .expect("fetch_commits must succeed even with a poisoned CommandRunner");

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject(), "add pkg-a file");
    }

    /// Spec: an empty pathspecs slice returns every commit, mirroring
    /// `git log` with no trailing `--` pathspec filter.
    #[test]
    fn test_fetch_commits_empty_pathspecs_returns_all_commits() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c1"]);

        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c2"]);

        let runner = PoisonedRunner;
        let commits = fetch_commits(&runner, root, &InferenceWindow::FullHistory, &[]).unwrap();

        assert_eq!(commits.len(), 2);
    }

    /// Spec: `InferenceWindow::SinceCommit` is an exclusive lower bound --
    /// the commit it points at is not itself included in the result.
    #[test]
    fn test_fetch_commits_since_commit_is_exclusive() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        std::fs::write(root.join("a.txt"), "a1\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c1"]);

        let since_sha = {
            let repo = callisto_vcs::GitRepository::discover(root).unwrap();
            repo.head_sha().unwrap()
        };

        std::fs::write(root.join("a.txt"), "a2\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c2"]);

        let runner = PoisonedRunner;
        let window = InferenceWindow::SinceCommit(since_sha);
        let commits = fetch_commits(&runner, root, &window, &[]).unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject(), "c2");
    }

    /// A directory guaranteed not to sit inside any Git repository, so
    /// `GitRepository::discover` fails exactly the way it unconditionally
    /// does on `wasm32` (gix is excluded from that target's dependency
    /// set) -- the native-testable stand-in for "gix is unavailable" that
    /// forces `fetch_commits` through the `CommandRunner` fallback.
    fn non_repo_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            callisto_vcs::GitRepository::discover(dir.path()).is_err(),
            "test fixture must not be discoverable as a Git repo"
        );
        dir
    }

    /// A `CommandRunner` double standing in for a real `git` binary on the
    /// fallback path. Returns a canned `git log` payload shaped exactly
    /// like the `--format=<RS>%H<US>%B` invocation `fetch_commits` is
    /// expected to issue, and records every invocation's args so the test
    /// can assert on the exact command shape.
    struct FakeGitLogRunner {
        stdout: String,
        calls: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl FakeGitLogRunner {
        fn new(stdout: impl Into<String>) -> Self {
            FakeGitLogRunner {
                stdout: stdout.into(),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for FakeGitLogRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: self.stdout.clone(),
                stderr: String::new(),
            })
        }
    }

    /// Builds a canned `git log` payload in the exact `<RS>%H<US>%B`
    /// record shape (record-separator `\x1e` prefixing each record,
    /// unit-separator `\x1f` between sha and raw message body) that
    /// `fetch_commits`'s `CommandRunner` fallback is expected to request
    /// and parse.
    fn canned_git_log_output(commits: &[(&str, &str)]) -> String {
        let mut out = String::new();
        for (sha, message) in commits {
            out.push('\u{1e}');
            out.push_str(sha);
            out.push('\u{1f}');
            out.push_str(message);
            out.push('\n'); // tformat's implicit trailing newline per entry
        }
        out
    }

    /// Spec: this is the bug under test. `fetch_commits` previously
    /// discarded `runner` entirely (`_runner: &dyn CommandRunner`) and
    /// hard-failed via `?` on `GitRepository::discover` with no fallback,
    /// silently dropping all conventional-commit inference whenever gix is
    /// unavailable (always true on wasm32). It must instead fall back to a
    /// `CommandRunner`-shelled `git log` and actually parse commits out of
    /// it -- not return empty/`None`.
    #[test]
    fn test_fetch_commits_falls_back_to_command_runner_when_gix_unavailable() {
        let dir = non_repo_dir();
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let stdout = canned_git_log_output(&[
            (&sha_a, "feat(core): add thing\n\nSome body text"),
            (&sha_b, "fix: bug"),
        ]);
        let runner = FakeGitLogRunner::new(stdout);
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];

        let commits = fetch_commits(
            &runner,
            dir.path(),
            &InferenceWindow::FullHistory,
            &pathspecs,
        )
        .expect(
            "fetch_commits must succeed via the CommandRunner fallback when gix is unavailable",
        );

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha().as_str(), sha_a);
        assert_eq!(commits[0].subject(), "add thing");
        assert_eq!(commits[1].sha().as_str(), sha_b);
        assert_eq!(commits[1].subject(), "bug");

        let calls = runner.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "fetch_commits should shell out exactly once, got {calls:?}"
        );
        let args = &calls[0];
        assert_eq!(args[0], "log");
        assert!(args.contains(&"--no-merges".to_string()));
        assert!(
            args.iter().any(|a| a.starts_with("--format=")),
            "expected a --format= arg, got {args:?}"
        );
        assert!(args.contains(&"HEAD".to_string()));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"crates/pkg-a".to_string()));
    }

    /// Spec: `InferenceWindow::SinceCommit` must translate into a
    /// `<sha>..HEAD` revision range on the `CommandRunner` fallback path,
    /// mirroring the exclusive-lower-bound semantics the gix path gets via
    /// `commits_since_with_pathspec`'s stop-at-`since` revwalk.
    #[test]
    fn test_fetch_commits_command_runner_fallback_uses_since_range() {
        let dir = non_repo_dir();
        let sha_since = CommitSha::parse(&"c".repeat(40)).unwrap();
        let sha_new = "d".repeat(40);
        let stdout = canned_git_log_output(&[(&sha_new, "feat: new")]);
        let runner = FakeGitLogRunner::new(stdout);

        let window = InferenceWindow::SinceCommit(sha_since.clone());
        let commits = fetch_commits(&runner, dir.path(), &window, &[]).unwrap();

        assert_eq!(commits.len(), 1);
        let calls = runner.calls.lock().unwrap();
        let args = &calls[0];
        assert!(
            args.contains(&format!("{}..HEAD", sha_since.as_str())),
            "expected a `<since>..HEAD` range arg, got {args:?}"
        );
    }

    /// Spec: a `CommandRunner` failure on the fallback path (e.g. `git`
    /// missing) must propagate as a real error, not be swallowed into an
    /// empty/silently-degraded commit list. Now routed through
    /// `GitAccess`/`GitDataSource`, so the error arrives wrapped as
    /// `ConventionalError::Vcs(VcsError::Command(_))` rather than the
    /// direct `ConventionalError::Command(_)` the old hand-rolled shell-out
    /// produced -- same propagation guarantee, new (centralized) shape.
    #[test]
    fn test_fetch_commits_propagates_command_runner_error() {
        struct FailingRunner;
        impl CommandRunner for FailingRunner {
            fn run(
                &self,
                _program: &str,
                _args: &[&str],
                _cwd: &Path,
            ) -> Result<CommandOutput, CommandError> {
                Err(CommandError::NotFound {
                    program: "git".to_string(),
                })
            }
        }

        let dir = non_repo_dir();
        let result = fetch_commits(
            &FailingRunner,
            dir.path(),
            &InferenceWindow::FullHistory,
            &[],
        );

        assert!(
            matches!(
                result,
                Err(ConventionalError::Vcs(callisto_vcs::VcsError::Command(_)))
            ),
            "expected ConventionalError::Vcs(VcsError::Command(_)), got {result:?}"
        );
    }
}
