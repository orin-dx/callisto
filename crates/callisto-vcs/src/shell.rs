//! `CommandRunner`-shelled implementation of [`GitDataSource`], consolidated
//! from five independently hand-rolled `git`-subprocess fallbacks that used
//! to live at each call site (`callisto-graph`'s `changed.rs`, `tags.rs`,
//! `commands/tag.rs`, `aggregate.rs`, and `callisto-conventional`'s
//! `window.rs`). Every operation here is reachable purely via
//! [`callisto_model::CommandRunner`], so it works on every target,
//! including `wasm32`, where native `gix` (`GitRepository`) is entirely
//! unavailable.
//!
//! Callers should not construct [`ShellGit`] directly except in tests that
//! specifically want to exercise the shell backend in isolation; use
//! [`crate::GitAccess::discover`] instead, which selects between this and
//! the native backend automatically.

use std::path::{Path, PathBuf};

use callisto_model::{ApplyPermit, CommandRunner, CommitSha, TagName};

use crate::{GitCommit, GitDataSource, VcsError};

/// Record separator placed immediately before each commit's fields in
/// `git log --format=` output, so a commit message containing `\n` can
/// never be mistaken for a record boundary.
const RECORD_SEP: char = '\u{1e}';

/// Unit separator between a commit's sha and its raw message body in
/// `git log --format=` output.
const FIELD_SEP: char = '\u{1f}';

/// Redacts known registry/VCS credential env-var values and any URL
/// userinfo component from raw `git` subprocess stderr before it is
/// embedded in a [`VcsError`] -- a failing `git` invocation can surface an
/// authenticated remote URL (e.g. GitHub Actions'
/// `https://x-access-token:TOKEN@github.com/...`) verbatim in its own
/// error output, and that text flows into `--format json` and miette
/// diagnostic output downstream.
fn redact_git_stderr(text: &str) -> String {
    callisto_model::redact_known_secrets(
        text,
        &callisto_model::known_credential_env_values(std::env::vars()),
    )
}

/// Shells `git` subcommands via a [`CommandRunner`] to implement
/// [`GitDataSource`]. See the module docs for the consolidation this
/// replaces.
pub struct ShellGit<'r> {
    runner: &'r dyn CommandRunner,
    root: PathBuf,
}

impl<'r> ShellGit<'r> {
    pub fn new(runner: &'r dyn CommandRunner, root: impl Into<PathBuf>) -> Self {
        ShellGit {
            runner,
            root: root.into(),
        }
    }
}

/// Compiles `glob` into a [`globset::GlobMatcher`], surfacing a malformed
/// pattern as [`VcsError::InvalidGlob`] rather than letting it silently
/// disable filtering (which would match every tag -- see
/// `GitRepository::list_tags`'s own doc comment for why that's a real
/// correctness risk for release tagging).
fn compile_glob(glob: &str) -> Result<globset::GlobMatcher, VcsError> {
    globset::Glob::new(glob)
        .map(|g| g.compile_matcher())
        .map_err(|e| VcsError::InvalidGlob {
            pattern: glob.to_string(),
            message: e.to_string(),
        })
}

impl GitDataSource for ShellGit<'_> {
    fn head_sha(&self) -> Result<CommitSha, VcsError> {
        let output = self.runner.run("git", &["rev-parse", "HEAD"], &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git rev-parse HEAD` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        let sha_str = output.stdout_trimmed();
        CommitSha::parse(sha_str)
            .map_err(|e| VcsError::Git(format!("could not parse HEAD SHA `{sha_str}`: {e}")))
    }

    /// Always fetches the *full, unfiltered* tag list via `git tag --list`
    /// and applies `glob` (if any) locally with `globset` -- deliberately
    /// never delegates filtering to `git tag --list <pattern>`'s own
    /// (different) glob dialect. This is what guarantees byte-identical
    /// tag selection between this backend and the native `gix` path
    /// (`GitRepository::list_tags`, which also filters with `globset`)
    /// regardless of which one a caller ends up using.
    fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        let output = self.runner.run("git", &["tag", "--list"], &self.root)?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git tag --list` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        let all = output.stdout_lines().map(|s| s.to_string());

        let matcher = glob.map(compile_glob).transpose()?;

        Ok(all
            .filter(|t| matcher.as_ref().is_none_or(|m| m.is_match(t)))
            .map(TagName)
            .collect())
    }

    fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError> {
        let rev = format!("{refname}^{{commit}}");
        let output = self.runner.run(
            "git",
            &["rev-parse", "--verify", "--quiet", &rev],
            &self.root,
        )?;

        if !output.success() {
            return Ok(None);
        }
        let sha_str = output.stdout_trimmed();
        if sha_str.is_empty() {
            return Ok(None);
        }
        Ok(CommitSha::parse(sha_str).ok())
    }

    /// Shells `git log --no-merges --format=<RECORD_SEP>%H<FIELD_SEP>%B
    /// <range> [-- <pathspecs>]`, matching the native path's shape
    /// field-for-field: `--no-merges` excludes merge commits exactly like
    /// `GitRepository::commits_since_with_pathspec`'s parent-count skip;
    /// `<since>..HEAD` (or bare `HEAD` for no bound) gives the same
    /// exclusive-lower-bound revision range as the revwalk's stop-at-
    /// `since` logic; and the trailing `-- <pathspecs>` reproduces `git
    /// log`'s own pathspec-prefix filtering.
    ///
    /// Deliberately does *not* pre-resolve `since_ref` via a separate
    /// `rev-parse` round-trip: an unresolvable `since_ref` already makes
    /// `git log <since_ref>..HEAD` itself fail (non-zero exit), which is
    /// surfaced below as `Err` exactly like any other `git log` failure --
    /// one shell call either way.
    fn commits_since(
        &self,
        since_ref: Option<&str>,
        pathspecs: &[PathBuf],
    ) -> Result<Vec<GitCommit>, VcsError> {
        let mut args: Vec<String> = vec![
            "log".to_string(),
            "--no-merges".to_string(),
            format!("--format={RECORD_SEP}%H{FIELD_SEP}%B"),
        ];

        match since_ref {
            Some(r) => args.push(format!("{r}..HEAD")),
            None => args.push("HEAD".to_string()),
        }

        if !pathspecs.is_empty() {
            args.push("--".to_string());
            args.extend(pathspecs.iter().map(|p| p.display().to_string()));
        }

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.runner.run("git", &arg_refs, &self.root)?;

        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git log` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }

        parse_git_log_output(&output.stdout, &self.root)
    }

    fn create_tag(
        &self,
        name: &str,
        target_sha: &CommitSha,
        message: Option<&str>,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        // `--` marks the end of option parsing so `name` (validated
        // upstream by `is_valid_git_ref_name`, but defended here too) can
        // never be misread as a `git tag` flag even if it started with `-`.
        let output = match message {
            Some(msg) => self.runner.run(
                "git",
                &["tag", "-a", "-m", msg, "--", name, target_sha.as_str()],
                &self.root,
            )?,
            None => {
                self.runner
                    .run("git", &["tag", "--", name, target_sha.as_str()], &self.root)?
            }
        };
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git tag` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        Ok(())
    }

    fn create_floating_major(
        &self,
        major_name: &str,
        target_sha: &CommitSha,
        _permit: &ApplyPermit,
    ) -> Result<(), VcsError> {
        let output = self.runner.run(
            "git",
            &["tag", "-f", "--", major_name, target_sha.as_str()],
            &self.root,
        )?;
        if !output.success() {
            return Err(VcsError::Git(format!(
                "`git tag -f` failed in `{}`: {}",
                self.root.display(),
                redact_git_stderr(&output.stderr)
            )));
        }
        Ok(())
    }
}

/// Parses the `<RECORD_SEP>%H<FIELD_SEP>%B`-formatted `git log` output
/// produced by [`ShellGit::commits_since`] into [`GitCommit`]s, splitting
/// each raw message on its first blank line into `summary`/`body` --
/// mirroring how `gix`'s own commit-message parsing (used by
/// `GitRepository::commits_since_with_pathspec`) splits title from body,
/// so both backends hand callers the same shape.
fn parse_git_log_output(stdout: &str, cwd: &Path) -> Result<Vec<GitCommit>, VcsError> {
    let mut commits = Vec::new();

    for record in stdout.split(RECORD_SEP) {
        // `tformat:`-style `--format=` output (the default when the format
        // string doesn't start with `format:`/`tformat:`) appends a
        // trailing newline after every entry; strip it rather than the
        // message's own content.
        let record = record.trim_end_matches('\n');
        if record.is_empty() {
            continue;
        }

        let Some((sha_str, message)) = record.split_once(FIELD_SEP) else {
            return Err(VcsError::Git(format!(
                "could not parse `git log` output in `{}` into commit records: expected a \
                 `<sha>{FIELD_SEP:?}<message>` record, got: {record:?}",
                cwd.display()
            )));
        };

        let sha = CommitSha::parse(sha_str).map_err(|e| {
            VcsError::Git(format!(
                "could not parse `git log` output in `{}`: invalid commit SHA `{sha_str}`: {e}",
                cwd.display()
            ))
        })?;

        let message = message.replace("\r\n", "\n");
        let (summary, body) = match message.split_once("\n\n") {
            Some((title, rest)) => (title.trim_end().to_string(), Some(rest.to_string())),
            None => (message.trim_end().to_string(), None),
        };

        commits.push(GitCommit { sha, summary, body });
    }

    Ok(commits)
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
    use std::sync::Mutex;

    type ResponseFn = dyn Fn(&[&str]) -> Result<CommandOutput, CommandError> + Send + Sync;

    struct FakeRunner {
        calls: Mutex<Vec<Vec<String>>>,
        response: Box<ResponseFn>,
    }

    impl CommandRunner for FakeRunner {
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
            (self.response)(args)
        }
    }

    fn ok(stdout: impl Into<String>) -> CommandOutput {
        CommandOutput {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    #[test]
    fn test_list_tags_fetches_unfiltered_and_filters_locally_with_globset() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok("pkg-a@1.0.0\npkg-ab@1.0.0\nunrelated\n"))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let tags = git.list_tags(Some("pkg-a@*")).unwrap();

        assert_eq!(
            tags.into_iter().map(|t| t.0).collect::<Vec<_>>(),
            vec!["pkg-a@1.0.0".to_string()]
        );
        // Exactly one shell call, and it must not bake the glob into the
        // command -- filtering happens locally so both backends share
        // identical semantics.
        assert_eq!(*runner.calls.lock().unwrap(), vec![vec!["tag", "--list"]]);
    }

    #[test]
    fn test_list_tags_rejects_malformed_glob() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok("pkg-a@1.0.0\n"))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.list_tags(Some("pkg-a@{malformed"));

        assert!(matches!(result, Err(VcsError::InvalidGlob { .. })));
    }

    /// A failing `git tag --list` (e.g. a corrupted or locked repository)
    /// must surface as `Err`, not be silently treated as "zero tags" --
    /// every sibling method on this impl (`head_sha`, `resolve_commit`,
    /// ...) already checks `output.success()` before trusting stdout.
    #[test]
    fn test_list_tags_errors_on_failed_git_invocation() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: not a git repository".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.list_tags(None);

        assert!(
            matches!(result, Err(VcsError::Git(ref msg)) if msg.contains("not a git repository")),
            "expected Err(VcsError::Git(..)) mentioning the failure, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_commit_returns_none_on_failed_rev_parse() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        assert_eq!(git.resolve_commit("missing-tag").unwrap(), None);
    }

    #[test]
    fn test_resolve_commit_parses_sha_on_success() {
        let sha = "a".repeat(40);
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(format!("{}\n", "a".repeat(40))))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        assert_eq!(
            git.resolve_commit("v1.0.0").unwrap(),
            Some(CommitSha::parse(&sha).unwrap())
        );
    }

    #[test]
    fn test_commits_since_parses_record_and_field_separators_into_summary_and_body() {
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let stdout = format!(
            "{RECORD_SEP}{sha_a}{FIELD_SEP}feat: add thing\n\nSome body text\n{RECORD_SEP}{sha_b}{FIELD_SEP}fix: bug\n"
        );
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(stdout.clone()))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let commits = git.commits_since(None, &[]).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha.as_str(), sha_a);
        assert_eq!(commits[0].summary, "feat: add thing");
        assert_eq!(commits[0].body.as_deref(), Some("Some body text"));
        assert_eq!(commits[1].sha.as_str(), sha_b);
        assert_eq!(commits[1].summary, "fix: bug");
        assert_eq!(commits[1].body, None);
    }

    #[test]
    fn test_commits_since_builds_since_range_and_pathspec_args() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let sha = "c".repeat(40);
        git.commits_since(Some(&sha), &[PathBuf::from("crates/pkg-a")])
            .unwrap();

        let calls = runner.calls.lock().unwrap();
        let args = &calls[0];
        assert!(args.contains(&"--no-merges".to_string()));
        assert!(args.contains(&format!("{sha}..HEAD")));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"crates/pkg-a".to_string()));
    }

    #[test]
    fn test_commits_since_propagates_git_log_failure_instead_of_unbounded_walk() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: bad revision 'missing-ref..HEAD'".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.commits_since(Some("missing-ref"), &[]);

        assert!(
            matches!(result, Err(VcsError::Git(_))),
            "an unresolvable since_ref must surface as Err, not silently degrade to an \
             unbounded walk; got {result:?}"
        );
    }

    #[test]
    fn test_create_tag_annotated_shells_expected_args() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"d".repeat(40)).unwrap();

        git.create_tag("pkg-a@1.0.0", &sha, Some("Release pkg-a@1.0.0"), &permit())
            .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            vec![
                "tag",
                "-a",
                "-m",
                "Release pkg-a@1.0.0",
                "--",
                "pkg-a@1.0.0",
                sha.as_str(),
            ]
        );
    }

    #[test]
    fn test_create_tag_lightweight_shells_expected_args() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"e".repeat(40)).unwrap();

        git.create_tag("pkg-a@1.0.0", &sha, None, &permit())
            .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["tag", "--", "pkg-a@1.0.0", sha.as_str()]);
    }

    #[test]
    fn test_create_floating_major_shells_force_tag() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| Ok(ok(""))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"f".repeat(40)).unwrap();

        git.create_floating_major("pkg-a@1", &sha, &permit())
            .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["tag", "-f", "--", "pkg-a@1", sha.as_str()]);
    }

    #[test]
    fn test_head_sha_returns_current_head_commit() {
        let sha = "a".repeat(40);
        let sha_clone = sha.clone();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(move |_args| Ok(ok(format!("{}\n", sha_clone)))),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));

        let result = git.head_sha().unwrap();
        assert_eq!(result.as_str(), sha);
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec![vec!["rev-parse", "HEAD"]]
        );
    }

    #[test]
    fn test_head_sha_propagates_git_failure() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(|_args| {
                Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: not a git repository".to_string(),
                })
            }),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        assert!(matches!(git.head_sha(), Err(VcsError::Git(_))));
    }

    /// A leaking authenticated remote URL in `git`'s stderr (the realistic
    /// GitHub Actions shape: `https://x-access-token:TOKEN@github.com/...`)
    /// must not survive into a `VcsError` from any of the four operations
    /// that embed raw subprocess stderr.
    fn leaky_response(_args: &[&str]) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(128),
            stdout: String::new(),
            stderr: "fatal: unable to access 'https://x-access-token:ghs_leaked_secret@github.com/org/repo.git/': The requested URL returned error: 403".to_string(),
        })
    }

    #[test]
    fn head_sha_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let err = git.head_sha().expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn commits_since_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let err = git.commits_since(None, &[]).expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn create_tag_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();
        let err = git
            .create_tag("v1.0.0", &sha, None, &permit())
            .expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }

    #[test]
    fn create_floating_major_failure_redacts_credential() {
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            response: Box::new(leaky_response),
        };
        let git = ShellGit::new(&runner, PathBuf::from("."));
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();
        let err = git
            .create_floating_major("v1", &sha, &permit())
            .expect_err("must fail");
        let rendered = format!("{err}");
        assert!(!rendered.contains("ghs_leaked_secret"), "got: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "got: {rendered}");
    }
}
