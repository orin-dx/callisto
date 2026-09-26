use std::path::PathBuf;

use callisto_model::{CommitSha, CommitWalker};

use crate::conventional::{parse_commit, ConventionalError, ParsedCommit};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceWindow {
    SinceCommit(CommitSha),
    FullHistory,
}

/// Fetches commits reachable from `HEAD` (down to `window`'s lower bound,
/// exclusive), scoped to those touching at least one of `pathspecs`, and
/// parses each one as a conventional commit.
///
/// Sourcing the history is entirely `walker`'s business. This crate names
/// only the Layer 1 [`CommitWalker`] contract, so it links against no VCS
/// engine at all: callers hand it `callisto_model::vcs::GitAccess` or a test double.
pub fn fetch_commits(
    walker: &dyn CommitWalker,
    window: &InferenceWindow,
    pathspecs: &[PathBuf],
) -> Result<Vec<ParsedCommit>, ConventionalError> {
    let since_ref = match window {
        InferenceWindow::SinceCommit(sha) => Some(sha.as_str().to_string()),
        InferenceWindow::FullHistory => None,
    };

    let commits = walker.commits_since(since_ref.as_deref(), pathspecs)?;

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
    use std::path::Path;

    use super::*;
    use callisto_fixtures::git::{init_repo, run_git, GitRunner};
    use callisto_model::vcs::GitAccess;
    use callisto_model::{CommandError, CommandOutput, CommandRunner, CommitRecord, CommitWalkError};

    /// A [`CommitWalker`] double built from `callisto-model` types alone --
    /// it links against no VCS crate whatsoever. Records every
    /// `commits_since` argument pair so the test can assert on the exact
    /// window/pathspec translation `fetch_commits` performs.
    struct MockWalker {
        records: Vec<CommitRecord>,
        calls: std::sync::Mutex<Vec<(Option<String>, Vec<PathBuf>)>>,
    }

    impl MockWalker {
        fn new(records: Vec<CommitRecord>) -> Self {
            MockWalker {
                records,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl CommitWalker for MockWalker {
        fn commits_since(
            &self,
            since_ref: Option<&str>,
            pathspecs: &[PathBuf],
        ) -> Result<Vec<CommitRecord>, CommitWalkError> {
            self.calls
                .lock()
                .unwrap()
                .push((since_ref.map(str::to_string), pathspecs.to_vec()));
            Ok(self.records.clone())
        }
    }

    fn record(sha: &str, summary: &str, body: Option<&str>) -> CommitRecord {
        CommitRecord {
            sha: CommitSha::parse(sha).unwrap(),
            summary: summary.to_string(),
            body: body.map(str::to_string),
        }
    }

    /// Spec: `fetch_commits` is drivable by any `callisto_model::CommitWalker`
    /// implementation, with no `callisto_model::vcs` type involved at all. This is
    /// the abstraction-is-real proof: a mock built purely from Layer 1 types
    /// satisfies the whole contract, and summary/body are rejoined into the
    /// raw commit message before conventional parsing exactly as they are
    /// for the real backends.
    #[test]
    fn test_fetch_commits_accepts_any_commit_walker_impl() {
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let walker = MockWalker::new(vec![
            record(&sha_a, "feat(core): add thing", Some("Some body text")),
            record(&sha_b, "fix: bug", None),
        ]);
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];

        let commits = fetch_commits(&walker, &InferenceWindow::FullHistory, &pathspecs).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha().as_str(), sha_a);
        assert_eq!(commits[0].subject(), "add thing");
        assert_eq!(commits[1].sha().as_str(), sha_b);
        assert_eq!(commits[1].subject(), "bug");

        let calls = walker.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "expected exactly one walk, got {calls:?}");
        assert_eq!(calls[0].0, None, "FullHistory must pass no `since` bound");
        assert_eq!(calls[0].1, pathspecs);
    }

    /// Spec: `InferenceWindow::SinceCommit` translates into the walker's
    /// `since_ref` argument verbatim, so the exclusive-lower-bound decision
    /// belongs to the walker implementation, not to this crate.
    #[test]
    fn test_fetch_commits_passes_since_commit_through_to_walker() {
        let since = CommitSha::parse(&"c".repeat(40)).unwrap();
        let walker = MockWalker::new(vec![record(&"d".repeat(40), "feat: new", None)]);

        let window = InferenceWindow::SinceCommit(since.clone());
        let commits = fetch_commits(&walker, &window, &[]).unwrap();

        assert_eq!(commits.len(), 1);
        let calls = walker.calls.lock().unwrap();
        assert_eq!(calls[0].0.as_deref(), Some(since.as_str()));
        assert!(calls[0].1.is_empty());
    }

    /// Spec: a walker failure must propagate as
    /// `ConventionalError::CommitWalk`, never be swallowed into an empty
    /// commit list -- proven without any VCS backend in the picture.
    #[test]
    fn test_fetch_commits_propagates_walker_error() {
        struct FailingWalker;
        impl CommitWalker for FailingWalker {
            fn commits_since(
                &self,
                _since_ref: Option<&str>,
                _pathspecs: &[PathBuf],
            ) -> Result<Vec<CommitRecord>, CommitWalkError> {
                Err(CommitWalkError::RefNotFound {
                    ref_name: "v9.9.9".to_string(),
                })
            }
        }

        let result = fetch_commits(&FailingWalker, &InferenceWindow::FullHistory, &[]);

        assert!(
            matches!(
                result,
                Err(ConventionalError::CommitWalk(CommitWalkError::RefNotFound { .. }))
            ),
            "expected ConventionalError::CommitWalk(RefNotFound), got {result:?}"
        );
    }

    /// Spec: the real `callisto_model::vcs::GitAccess` backend satisfies
    /// `CommitWalker`, so production wiring works end to end and pathspec
    /// filtering scopes commits to the given paths.
    #[test]
    fn test_real_git_access_backend_satisfies_commit_walker_and_filters_by_pathspec() {
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

        let runner = GitRunner;
        let git = GitAccess::new(root, &runner);
        let pathspecs = vec![PathBuf::from("crates/pkg-a")];
        let commits = fetch_commits(&git, &InferenceWindow::FullHistory, &pathspecs)
            .expect("fetch_commits must succeed against a real repository");

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

        let runner = GitRunner;
        let git = GitAccess::new(root, &runner);
        let commits = fetch_commits(&git, &InferenceWindow::FullHistory, &[]).unwrap();

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

        let since_sha = GitAccess::new(root, &GitRunner).head_sha().unwrap();

        std::fs::write(root.join("a.txt"), "a2\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: c2"]);

        let runner = GitRunner;
        let git = GitAccess::new(root, &runner);
        let window = InferenceWindow::SinceCommit(since_sha);
        let commits = fetch_commits(&git, &window, &[]).unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject(), "c2");
    }

    /// A `CommandRunner` double standing in for the `git` binary. Returns a
    /// canned `git log` payload in the `--format=<RS>%H<US>%B` record shape
    /// `GitAccess` issues.
    struct FakeGitLogRunner {
        stdout: String,
    }

    impl CommandRunner for FakeGitLogRunner {
        fn run(&self, program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: self.stdout.clone(),
                stderr: String::new(),
            })
        }
    }

    /// Spec: `fetch_commits` parses the commits `GitAccess` reads from
    /// `git log` rather than degrading to an empty list. (The exact argv is
    /// `callisto_model::vcs`'s contract.)
    #[test]
    fn test_inference_parses_commits_read_through_git_access() {
        let dir = tempfile::tempdir().unwrap();
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let stdout =
            format!("\u{1e}{sha_a}\u{1f}feat(core): add thing\n\nSome body text\n\u{1e}{sha_b}\u{1f}fix: bug\n");
        let runner = FakeGitLogRunner { stdout };
        let git = GitAccess::new(dir.path(), &runner);

        let commits =
            fetch_commits(&git, &InferenceWindow::FullHistory, &[]).expect("inference must parse the git log records");

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha().as_str(), sha_a);
        assert_eq!(commits[0].subject(), "add thing");
        assert_eq!(commits[1].sha().as_str(), sha_b);
        assert_eq!(commits[1].subject(), "bug");
    }

    /// Spec: a `CommandError` raised deep inside the VCS backend keeps its
    /// identity as it narrows through `From<VcsError> for CommitWalkError`
    /// -- it must arrive as `CommitWalk(Command(_))`, not be flattened into
    /// the catch-all `Backend` variant and not be swallowed into an
    /// empty/silently-degraded commit list.
    #[test]
    fn test_backend_command_error_narrows_to_commit_walk_command() {
        struct FailingRunner;
        impl CommandRunner for FailingRunner {
            fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
                Err(CommandError::NotFound {
                    program: "git".to_string(),
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let git = GitAccess::new(dir.path(), &FailingRunner);
        let result = fetch_commits(&git, &InferenceWindow::FullHistory, &[]);

        assert!(
            matches!(result, Err(ConventionalError::CommitWalk(CommitWalkError::Command(_)))),
            "expected ConventionalError::CommitWalk(Command(_)), got {result:?}"
        );
    }
}
